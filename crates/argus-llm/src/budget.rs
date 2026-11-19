//! Token budget tracker with multi-level scoping.
//!
//! Tracks spending at global, per-provider, per-project, and per-task levels.
//! Emits warning events when approaching budget limits.

use std::sync::Arc;

use dashmap::DashMap;
use uuid::Uuid;

use argus_core::event_bus::EventBus;
use argus_core::events::{ArgusEvent, EventSource};

use crate::types::{BudgetCheck, BudgetScope, BudgetState, UsageRecord};

// ============================================================
// Budget tracker
// ============================================================

/// Multi-level token budget tracker.
///
/// Thread-safe via `DashMap` — can be accessed from multiple tokio tasks
/// concurrently without external locking.
pub struct BudgetTracker {
    /// Global budget state.
    global: tokio::sync::RwLock<BudgetState>,
    /// Per-provider budget states.
    per_provider: DashMap<String, BudgetState>,
    /// Per-project budget states.
    per_project: DashMap<Uuid, BudgetState>,
    /// Per-task budget states.
    per_task: DashMap<Uuid, BudgetState>,
    /// Event bus for budget warnings.
    event_bus: Arc<EventBus>,
    /// Warning threshold (0.0 - 1.0), fires warning at this percentage.
    warning_threshold: f64,
}

impl BudgetTracker {
    /// Create a new budget tracker.
    pub fn new(
        monthly_limit: Option<f64>,
        daily_limit: Option<f64>,
        warning_threshold: f64,
        event_bus: Arc<EventBus>,
    ) -> Self {
        Self {
            global: tokio::sync::RwLock::new(BudgetState::new(monthly_limit, daily_limit)),
            per_provider: DashMap::new(),
            per_project: DashMap::new(),
            per_task: DashMap::new(),
            event_bus,
            warning_threshold,
        }
    }

    /// Set a provider's monthly budget.
    pub fn set_provider_budget(&self, provider: &str, monthly_limit: f64) {
        self.per_provider
            .entry(provider.to_string())
            .and_modify(|s| s.monthly_limit_usd = Some(monthly_limit))
            .or_insert_with(|| BudgetState::new(Some(monthly_limit), None));
    }

    /// Set a project's daily budget.
    pub fn set_project_budget(&self, project_id: Uuid, daily_limit: f64) {
        self.per_project
            .entry(project_id)
            .and_modify(|s| s.daily_limit_usd = Some(daily_limit))
            .or_insert_with(|| BudgetState::new(None, Some(daily_limit)));
    }

    /// Set a task's total budget.
    pub fn set_task_budget(&self, task_id: Uuid, total_limit: f64) {
        self.per_task
            .entry(task_id)
            .and_modify(|s| s.monthly_limit_usd = Some(total_limit))
            .or_insert_with(|| BudgetState::new(Some(total_limit), None));
    }

    /// Check if a request is within all applicable budgets.
    pub async fn check_budget(
        &self,
        provider: &str,
        project_id: Option<Uuid>,
        task_id: Option<Uuid>,
        estimated_cost: f64,
    ) -> BudgetCheck {
        // 1. Global monthly
        {
            let mut global = self.global.write().await;
            global.maybe_reset_monthly();
            global.maybe_reset_daily();

            if let Some(limit) = global.monthly_limit_usd {
                if global.month_spent_usd + estimated_cost > limit {
                    return BudgetCheck::OverBudget {
                        scope: BudgetScope::GlobalMonthly,
                        remaining_usd: limit - global.month_spent_usd,
                    };
                }
            }

            if let Some(limit) = global.daily_limit_usd {
                if global.today_spent_usd + estimated_cost > limit {
                    return BudgetCheck::OverBudget {
                        scope: BudgetScope::GlobalDaily,
                        remaining_usd: limit - global.today_spent_usd,
                    };
                }
            }
        }

        // 2. Provider monthly
        if let Some(mut state) = self.per_provider.get_mut(provider) {
            state.maybe_reset_monthly();
            if let Some(limit) = state.monthly_limit_usd {
                if state.month_spent_usd + estimated_cost > limit {
                    return BudgetCheck::OverBudget {
                        scope: BudgetScope::ProviderMonthly,
                        remaining_usd: limit - state.month_spent_usd,
                    };
                }
            }
        }

        // 3. Project daily
        if let Some(pid) = project_id {
            if let Some(mut state) = self.per_project.get_mut(&pid) {
                state.maybe_reset_daily();
                if let Some(limit) = state.daily_limit_usd {
                    if state.today_spent_usd + estimated_cost > limit {
                        return BudgetCheck::OverBudget {
                            scope: BudgetScope::ProjectDaily,
                            remaining_usd: limit - state.today_spent_usd,
                        };
                    }
                }
            }
        }

        // 4. Task total
        if let Some(tid) = task_id {
            if let Some(state) = self.per_task.get(&tid) {
                if let Some(limit) = state.monthly_limit_usd {
                    // For tasks, monthly_limit_usd is used as total limit
                    if state.month_spent_usd + estimated_cost > limit {
                        return BudgetCheck::OverBudget {
                            scope: BudgetScope::TaskTotal,
                            remaining_usd: limit - state.month_spent_usd,
                        };
                    }
                }
            }
        }

        BudgetCheck::WithinBudget
    }

    /// Record usage after a successful LLM call.
    pub async fn record_usage(&self, record: &UsageRecord) {
        let cost = record.cost_usd;
        let input = record.input_tokens;
        let output = record.output_tokens;

        // 1. Global
        {
            let mut global = self.global.write().await;
            global.record(cost, input, output);

            // Check warning thresholds
            if let Some(limit) = global.monthly_limit_usd {
                let pct = global.month_spent_usd / limit;
                if pct > self.warning_threshold {
                    self.event_bus.publish(
                        EventSource::LlmProvider,
                        ArgusEvent::LlmBudgetWarning {
                            provider: "global".to_string(),
                            remaining_percent: 1.0 - pct,
                            monthly_budget: limit,
                            current_usage: global.month_spent_usd,
                        },
                    );
                }
            }
        }

        // 2. Per-provider
        self.per_provider
            .entry(record.provider.clone())
            .and_modify(|s| s.record(cost, input, output))
            .or_insert_with(|| {
                let mut s = BudgetState::new(None, None);
                s.record(cost, input, output);
                s
            });

        // Check provider budget warning
        if let Some(state) = self.per_provider.get(&record.provider) {
            if let Some(limit) = state.monthly_limit_usd {
                let pct = state.month_spent_usd / limit;
                if pct > self.warning_threshold {
                    self.event_bus.publish(
                        EventSource::LlmProvider,
                        ArgusEvent::LlmBudgetWarning {
                            provider: record.provider.clone(),
                            remaining_percent: 1.0 - pct,
                            monthly_budget: limit,
                            current_usage: state.month_spent_usd,
                        },
                    );
                }
            }
        }

        // 3. Per-project
        if let Some(pid) = record.project_id {
            self.per_project
                .entry(pid)
                .and_modify(|s| s.record(cost, input, output))
                .or_insert_with(|| {
                    let mut s = BudgetState::new(None, None);
                    s.record(cost, input, output);
                    s
                });
        }

        // 4. Per-task
        if let Some(tid) = record.task_id {
            self.per_task
                .entry(tid)
                .and_modify(|s| s.record(cost, input, output))
                .or_insert_with(|| {
                    let mut s = BudgetState::new(None, None);
                    s.record(cost, input, output);
                    s
                });
        }
    }

    /// Remove task budget tracking when a task is complete.
    pub fn remove_task(&self, task_id: Uuid) {
        self.per_task.remove(&task_id);
    }

    /// Get global spending summary.
    pub async fn global_summary(&self) -> BudgetState {
        self.global.read().await.clone()
    }

    /// Get provider spending summary.
    pub fn provider_summary(&self, provider: &str) -> Option<BudgetState> {
        self.per_provider.get(provider).map(|s| s.clone())
    }

    /// Get project spending summary.
    pub fn project_summary(&self, project_id: Uuid) -> Option<BudgetState> {
        self.per_project.get(&project_id).map(|s| s.clone())
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ModelClass;
    use chrono::Utc;

    fn make_event_bus() -> Arc<EventBus> {
        Arc::new(EventBus::new())
    }

    fn make_usage(provider: &str, cost: f64) -> UsageRecord {
        UsageRecord {
            provider: provider.to_string(),
            model: "test-model".to_string(),
            model_class: ModelClass::Fast,
            input_tokens: 100,
            output_tokens: 50,
            cost_usd: cost,
            project_id: None,
            task_id: None,
            timestamp: Utc::now(),
            latency_ms: 100,
            purpose: "test".to_string(),
        }
    }

    #[tokio::test]
    async fn test_within_budget() {
        let tracker = BudgetTracker::new(Some(100.0), Some(10.0), 0.8, make_event_bus());
        let check = tracker.check_budget("openai", None, None, 1.0).await;
        assert!(matches!(check, BudgetCheck::WithinBudget));
    }

    #[tokio::test]
    async fn test_over_global_monthly_budget() {
        let tracker = BudgetTracker::new(Some(1.0), None, 0.8, make_event_bus());

        // Spend $0.90
        let record = make_usage("openai", 0.90);
        tracker.record_usage(&record).await;

        // $0.20 would exceed $1.00 limit
        let check = tracker.check_budget("openai", None, None, 0.20).await;
        match check {
            BudgetCheck::OverBudget { scope, .. } => {
                assert_eq!(scope, BudgetScope::GlobalMonthly);
            }
            _ => panic!("Expected OverBudget"),
        }
    }

    #[tokio::test]
    async fn test_over_provider_budget() {
        let tracker = BudgetTracker::new(Some(100.0), None, 0.8, make_event_bus());
        tracker.set_provider_budget("openai", 1.0);

        let record = make_usage("openai", 0.90);
        tracker.record_usage(&record).await;

        let check = tracker.check_budget("openai", None, None, 0.20).await;
        match check {
            BudgetCheck::OverBudget { scope, .. } => {
                assert_eq!(scope, BudgetScope::ProviderMonthly);
            }
            _ => panic!("Expected OverBudget"),
        }

        // Different provider should be fine
        let check2 = tracker.check_budget("anthropic", None, None, 0.20).await;
        assert!(matches!(check2, BudgetCheck::WithinBudget));
    }

    #[tokio::test]
    async fn test_over_task_budget() {
        let task_id = Uuid::new_v4();
        let tracker = BudgetTracker::new(Some(100.0), None, 0.8, make_event_bus());
        tracker.set_task_budget(task_id, 0.50);

        let mut record = make_usage("openai", 0.40);
        record.task_id = Some(task_id);
        tracker.record_usage(&record).await;

        let check = tracker
            .check_budget("openai", None, Some(task_id), 0.20)
            .await;
        match check {
            BudgetCheck::OverBudget { scope, .. } => {
                assert_eq!(scope, BudgetScope::TaskTotal);
            }
            _ => panic!("Expected OverBudget"),
        }
    }

    #[tokio::test]
    async fn test_record_usage_updates_all_levels() {
        let project_id = Uuid::new_v4();
        let task_id = Uuid::new_v4();
        let tracker = BudgetTracker::new(Some(100.0), None, 0.8, make_event_bus());

        let mut record = make_usage("openai", 0.50);
        record.project_id = Some(project_id);
        record.task_id = Some(task_id);
        tracker.record_usage(&record).await;

        let global = tracker.global_summary().await;
        assert!((global.month_spent_usd - 0.50).abs() < 0.001);
        assert_eq!(global.request_count, 1);

        let provider = tracker.provider_summary("openai").unwrap();
        assert!((provider.month_spent_usd - 0.50).abs() < 0.001);

        let project = tracker.project_summary(project_id).unwrap();
        assert!((project.today_spent_usd - 0.50).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_remove_task() {
        let task_id = Uuid::new_v4();
        let tracker = BudgetTracker::new(None, None, 0.8, make_event_bus());
        tracker.set_task_budget(task_id, 1.0);

        assert!(tracker.per_task.contains_key(&task_id));
        tracker.remove_task(task_id);
        assert!(!tracker.per_task.contains_key(&task_id));
    }

    #[tokio::test]
    async fn test_no_limits_always_within_budget() {
        let tracker = BudgetTracker::new(None, None, 0.8, make_event_bus());
        let check = tracker.check_budget("openai", None, None, 1000.0).await;
        assert!(matches!(check, BudgetCheck::WithinBudget));
    }
}
