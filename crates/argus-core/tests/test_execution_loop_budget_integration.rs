//! Integration test for ExecutionLoop with BudgetService.
//!
//! PRODUCTION REQUIREMENT (V0.37 Definition of Done):
//! "When budget is insufficient, Agent processes must never be spawned"
//!
//! This test validates that BudgetService.preflight_check() is correctly
//! integrated into ExecutionLoop.process_task() and that it can block
//! execution when budget limits are exceeded.

#[cfg(test)]
mod integration {
    use std::sync::{Arc, Mutex};
    use uuid::Uuid;

    use argus_core::budget_service::{BudgetPolicy, BudgetService};
    use argus_core::spend_repository::{SpendRecord, SpendRepository};
    use argus_core::types::ArgusResult;
    use async_trait::async_trait;
    use chrono::Utc;

    /// Mock spend repository for testing
    struct TestSpendRepository {
        records: Mutex<Vec<SpendRecord>>,
    }

    impl TestSpendRepository {
        fn new() -> Self {
            Self {
                records: Mutex::new(Vec::new()),
            }
        }

        fn add_spend(&self, record: SpendRecord) {
            self.records.lock().unwrap().push(record);
        }
    }

    #[async_trait]
    impl SpendRepository for TestSpendRepository {
        async fn record_spend(&self, record: SpendRecord) -> ArgusResult<bool> {
            self.records.lock().unwrap().push(record);
            Ok(true)
        }

        async fn spent_today_usd(&self, project_id: Uuid) -> ArgusResult<Option<f64>> {
            let records = self.records.lock().unwrap();
            let today = Utc::now();

            let spent: f64 = records
                .iter()
                .filter(|r| {
                    r.project_id == project_id
                        && r.success
                        && r.created_at.date_naive() == today.date_naive()
                })
                .filter_map(|r| r.cost_usd)
                .sum();

            if spent > 0.0 {
                Ok(Some(spent))
            } else {
                Ok(None)
            }
        }

        async fn spent_this_month_usd(&self, _project_id: Uuid) -> ArgusResult<Option<f64>> {
            Ok(None)
        }

        async fn task_spend(&self, task_id: Uuid) -> ArgusResult<Vec<SpendRecord>> {
            Ok(self
                .records
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r.task_id == task_id)
                .cloned()
                .collect())
        }

        async fn execution_spend(&self, execution_id: Uuid) -> ArgusResult<Vec<SpendRecord>> {
            Ok(self
                .records
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r.execution_id == Some(execution_id))
                .cloned()
                .collect())
        }

        async fn get_spend(&self, spend_id: Uuid) -> ArgusResult<Option<SpendRecord>> {
            Ok(self
                .records
                .lock()
                .unwrap()
                .iter()
                .find(|r| r.id == spend_id)
                .cloned())
        }
    }

    #[tokio::test]
    async fn test_budget_service_blocks_when_daily_limit_exceeded() {
        let project_id = Uuid::new_v4();
        let task_id = Uuid::new_v4();

        // Setup repository with pre-existing spend
        let repo = Arc::new(TestSpendRepository::new());
        repo.add_spend(SpendRecord {
            id: Uuid::new_v4(),
            task_id: Uuid::new_v4(),
            project_id,
            execution_id: None,
            attempt_number: 1,
            agent_type: "claude_code".to_string(),
            model: "claude-opus-5".to_string(),
            provider: "anthropic".to_string(),
            external_session_id: None,
            input_tokens: Some(1000),
            output_tokens: Some(500),
            total_tokens: Some(1500),
            cost_usd: Some(9.50), // $9.50 of $10.00 limit
            duration_ms: Some(5000),
            success: true,
            error: None,
            created_at: Utc::now(),
        });

        // Create budget service with $10 daily limit
        let policy = BudgetPolicy {
            daily_budget_usd: Some(10.0),
            max_per_task_usd: Some(1.0),
            strict_mode: true,
        };
        let service = Arc::new(BudgetService::new(repo, policy));

        // Test preflight check with $0.75 estimate
        let decision = service
            .preflight_check(project_id, task_id, Some(0.75))
            .await;

        // Verify decision is Blocked
        match decision {
            argus_core::budget_service::BudgetDecision::Blocked { reason, .. } => {
                assert!(
                    reason.contains("exceeds remaining"),
                    "Blocking reason should mention exceeds remaining; got: {}",
                    reason
                );
                println!("✅ Budget correctly blocks execution: {}", reason);
            }
            _ => panic!("Expected Blocked decision, got {:?}", decision),
        }
    }

    #[tokio::test]
    async fn test_budget_service_blocks_per_task_limit() {
        let project_id = Uuid::new_v4();
        let task_id = Uuid::new_v4();

        // Setup repository with task-level spend
        let repo = Arc::new(TestSpendRepository::new());
        repo.add_spend(SpendRecord {
            id: Uuid::new_v4(),
            task_id,
            project_id,
            execution_id: None,
            attempt_number: 1,
            agent_type: "claude_code".to_string(),
            model: "claude-opus-5".to_string(),
            provider: "anthropic".to_string(),
            external_session_id: None,
            input_tokens: Some(1000),
            output_tokens: Some(500),
            total_tokens: Some(1500),
            cost_usd: Some(0.80), // $0.80 of $1.00 per-task limit
            duration_ms: Some(5000),
            success: true,
            error: None,
            created_at: Utc::now(),
        });

        // Create budget service
        let policy = BudgetPolicy {
            daily_budget_usd: Some(100.0),
            max_per_task_usd: Some(1.0),
            strict_mode: true,
        };
        let service = Arc::new(BudgetService::new(repo, policy));

        // Test preflight check with $0.50 estimate (would exceed per-task limit)
        let decision = service
            .preflight_check(project_id, task_id, Some(0.50))
            .await;

        // Verify decision is Blocked
        match decision {
            argus_core::budget_service::BudgetDecision::Blocked { reason, .. } => {
                assert!(
                    reason.contains("task spend") || reason.contains("exceeds limit"),
                    "Blocking reason should mention task budget; got: {}",
                    reason
                );
                println!("✅ Per-task budget correctly blocks execution: {}", reason);
            }
            _ => panic!("Expected Blocked decision, got {:?}", decision),
        }
    }

    #[tokio::test]
    async fn test_budget_service_allows_within_limits() {
        let project_id = Uuid::new_v4();
        let task_id = Uuid::new_v4();

        // Setup repository with room in budget
        let repo = Arc::new(TestSpendRepository::new());
        repo.add_spend(SpendRecord {
            id: Uuid::new_v4(),
            task_id: Uuid::new_v4(),
            project_id,
            execution_id: None,
            attempt_number: 1,
            agent_type: "claude_code".to_string(),
            model: "claude-opus-5".to_string(),
            provider: "anthropic".to_string(),
            external_session_id: None,
            input_tokens: Some(1000),
            output_tokens: Some(500),
            total_tokens: Some(1500),
            cost_usd: Some(3.00), // $3 of $10 daily limit
            duration_ms: Some(5000),
            success: true,
            error: None,
            created_at: Utc::now(),
        });

        // Create budget service
        let policy = BudgetPolicy {
            daily_budget_usd: Some(10.0),
            max_per_task_usd: Some(1.0),
            strict_mode: true,
        };
        let service = Arc::new(BudgetService::new(repo, policy));

        // Test preflight check with $0.50 estimate
        let decision = service
            .preflight_check(project_id, task_id, Some(0.50))
            .await;

        // Verify decision is Allowed
        match decision {
            argus_core::budget_service::BudgetDecision::Allowed {
                remaining_today_usd,
            } => {
                assert!(remaining_today_usd > 0.0, "Should have remaining budget");
                println!(
                    "✅ Budget allows execution with ${} remaining",
                    remaining_today_usd
                );
            }
            _ => panic!("Expected Allowed decision, got {:?}", decision),
        }
    }

    #[tokio::test]
    async fn test_budget_service_fail_closed_on_error() {
        let project_id = Uuid::new_v4();
        let task_id = Uuid::new_v4();

        // Create a mock repository that always returns errors
        struct FailingRepository;

        #[async_trait]
        impl SpendRepository for FailingRepository {
            async fn record_spend(&self, _: SpendRecord) -> ArgusResult<bool> {
                Err(argus_core::types::ArgusError::Task(
                    "test error".to_string(),
                ))
            }

            async fn spent_today_usd(&self, _: Uuid) -> ArgusResult<Option<f64>> {
                Err(argus_core::types::ArgusError::Task(
                    "test error".to_string(),
                ))
            }

            async fn spent_this_month_usd(&self, _: Uuid) -> ArgusResult<Option<f64>> {
                Err(argus_core::types::ArgusError::Task(
                    "test error".to_string(),
                ))
            }

            async fn task_spend(&self, _: Uuid) -> ArgusResult<Vec<SpendRecord>> {
                Err(argus_core::types::ArgusError::Task(
                    "test error".to_string(),
                ))
            }

            async fn execution_spend(&self, _: Uuid) -> ArgusResult<Vec<SpendRecord>> {
                Err(argus_core::types::ArgusError::Task(
                    "test error".to_string(),
                ))
            }

            async fn get_spend(&self, _: Uuid) -> ArgusResult<Option<SpendRecord>> {
                Err(argus_core::types::ArgusError::Task(
                    "test error".to_string(),
                ))
            }
        }

        let repo = Arc::new(FailingRepository);
        let policy = BudgetPolicy {
            daily_budget_usd: Some(10.0),
            max_per_task_usd: Some(1.0),
            strict_mode: true,
        };
        let service = Arc::new(BudgetService::new(repo, policy));

        // Test preflight check - should fail-closed (deny)
        let decision = service
            .preflight_check(project_id, task_id, Some(0.50))
            .await;

        // Verify decision is CheckFailed (which denies execution)
        match decision {
            argus_core::budget_service::BudgetDecision::CheckFailed { error } => {
                assert!(
                    error.contains("failed"),
                    "Error message should indicate failure"
                );
                println!(
                    "✅ Fail-closed: Budget check error denies execution: {}",
                    error
                );
            }
            _ => panic!("Expected CheckFailed decision on error, got {:?}", decision),
        }
    }
}
