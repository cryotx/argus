//! V0.38.1 — Runtime Stabilization tests.
//!
//! Covers spec items #14 (tight-loop) and #15 (deferred retry survives a
//! restart): a permanently-false admission-gate precondition must cost one
//! cheap state check per maintenance tick, never an unbounded claim/requeue
//! loop, and a deferred `retry_at` must be honored across a fresh
//! `TaskScheduler` instance backed by the same repository (i.e. a daemon
//! restart), not just in the live in-memory queue.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use uuid::Uuid;

use argus_core::accounting_state::RuntimeAccountingState;
use argus_core::budget_service::{BudgetPolicy, BudgetService};
use argus_core::execution_loop::{
    ExecutionLoop, ExecutionLoopConfig, ExecutionRequest, TaskExecutionResult, TaskExecutor,
};
use argus_core::spend_repository::{SpendRecord, SpendRepository};
use argus_core::task_repository::{CreateTaskResult, TaskRepository};
use argus_core::task_scheduler::TaskScheduler;
use argus_core::task_types::{Task, TaskBuilder, TaskState};
use argus_core::types::ArgusResult;

fn fast_config() -> ExecutionLoopConfig {
    ExecutionLoopConfig {
        idle_interval: Duration::from_millis(10),
        maintenance_interval: Duration::from_millis(50),
    }
}

fn success_result(task: &Task) -> TaskExecutionResult {
    TaskExecutionResult {
        task_id: task.id,
        success: true,
        exit_code: Some(0),
        stdout_summary: "ok".to_string(),
        stderr_summary: String::new(),
        started_at: Utc::now(),
        finished_at: Utc::now(),
        failure_reason: None,
        files_changed: Vec::new(),
        workspace: None,
        metadata: HashMap::new(),
        workspace_id: None,
        branch_name: None,
        base_revision: None,
        worktree_path: None,
    }
}

struct FnExecutor<F>
where
    F: Fn(&ExecutionRequest) -> TaskExecutionResult + Send + Sync,
{
    calls: Arc<AtomicUsize>,
    f: F,
}

#[async_trait]
impl<F> TaskExecutor for FnExecutor<F>
where
    F: Fn(&ExecutionRequest) -> TaskExecutionResult + Send + Sync,
{
    fn name(&self) -> &str {
        "fn-stub-executor"
    }

    async fn execute(&self, request: &ExecutionRequest) -> TaskExecutionResult {
        self.calls.fetch_add(1, Ordering::SeqCst);
        (self.f)(request)
    }
}

// ============================================================
// #14 — tight-loop: a permanently-false precondition must not hot-loop
// ============================================================

/// One project is permanently over its daily budget; the other is healthy.
/// Counts how many times each project's budget was actually checked, which
/// is exactly one claim/preflight cycle per call — the number a hot loop
/// would blow up and a correctly-deferred task would not.
struct PerProjectSpendRepo {
    always_blocked_project: Uuid,
    check_counts: Mutex<HashMap<Uuid, usize>>,
}

#[async_trait]
impl SpendRepository for PerProjectSpendRepo {
    async fn record_spend(&self, _record: SpendRecord) -> ArgusResult<bool> {
        Ok(true)
    }
    async fn spent_today_usd(&self, project_id: Uuid) -> ArgusResult<Option<f64>> {
        *self
            .check_counts
            .lock()
            .unwrap()
            .entry(project_id)
            .or_insert(0) += 1;
        if project_id == self.always_blocked_project {
            Ok(Some(1_000.0)) // permanently over the $1 limit below
        } else {
            Ok(Some(0.0))
        }
    }
    async fn spent_this_month_usd(&self, _project_id: Uuid) -> ArgusResult<Option<f64>> {
        Ok(Some(0.0))
    }
    async fn task_spend(&self, _task_id: Uuid) -> ArgusResult<Vec<SpendRecord>> {
        Ok(Vec::new())
    }
    async fn execution_spend(&self, _execution_id: Uuid) -> ArgusResult<Vec<SpendRecord>> {
        Ok(Vec::new())
    }
    async fn get_spend(&self, _spend_id: Uuid) -> ArgusResult<Option<SpendRecord>> {
        Ok(None)
    }
}

#[tokio::test]
async fn test_permanently_blocked_task_does_not_hot_loop() {
    let scheduler = Arc::new(TaskScheduler::new());

    let task_a = TaskBuilder::new(Uuid::new_v4(), "permanently budget-blocked").build();
    let task_a_id = task_a.id;
    let project_a = task_a.project_id;

    let task_b = TaskBuilder::new(Uuid::new_v4(), "healthy, should complete").build();
    let task_b_id = task_b.id;

    let spend_repo = Arc::new(PerProjectSpendRepo {
        always_blocked_project: project_a,
        check_counts: Mutex::new(HashMap::new()),
    });
    let budget_service = Arc::new(
        BudgetService::new(
            spend_repo.clone(),
            BudgetPolicy {
                daily_budget_usd: Some(1.0),
                max_per_task_usd: None,
                strict_mode: true,
            },
        )
        .with_accounting_state(RuntimeAccountingState::new()),
    );

    let calls = Arc::new(AtomicUsize::new(0));
    let executor = Arc::new(FnExecutor {
        calls: calls.clone(),
        f: |req: &ExecutionRequest| success_result(&req.task),
    });

    scheduler.add_task(task_a).await;
    scheduler.add_task(task_b).await;

    let handle = ExecutionLoop::new(Arc::clone(&scheduler), executor, fast_config())
        .with_budget_service(budget_service)
        .spawn();

    // Let the loop run for a while — at a 10ms idle interval, a genuine hot
    // loop on Task A would rack up on the order of 100+ claim/requeue
    // cycles in this window; a correctly-deferred task racks up ~1.
    tokio::time::sleep(Duration::from_millis(900)).await;

    // Task B must have executed normally and not been starved by Task A.
    assert_eq!(
        scheduler.get_task(task_b_id).await.map(|t| t.state),
        Some(TaskState::Completed),
        "an unrelated, schedulable task must not be blocked by a stuck one"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "Task B's executor ran exactly once"
    );

    // Task A must still be deferred, not failed, not spun through hundreds
    // of attempts.
    let task_a_final = scheduler.get_task(task_a_id).await.unwrap();
    assert_eq!(
        task_a_final.state,
        TaskState::Ready,
        "a budget-blocked task stays Ready-but-deferred, not Failed"
    );
    assert!(
        task_a_final.retry_at.is_some_and(|t| t > Utc::now()),
        "a permanently-blocked task must carry a future retry_at"
    );

    let project_a_checks = {
        let checks = spend_repo.check_counts.lock().unwrap();
        *checks.get(&project_a).unwrap_or(&0)
    };
    assert!(
        project_a_checks <= 3,
        "budget must be checked a handful of times, not hot-looped: got {project_a_checks} checks"
    );

    handle.shutdown().await;
}

// ============================================================
// #15 — deferred retry_at survives a restart
// ============================================================

/// Minimal `TaskRepository` that actually round-trips the full `Task`
/// (unlike some other test doubles in this suite that only track
/// `(id, state)`), because this test's whole point is that `retry_at`
/// itself survives being written and read back by a fresh scheduler.
#[derive(Default)]
struct FullFidelityTaskRepository {
    tasks: Mutex<HashMap<Uuid, Task>>,
}

#[async_trait]
impl TaskRepository for FullFidelityTaskRepository {
    async fn create_task(
        &self,
        task: &Task,
        _request_key: Option<&str>,
    ) -> ArgusResult<CreateTaskResult> {
        self.tasks.lock().unwrap().insert(task.id, task.clone());
        Ok(CreateTaskResult::Created(task.id))
    }

    async fn transition_task(
        &self,
        _expected_state: TaskState,
        task: &Task,
        _checkpoint_context: serde_json::Value,
    ) -> ArgusResult<bool> {
        use std::collections::hash_map::Entry;
        let mut tasks = self.tasks.lock().unwrap();
        match tasks.entry(task.id) {
            Entry::Occupied(mut e) => {
                e.insert(task.clone());
                Ok(true)
            }
            Entry::Vacant(_) => Ok(false),
        }
    }

    async fn load_tasks(&self) -> ArgusResult<Vec<Task>> {
        Ok(self.tasks.lock().unwrap().values().cloned().collect())
    }

    async fn load_active_tasks(&self) -> ArgusResult<Vec<Task>> {
        Ok(self
            .tasks
            .lock()
            .unwrap()
            .values()
            .filter(|t| !t.state.is_terminal())
            .cloned()
            .collect())
    }

    async fn find_task_id_by_request_key(&self, _request_key: &str) -> ArgusResult<Option<Uuid>> {
        Ok(None)
    }
}

#[tokio::test]
async fn test_deferred_task_survives_restart_without_tight_loop() {
    let repo = Arc::new(FullFidelityTaskRepository::default());

    // --- "Before restart": a task gets deferred by an admission gate. ---
    let scheduler_1 = TaskScheduler::new_with_repository(repo.clone(), None);
    let task = TaskBuilder::new(Uuid::new_v4(), "deferred across restart").build();
    let task_id = task.id;
    scheduler_1.add_task(task).await;

    // Simulate ExecutionLoop's real flow: dequeue (pops it off the live
    // queue) before deferring — deferring without dequeuing first would
    // leave a stale queue entry, which is not how `process_task` calls this.
    let claimed = scheduler_1.dequeue_task().await;
    assert_eq!(claimed, Some(task_id));

    let retry_at = Utc::now() + chrono::Duration::milliseconds(400);
    assert!(
        scheduler_1
            .defer_task(task_id, retry_at, "test_admission_gate_block")
            .await
            .unwrap(),
        "defer_task should succeed from Queued"
    );

    let deferred = scheduler_1.get_task(task_id).await.unwrap();
    assert_eq!(deferred.state, TaskState::Ready);
    assert_eq!(deferred.retry_at, Some(retry_at));

    // --- "Restart": a brand-new TaskScheduler over the same repository. ---
    let scheduler_2 = TaskScheduler::new_with_repository(repo.clone(), None);
    scheduler_2.restore().await.expect("restore should succeed");

    // The task must exist and still be deferred...
    let restored = scheduler_2.get_task(task_id).await.unwrap();
    assert_eq!(restored.state, TaskState::Ready);
    assert_eq!(
        restored.retry_at,
        Some(retry_at),
        "retry_at must survive the restart exactly"
    );

    // ...and, critically, must NOT be immediately schedulable: restore()
    // must not have put it back on the live queue just because it's Ready.
    assert_eq!(
        scheduler_2.dequeue_task().await,
        None,
        "a deferred task must not be dequeueable before its retry_at, even right after a restart"
    );

    // Once retry_at has passed, the next maintenance tick (not a busy poll
    // of the dequeue path) makes it schedulable again.
    tokio::time::sleep(Duration::from_millis(500)).await;
    scheduler_2.promote_ready_tasks().await;

    assert_eq!(
        scheduler_2.dequeue_task().await,
        Some(task_id),
        "once retry_at has passed, the task must become schedulable again"
    );
}
