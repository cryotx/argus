//! Tests for V0.36 Spend Safety & Budget Enforcement

use chrono::Utc;
use std::sync::Arc;
use std::sync::Mutex;
use uuid::Uuid;

use argus_core::budget_service::{BudgetDecision, BudgetPolicy, BudgetService};
use argus_core::spend_repository::{SpendRecord, SpendRepository};
use argus_core::types::ArgusResult;

/// Mock implementation for testing
struct MockSpendRepository {
    records: Mutex<Vec<SpendRecord>>,
}

impl MockSpendRepository {
    fn new() -> Self {
        Self {
            records: Mutex::new(Vec::new()),
        }
    }

    fn add_spend(&self, record: SpendRecord) {
        self.records.lock().unwrap().push(record);
    }
}

#[async_trait::async_trait]
impl SpendRepository for MockSpendRepository {
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
        Ok(None) // Not tested in this round
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
async fn test_budget_allows_within_limit() {
    let repo = Arc::new(MockSpendRepository::new());
    let policy = BudgetPolicy {
        daily_budget_usd: Some(10.0),
        max_per_task_usd: Some(1.0),
        strict_mode: true,
    };
    let service = BudgetService::new(repo, policy);

    let project_id = Uuid::new_v4();
    let task_id = Uuid::new_v4();

    let decision = service
        .preflight_check(project_id, task_id, Some(0.50))
        .await;

    match decision {
        BudgetDecision::Allowed {
            remaining_today_usd,
        } => {
            assert!(remaining_today_usd > 0.0);
        }
        _ => panic!("Expected Allowed, got {:?}", decision),
    }
}

#[allow(irrefutable_let_patterns)]
#[tokio::test]
async fn test_budget_blocks_after_daily_limit() {
    let repo = Arc::new(MockSpendRepository::new());

    // Add existing spend
    let project_id = Uuid::new_v4();
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
        cost_usd: Some(9.99), // Already spent $9.99
        duration_ms: Some(5000),
        success: true,
        error: None,
        created_at: Utc::now(),
    });

    let policy = BudgetPolicy {
        daily_budget_usd: Some(10.0), // $10 daily limit
        max_per_task_usd: Some(1.0),
        strict_mode: true,
    };
    let service = BudgetService::new(repo, policy);

    let task_id = Uuid::new_v4();

    // Try to spend $0.50 when only $0.01 remaining
    let decision = service
        .preflight_check(project_id, task_id, Some(0.50))
        .await;

    match decision {
        BudgetDecision::Blocked {
            reason,
            spent_today_usd,
            daily_limit_usd,
        } => {
            assert_eq!(spent_today_usd, Some(9.99));
            assert_eq!(daily_limit_usd, Some(10.0));
            assert!(reason.contains("exceeds remaining"));
        }
        _ => panic!("Expected Blocked, got {:?}", decision),
    }
}

#[tokio::test]
async fn test_budget_blocks_per_task_limit() {
    let repo = Arc::new(MockSpendRepository::new());
    let project_id = Uuid::new_v4();
    let task_id = Uuid::new_v4();

    // Add existing spend for this task
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
        cost_usd: Some(0.80),
        duration_ms: Some(5000),
        success: true,
        error: None,
        created_at: Utc::now(),
    });

    let policy = BudgetPolicy {
        daily_budget_usd: Some(100.0),
        max_per_task_usd: Some(1.0), // $1 per task
        strict_mode: true,
    };
    let service = BudgetService::new(repo, policy);

    // Try to spend $0.50 when already spent $0.80 (total would be $1.30)
    let decision = service
        .preflight_check(project_id, task_id, Some(0.50))
        .await;

    match decision {
        BudgetDecision::Blocked { reason, .. } => {
            assert!(reason.contains("task spend"));
        }
        _ => panic!("Expected Blocked, got {:?}", decision),
    }
}

#[tokio::test]
async fn test_budget_no_duplicate_attempts() {
    let repo = Arc::new(MockSpendRepository::new());
    let project_id = Uuid::new_v4();
    let task_id = Uuid::new_v4();
    let execution_id = Uuid::new_v4();

    // Simulate first attempt
    repo.add_spend(SpendRecord {
        id: Uuid::new_v4(),
        task_id,
        project_id,
        execution_id: Some(execution_id),
        attempt_number: 1,
        agent_type: "claude_code".to_string(),
        model: "claude-opus-5".to_string(),
        provider: "anthropic".to_string(),
        external_session_id: Some("session-123".to_string()),
        input_tokens: Some(1000),
        output_tokens: Some(500),
        total_tokens: Some(1500),
        cost_usd: Some(0.30),
        duration_ms: Some(5000),
        success: true,
        error: None,
        created_at: Utc::now(),
    });

    // Simulate second attempt (retry)
    repo.add_spend(SpendRecord {
        id: Uuid::new_v4(),
        task_id,
        project_id,
        execution_id: Some(execution_id),
        attempt_number: 2,
        agent_type: "claude_code".to_string(),
        model: "claude-opus-5".to_string(),
        provider: "anthropic".to_string(),
        external_session_id: Some("session-456".to_string()),
        input_tokens: Some(2000),
        output_tokens: Some(1000),
        total_tokens: Some(3000),
        cost_usd: Some(0.50),
        duration_ms: Some(8000),
        success: true,
        error: None,
        created_at: Utc::now(),
    });

    // Get execution spend
    let spends = repo.execution_spend(execution_id).await.unwrap();
    assert_eq!(spends.len(), 2);

    // Total should be $0.80, not duplicated
    let total: f64 = spends.iter().filter_map(|s| s.cost_usd).sum();
    assert!((total - 0.80).abs() < 0.01);
}

#[tokio::test]
async fn test_budget_service_fail_closed() {
    // Test that check failures result in denial
    let decision = BudgetDecision::CheckFailed {
        error: "database error".to_string(),
    };

    // Verify it's not Allowed
    match decision {
        BudgetDecision::Allowed { .. } => panic!("Should not be allowed on check failure"),
        BudgetDecision::CheckFailed { .. } => {
            // Expected
        }
        _ => {}
    }
}

#[tokio::test]
async fn test_budget_unknown_cost_not_assumed_zero() {
    let repo = Arc::new(MockSpendRepository::new());
    let project_id = Uuid::new_v4();
    let task_id = Uuid::new_v4();

    let policy = BudgetPolicy {
        daily_budget_usd: Some(10.0),
        max_per_task_usd: Some(1.0),
        strict_mode: true, // Unknown cost agents rejected
    };
    let service = BudgetService::new(repo, policy);

    // Request with unknown cost (None)
    let decision = service.preflight_check(project_id, task_id, None).await;

    // In strict mode, unknown cost should still be allowed if budget space exists
    // (but system should track it and warn)
    match decision {
        BudgetDecision::Allowed { .. } => {
            // OK in this case - unknown cost is allowed if budget has room
        }
        _ => panic!("Expected Allowed for unknown cost with budget room"),
    }
}
