//! Test that accounting degraded state blocks all Paid Agent execution.
//!
//! PRODUCTION REQUIREMENT (V0.37.2 Definition of Done):
//! "When spending ledger fails, system enters DEGRADED state and
//!  blocks ALL subsequent Paid Agent execution with explicit error."

#[cfg(test)]
mod accounting_blocking_tests {
    use std::sync::{Arc, Mutex};
    use uuid::Uuid;

    use argus_core::accounting_state::RuntimeAccountingState;
    use argus_core::budget_service::{BudgetDecision, BudgetPolicy, BudgetService};
    use argus_core::spend_repository::{SpendRecord, SpendRepository};
    use argus_core::types::ArgusResult;
    use async_trait::async_trait;
    use chrono::{Datelike, Utc};

    /// Mock spend repository for accounting tests
    struct TestSpendRepository {
        records: Mutex<Vec<SpendRecord>>,
    }

    impl TestSpendRepository {
        fn new() -> Self {
            Self {
                records: Mutex::new(Vec::new()),
            }
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

        async fn spent_this_month_usd(&self, project_id: Uuid) -> ArgusResult<Option<f64>> {
            let records = self.records.lock().unwrap();
            let today = Utc::now();

            let spent: f64 = records
                .iter()
                .filter(|r| {
                    r.project_id == project_id && r.success && r.created_at.month() == today.month()
                })
                .filter_map(|r| r.cost_usd)
                .sum();

            if spent > 0.0 {
                Ok(Some(spent))
            } else {
                Ok(None)
            }
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
    async fn test_accounting_degraded_blocks_execution() {
        // Setup budget service with accounting state
        let spend_repo = Arc::new(TestSpendRepository::new());
        let policy = BudgetPolicy::default();
        let budget_service = BudgetService::new(spend_repo, policy);

        // Create and degrade accounting state
        let accounting_state = RuntimeAccountingState::new();
        accounting_state
            .mark_degraded("spend recording failed: database connection lost")
            .await;

        let budget_service = budget_service.with_accounting_state(accounting_state);

        // Attempt preflight check - should be blocked
        let decision = budget_service
            .preflight_check(Uuid::new_v4(), Uuid::new_v4(), Some(0.01))
            .await;

        // Verify it's blocked with accounting_degraded message
        match decision {
            BudgetDecision::CheckFailed { error } => {
                assert!(
                    error.contains("accounting_degraded"),
                    "Expected 'accounting_degraded' in error, got: {}",
                    error
                );
                assert!(
                    error.contains("database connection lost"),
                    "Expected degraded reason in error, got: {}",
                    error
                );
                println!(
                    "✓ Accounting degraded correctly blocks execution: {}",
                    error
                );
            }
            other => panic!(
                "Expected CheckFailed with accounting_degraded, got: {:?}",
                other
            ),
        }
    }

    #[tokio::test]
    async fn test_accounting_recovery_allows_execution() {
        // Setup budget service with accounting state
        let spend_repo = Arc::new(TestSpendRepository::new());
        let policy = BudgetPolicy::default();
        let budget_service = BudgetService::new(spend_repo, policy);

        // Create accounting state, mark degraded, then recover
        let accounting_state = RuntimeAccountingState::new();
        accounting_state
            .mark_degraded("temporary spend failure")
            .await;

        // Recover the accounting state
        accounting_state.mark_healthy().await;

        let budget_service = budget_service.with_accounting_state(accounting_state);

        // Preflight check should now allow execution
        let decision = budget_service
            .preflight_check(Uuid::new_v4(), Uuid::new_v4(), Some(0.01))
            .await;

        match decision {
            BudgetDecision::Allowed {
                remaining_today_usd,
            } => {
                println!(
                    "✓ Accounting recovered, execution allowed. Remaining: ${}",
                    remaining_today_usd
                );
            }
            other => panic!("Expected Allowed after recovery, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_accounting_state_tracks_degraded_reason() {
        let accounting_state = RuntimeAccountingState::new();

        // Verify initial healthy state
        let state = accounting_state.get_state().await;
        assert!(!state.is_degraded());
        assert!(state.reason.is_none());

        // Mark degraded with specific reason
        let reason = "PostgreSQL connection pool exhausted";
        accounting_state.mark_degraded(reason).await;

        let state = accounting_state.get_state().await;
        assert!(state.is_degraded());
        assert_eq!(state.reason, Some(reason.to_string()));
        println!(
            "✓ Degraded state tracks reason: {}",
            state.reason.unwrap_or_default()
        );

        // Recover
        accounting_state.mark_healthy().await;
        let state = accounting_state.get_state().await;
        assert!(!state.is_degraded());
        assert!(state.reason.is_none());
        println!("✓ Recovery clears reason");
    }
}
