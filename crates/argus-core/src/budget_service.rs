//! Budget enforcement service for Agent execution.
//!
//! Pre-flight budget checks before ANY paid Agent execution.
//! Fail-closed: if budget status cannot be determined, deny execution.

use std::sync::Arc;
use tracing::warn;
use uuid::Uuid;

use crate::accounting_state::RuntimeAccountingState;
use crate::spend_repository::SpendRepository;

/// Configuration for budget policy.
#[derive(Clone, Debug)]
pub struct BudgetPolicy {
    /// Global daily budget in USD (None = unlimited)
    pub daily_budget_usd: Option<f64>,
    /// Per-task maximum spend in USD (None = unlimited)
    pub max_per_task_usd: Option<f64>,
    /// Whether to enforce budget strictly (fail closed on unknown cost agents)
    pub strict_mode: bool,
}

impl Default for BudgetPolicy {
    fn default() -> Self {
        Self {
            daily_budget_usd: Some(10.0), // Conservative default
            max_per_task_usd: Some(1.0),
            strict_mode: true,
        }
    }
}

/// Decision made by budget preflight check.
#[derive(Clone, Debug, PartialEq)]
pub enum BudgetDecision {
    /// Execution is allowed within budget
    Allowed { remaining_today_usd: f64 },
    /// Execution is blocked
    Blocked {
        reason: String,
        spent_today_usd: Option<f64>,
        daily_limit_usd: Option<f64>,
    },
    /// Budget check failed (database error, etc.) - default deny
    CheckFailed { error: String },
}

/// Budget enforcement service.
pub struct BudgetService {
    spend_repo: Arc<dyn SpendRepository>,
    policy: BudgetPolicy,
    accounting_state: Option<RuntimeAccountingState>,
}

impl BudgetService {
    pub fn new(spend_repo: Arc<dyn SpendRepository>, policy: BudgetPolicy) -> Self {
        Self {
            spend_repo,
            policy,
            accounting_state: None,
        }
    }

    /// Attach an accounting state tracker to block executions when ledger is degraded.
    pub fn with_accounting_state(mut self, accounting_state: RuntimeAccountingState) -> Self {
        self.accounting_state = Some(accounting_state);
        self
    }

    /// Pre-flight budget check before executing a paid Agent.
    ///
    /// Returns BudgetDecision indicating whether execution is allowed.
    /// On any error, defaults to DENY (fail-closed).
    ///
    /// Also checks accounting state: if DEGRADED (from prior spend failures),
    /// blocks execution to prevent further damage.
    pub async fn preflight_check(
        &self,
        project_id: Uuid,
        task_id: Uuid,
        estimated_cost_usd: Option<f64>,
    ) -> BudgetDecision {
        // --- CHECK ACCOUNTING STATE FIRST ---
        // If spend ledger is degraded from prior failures, block all paid agents
        if let Some(accounting_state) = &self.accounting_state {
            if accounting_state.is_degraded().await {
                let state = accounting_state.get_state().await;
                return BudgetDecision::CheckFailed {
                    error: format!(
                        "accounting_degraded: {} (reason: {})",
                        state.status,
                        state.reason.as_deref().unwrap_or("unknown")
                    ),
                };
            }
        }

        // Check daily budget
        if let Some(limit) = self.policy.daily_budget_usd {
            match self.spend_repo.spent_today_usd(project_id).await {
                Ok(Some(spent)) => {
                    let remaining = limit - spent;
                    if remaining <= 0.0 {
                        return BudgetDecision::Blocked {
                            reason: format!(
                                "daily budget exhausted: ${:.2} spent of ${:.2} limit",
                                spent, limit
                            ),
                            spent_today_usd: Some(spent),
                            daily_limit_usd: Some(limit),
                        };
                    }

                    // If we have an estimate, check it won't exceed remaining
                    if let Some(est_cost) = estimated_cost_usd {
                        if est_cost > remaining {
                            return BudgetDecision::Blocked {
                                reason: format!(
                                    "estimated cost ${:.2} exceeds remaining budget ${:.2}",
                                    est_cost, remaining
                                ),
                                spent_today_usd: Some(spent),
                                daily_limit_usd: Some(limit),
                            };
                        }
                    }
                }
                Ok(None) => {
                    // No spend yet today, check limit is sufficient for estimate
                    if let Some(est_cost) = estimated_cost_usd {
                        if est_cost > limit {
                            return BudgetDecision::Blocked {
                                reason: format!(
                                    "estimated cost ${:.2} exceeds daily limit ${:.2}",
                                    est_cost, limit
                                ),
                                spent_today_usd: None,
                                daily_limit_usd: Some(limit),
                            };
                        }
                    }
                }
                Err(e) => {
                    warn!("budget check failed (daily): {}", e);
                    return BudgetDecision::CheckFailed {
                        error: format!("failed to check daily budget: {}", e),
                    };
                }
            }
        }

        // Check per-task budget
        if let Some(task_limit) = self.policy.max_per_task_usd {
            match self.spend_repo.task_spend(task_id).await {
                Ok(records) => {
                    let task_spent: f64 = records.iter().filter_map(|r| r.cost_usd).sum();

                    if task_spent >= task_limit {
                        return BudgetDecision::Blocked {
                            reason: format!(
                                "task budget exhausted: ${:.2} spent of ${:.2} limit",
                                task_spent, task_limit
                            ),
                            spent_today_usd: Some(task_spent),
                            daily_limit_usd: Some(task_limit),
                        };
                    }

                    if let Some(est_cost) = estimated_cost_usd {
                        if task_spent + est_cost > task_limit {
                            return BudgetDecision::Blocked {
                                reason: format!(
                                    "task spend ${:.2} + estimate ${:.2} exceeds limit ${:.2}",
                                    task_spent, est_cost, task_limit
                                ),
                                spent_today_usd: Some(task_spent),
                                daily_limit_usd: Some(task_limit),
                            };
                        }
                    }
                }
                Err(e) => {
                    warn!("budget check failed (per-task): {}", e);
                    return BudgetDecision::CheckFailed {
                        error: format!("failed to check per-task budget: {}", e),
                    };
                }
            }
        }

        // All checks passed
        let remaining = self.policy.daily_budget_usd.unwrap_or(f64::INFINITY)
            - self
                .spend_repo
                .spent_today_usd(project_id)
                .await
                .unwrap_or(None)
                .unwrap_or(0.0);

        BudgetDecision::Allowed {
            remaining_today_usd: remaining,
        }
    }

    /// Update the budget policy at runtime.
    pub fn update_policy(&mut self, policy: BudgetPolicy) {
        self.policy = policy;
    }

    /// Get current policy.
    pub fn policy(&self) -> &BudgetPolicy {
        &self.policy
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spend_repository::{SpendRecord, SpendRepository};
    use async_trait::async_trait;

    #[tokio::test]
    async fn test_budget_decision_format() {
        let decision = BudgetDecision::Allowed {
            remaining_today_usd: 5.5,
        };
        assert_eq!(
            decision,
            BudgetDecision::Allowed {
                remaining_today_usd: 5.5
            }
        );
    }

    /// Mock spend repository for testing.
    struct MockSpendRepository;

    #[async_trait]
    impl SpendRepository for MockSpendRepository {
        async fn record_spend(&self, _record: SpendRecord) -> crate::types::ArgusResult<bool> {
            Ok(true)
        }

        async fn spent_today_usd(
            &self,
            _project_id: Uuid,
        ) -> crate::types::ArgusResult<Option<f64>> {
            Ok(Some(0.0))
        }

        async fn spent_this_month_usd(
            &self,
            _project_id: Uuid,
        ) -> crate::types::ArgusResult<Option<f64>> {
            Ok(Some(0.0))
        }

        async fn task_spend(&self, _task_id: Uuid) -> crate::types::ArgusResult<Vec<SpendRecord>> {
            Ok(vec![])
        }

        async fn execution_spend(
            &self,
            _execution_id: Uuid,
        ) -> crate::types::ArgusResult<Vec<SpendRecord>> {
            Ok(vec![])
        }

        async fn get_spend(
            &self,
            _spend_id: Uuid,
        ) -> crate::types::ArgusResult<Option<SpendRecord>> {
            Ok(None)
        }
    }

    #[tokio::test]
    async fn test_preflight_blocks_when_accounting_degraded() {
        let mock_repo = Arc::new(MockSpendRepository);
        let policy = BudgetPolicy::default();
        let budget_service = BudgetService::new(mock_repo, policy);

        // Create accounting state and mark degraded
        let accounting_state = RuntimeAccountingState::new();
        accounting_state
            .mark_degraded("simulated spend failure")
            .await;

        // Attach to budget service
        let budget_service_with_accounting = budget_service.with_accounting_state(accounting_state);

        // Now preflight check should fail with accounting_degraded
        let decision = budget_service_with_accounting
            .preflight_check(Uuid::nil(), Uuid::nil(), Some(0.01))
            .await;

        match decision {
            BudgetDecision::CheckFailed { error } => {
                assert!(error.contains("accounting_degraded"));
                assert!(error.contains("simulated spend failure"));
            }
            _ => panic!(
                "Expected CheckFailed with accounting_degraded, got {:?}",
                decision
            ),
        }
    }

    #[tokio::test]
    async fn test_preflight_allows_when_accounting_healthy() {
        let mock_repo = Arc::new(MockSpendRepository);
        let policy = BudgetPolicy::default();
        let budget_service = BudgetService::new(mock_repo, policy);

        // Create accounting state (starts healthy)
        let accounting_state = RuntimeAccountingState::new();

        // Attach to budget service
        let budget_service_with_accounting = budget_service.with_accounting_state(accounting_state);

        // Preflight check should allow (no budget issues, accounting is healthy)
        let decision = budget_service_with_accounting
            .preflight_check(Uuid::nil(), Uuid::nil(), Some(0.01))
            .await;

        match decision {
            BudgetDecision::Allowed { .. } => {
                // Expected: accounting is healthy, budget allows
            }
            other => panic!(
                "Expected Allowed when accounting is healthy, got {:?}",
                other
            ),
        }
    }
}
