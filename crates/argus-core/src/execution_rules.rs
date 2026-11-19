//! Strong execution rules for long-running project loops.
//!
//! This module enforces a bounded-iteration model:
//! - Project can be long-lived and iterated indefinitely.
//! - Each iteration is strictly bounded by task/time/cost budgets.
//! - Each task must be independently executable (clear DoD + test command).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::task_types::Task;
use crate::types::{ArgusError, ArgusResult};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IterationBudget {
    pub max_tasks: usize,
    pub max_minutes: u64,
    pub max_cost_usd: f64,
}

impl Default for IterationBudget {
    fn default() -> Self {
        Self {
            max_tasks: 20,
            max_minutes: 60,
            max_cost_usd: 3.0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecutionPolicy {
    pub budget: IterationBudget,
    pub retry_limit_per_task: u32,
    pub require_acceptance_criteria: bool,
    pub require_test_commands: bool,
    pub default_acceptance_criterion: String,
    pub default_test_command: String,
}

impl Default for ExecutionPolicy {
    fn default() -> Self {
        Self {
            budget: IterationBudget::default(),
            retry_limit_per_task: 2,
            require_acceptance_criteria: true,
            require_test_commands: true,
            default_acceptance_criterion: "Task acceptance criteria satisfied".to_string(),
            default_test_command: "cargo test".to_string(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    TaskLimitReached,
    TimeBudgetExceeded,
    CostBudgetExceeded,
}

#[derive(Clone, Debug)]
pub struct IterationContext {
    pub project_id: Uuid,
    pub iteration: u64,
    pub started_at: DateTime<Utc>,
    pub admitted_tasks: usize,
    pub completed_tasks: usize,
    pub failed_tasks: usize,
    pub consumed_minutes: u64,
    pub consumed_cost_usd: f64,
}

impl IterationContext {
    pub fn new(project_id: Uuid, iteration: u64) -> Self {
        Self {
            project_id,
            iteration,
            started_at: Utc::now(),
            admitted_tasks: 0,
            completed_tasks: 0,
            failed_tasks: 0,
            consumed_minutes: 0,
            consumed_cost_usd: 0.0,
        }
    }

    pub fn record_completed_task(&mut self, task: &Task, task_cost_usd: f64) {
        self.completed_tasks += 1;
        self.consumed_cost_usd += task_cost_usd.max(0.0);
        if let Some(actual) = task.actual_minutes {
            self.consumed_minutes = self.consumed_minutes.saturating_add(actual as u64);
        }
    }

    pub fn record_failed_task(&mut self, task: &Task, task_cost_usd: f64) {
        self.failed_tasks += 1;
        self.consumed_cost_usd += task_cost_usd.max(0.0);
        if let Some(actual) = task.actual_minutes {
            self.consumed_minutes = self.consumed_minutes.saturating_add(actual as u64);
        }
    }
}

#[derive(Clone, Debug)]
pub struct ExecutionRuleEngine {
    policy: ExecutionPolicy,
}

impl ExecutionRuleEngine {
    pub fn new(policy: ExecutionPolicy) -> Self {
        Self { policy }
    }

    pub fn policy(&self) -> &ExecutionPolicy {
        &self.policy
    }

    pub fn prepare_iteration_tasks(
        &self,
        tasks: Vec<Task>,
        ctx: &mut IterationContext,
    ) -> ArgusResult<Vec<Task>> {
        if tasks.is_empty() {
            return Err(ArgusError::Validation(
                "Iteration must contain at least one task".to_string(),
            ));
        }

        let mut prepared = Vec::new();
        for task in tasks.into_iter().take(self.policy.budget.max_tasks) {
            prepared.push(self.prepare_task(task, ctx.iteration)?);
        }
        ctx.admitted_tasks = prepared.len();

        if prepared.is_empty() {
            return Err(ArgusError::Validation(
                "No task admitted into iteration after rule checks".to_string(),
            ));
        }

        Ok(prepared)
    }

    pub fn prepare_task(&self, mut task: Task, iteration: u64) -> ArgusResult<Task> {
        if task.title.trim().is_empty() {
            return Err(ArgusError::Validation(
                "Task title cannot be empty".to_string(),
            ));
        }
        if task.description.trim().is_empty() {
            return Err(ArgusError::Validation(
                "Task description cannot be empty".to_string(),
            ));
        }

        if self.policy.require_acceptance_criteria && task.acceptance_criteria.is_empty() {
            task.acceptance_criteria
                .push(self.policy.default_acceptance_criterion.clone());
        }
        if self.policy.require_test_commands && task.test_commands.is_empty() {
            task.test_commands
                .push(self.policy.default_test_command.clone());
        }

        if task.max_attempts == 0 || task.max_attempts > self.policy.retry_limit_per_task {
            task.max_attempts = self.policy.retry_limit_per_task;
        }

        let iteration_tag = format!("iteration:{iteration}");
        if !task.tags.iter().any(|t| t == &iteration_tag) {
            task.tags.push(iteration_tag);
        }

        Ok(task)
    }

    pub fn should_stop_iteration(&self, ctx: &IterationContext) -> Option<StopReason> {
        if ctx.completed_tasks + ctx.failed_tasks >= self.policy.budget.max_tasks {
            return Some(StopReason::TaskLimitReached);
        }
        if ctx.consumed_minutes >= self.policy.budget.max_minutes {
            return Some(StopReason::TimeBudgetExceeded);
        }
        if self.policy.budget.max_cost_usd > 0.0
            && ctx.consumed_cost_usd >= self.policy.budget.max_cost_usd
        {
            return Some(StopReason::CostBudgetExceeded);
        }
        None
    }
}

impl Default for ExecutionRuleEngine {
    fn default() -> Self {
        Self::new(ExecutionPolicy::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task_types::TaskBuilder;

    #[test]
    fn test_prepare_task_injects_dod_and_test_defaults() {
        let project_id = Uuid::new_v4();
        let task = TaskBuilder::new(project_id, "Implement API")
            .description("Add endpoint")
            .build();

        let engine = ExecutionRuleEngine::default();
        let prepared = engine.prepare_task(task, 3).unwrap();

        assert_eq!(prepared.acceptance_criteria.len(), 1);
        assert_eq!(prepared.test_commands.len(), 1);
        assert!(prepared.tags.iter().any(|t| t == "iteration:3"));
        assert_eq!(prepared.max_attempts, 2);
    }

    #[test]
    fn test_prepare_iteration_respects_task_budget() {
        let project_id = Uuid::new_v4();
        let tasks = (0..5)
            .map(|i| {
                TaskBuilder::new(project_id, format!("Task {i}"))
                    .description("Work item")
                    .build()
            })
            .collect::<Vec<_>>();

        let mut policy = ExecutionPolicy::default();
        policy.budget.max_tasks = 2;
        let engine = ExecutionRuleEngine::new(policy);

        let mut ctx = IterationContext::new(project_id, 1);
        let admitted = engine.prepare_iteration_tasks(tasks, &mut ctx).unwrap();

        assert_eq!(admitted.len(), 2);
        assert_eq!(ctx.admitted_tasks, 2);
    }

    #[test]
    fn test_stop_reason_task_limit() {
        let project_id = Uuid::new_v4();
        let mut policy = ExecutionPolicy::default();
        policy.budget.max_tasks = 1;
        let engine = ExecutionRuleEngine::new(policy);

        let mut ctx = IterationContext::new(project_id, 1);
        ctx.completed_tasks = 1;
        assert_eq!(
            engine.should_stop_iteration(&ctx),
            Some(StopReason::TaskLimitReached)
        );
    }
}
