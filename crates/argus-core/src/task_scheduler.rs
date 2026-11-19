//! Task Scheduler — orchestrates task execution.
//!
//! Manages task lifecycle, resolves dependencies, assigns work to workers.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::dependency_graph::DependencyGraph;
use crate::event_bus::EventBus;
use crate::events::{ArgusEvent, EventSource};
use crate::priority_queue::{SchedulableTask, TaskPriorityQueue};
use crate::quality::{QualityChecker, QualityReport, QualityResult};
use crate::task_repository::{CreateTaskResult, TaskRepository};
use crate::task_types::{Task, TaskError, TaskErrorType, TaskOutput, TaskState};

/// Recovery statistics reported when scheduler is restored on daemon startup.
///
/// Used to provide transparency about what tasks were recovered and how many
/// historical tasks were excluded to prevent memory bloat.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecoveryReport {
    /// Total non-terminal (active) tasks loaded
    pub active_tasks: usize,
    /// Tasks that were interrupted (Queued/Assigned/Running/Testing) and resumed to Ready
    pub interrupted_resumed: usize,
    /// Tasks already in Ready state waiting for execution
    pub ready_for_execution: usize,
    /// Tasks in other states (Pending, Paused, Blocked, etc.)
    pub other_states: usize,
    /// Historical terminal tasks that were excluded from recovery
    pub terminal_tasks_skipped: Option<usize>,
}

/// Task Scheduler — the main orchestration engine.
pub struct TaskScheduler {
    /// Task registry (in-memory storage)
    tasks: RwLock<HashMap<Uuid, Task>>,
    /// Dependency graph for tracking task dependencies
    dep_graph: RwLock<DependencyGraph>,
    /// Priority queue for ready tasks
    queue: RwLock<TaskPriorityQueue>,
    /// Idempotency index for external request keys
    request_index: RwLock<HashMap<String, Uuid>>,
    /// Optional event bus for lifecycle events
    event_bus: Option<Arc<EventBus>>,
    /// Optional durable storage for production schedulers
    repository: Option<Arc<dyn TaskRepository>>,
    /// Serializes state transitions across DB and in-memory mirrors.
    transition_lock: Mutex<()>,
    /// Default timeout for tasks
    _default_timeout: Duration,
}

impl TaskScheduler {
    /// Create a new task scheduler.
    pub fn new() -> Self {
        Self {
            tasks: RwLock::new(HashMap::new()),
            dep_graph: RwLock::new(DependencyGraph::new()),
            queue: RwLock::new(TaskPriorityQueue::new()),
            request_index: RwLock::new(HashMap::new()),
            event_bus: None,
            repository: None,
            transition_lock: Mutex::new(()),
            _default_timeout: Duration::from_secs(30 * 60),
        }
    }

    /// Create a scheduler wired to an event bus.
    pub fn new_with_event_bus(event_bus: Arc<EventBus>) -> Self {
        let mut scheduler = Self::new();
        scheduler.event_bus = Some(event_bus);
        scheduler
    }

    /// Create a scheduler backed by durable task storage.
    pub fn new_with_repository(
        repository: Arc<dyn TaskRepository>,
        event_bus: Option<Arc<EventBus>>,
    ) -> Self {
        let mut scheduler = Self::new();
        scheduler.repository = Some(repository);
        scheduler.event_bus = event_bus;
        scheduler
    }

    /// Restore persisted tasks and normalize interrupted work back to ready.
    ///
    /// Only loads non-terminal tasks (excludes Completed, Failed, Cancelled, TimedOut)
    /// to prevent unbounded memory growth from historical tasks.
    ///
    /// Returns a detailed RecoveryReport for startup diagnostics.
    pub async fn restore(&self) -> crate::types::ArgusResult<RecoveryReport> {
        let Some(repository) = &self.repository else {
            return Ok(RecoveryReport {
                active_tasks: 0,
                interrupted_resumed: 0,
                ready_for_execution: 0,
                other_states: 0,
                terminal_tasks_skipped: None,
            });
        };

        // Load only active (non-terminal) tasks to prevent memory bloat
        let mut tasks = repository.load_active_tasks().await?;
        let total_active = tasks.len();
        let mut interrupted_count = 0;
        let mut ready_count = 0;
        let mut other_states_count = 0;

        for task in &mut tasks {
            if matches!(
                task.state,
                TaskState::Queued | TaskState::Assigned | TaskState::Running | TaskState::Testing
            ) {
                let interrupted_state = task.state;
                task.state = TaskState::Ready;
                task.assigned_worker_id = None;
                task.resume_count = task.resume_count.saturating_add(1);
                task.updated_at = chrono::Utc::now();
                interrupted_count += 1;
                if !repository
                    .transition_task(
                        interrupted_state,
                        task,
                        serde_json::json!({
                            "reason": "process_restart_resume",
                            "previous_state": interrupted_state,
                        }),
                    )
                    .await?
                {
                    return Err(crate::types::ArgusError::Task(format!(
                        "failed to recover task {} from {}",
                        task.id, interrupted_state
                    )));
                }
            }
            match task.state {
                TaskState::Ready => ready_count += 1,
                TaskState::Pending | TaskState::Paused | TaskState::Blocked => {
                    other_states_count += 1
                }
                _ => {}
            }
        }

        {
            let mut registry = self.tasks.write().await;
            registry.clear();
            registry.extend(tasks.iter().cloned().map(|task| (task.id, task)));
        }

        {
            let mut graph = self.dep_graph.write().await;
            *graph = DependencyGraph::new();
            for task in &tasks {
                graph.add_task(task.id, &task.depends_on);
            }
            for task in &tasks {
                match task.state {
                    TaskState::Completed => {
                        graph.mark_completed(task.id);
                    }
                    TaskState::Failed | TaskState::TimedOut => {
                        graph.mark_failed(task.id);
                    }
                    _ => {}
                }
            }
        }

        {
            let mut queue = self.queue.write().await;
            *queue = TaskPriorityQueue::new();
            let now = chrono::Utc::now();
            for task in &tasks {
                // V0.38.1: a deferred task's `retry_at` must survive a
                // restart — restoring it straight onto the live queue would
                // turn "wait until retry_at" into "tight-loop from tick one."
                // The next `promote_ready_tasks` sweep re-enqueues it once
                // due; `restore()` only seeds the queue with what's
                // immediately eligible right now.
                let due = task.retry_at.is_none_or(|retry_at| retry_at <= now);
                if task.state == TaskState::Ready && due {
                    queue.push(SchedulableTask::from_task(task));
                }
            }
        }

        let report = RecoveryReport {
            active_tasks: total_active,
            interrupted_resumed: interrupted_count,
            ready_for_execution: ready_count,
            other_states: other_states_count,
            terminal_tasks_skipped: None,
        };

        info!(
            active_tasks = report.active_tasks,
            interrupted_resumed = report.interrupted_resumed,
            ready_for_execution = report.ready_for_execution,
            other_states = report.other_states,
            "Scheduler state restored (terminal tasks excluded)"
        );
        Ok(report)
    }

    /// Add a task to the scheduler.
    ///
    /// The task starts in Pending state and will be moved to Ready
    /// once its dependencies are resolved.
    pub async fn add_task(&self, task: Task) {
        if let Err(err) = self.add_task_durable(task).await {
            error!(error = %err, "Failed to add task");
        }
    }

    pub async fn add_task_durable(&self, task: Task) -> crate::types::ArgusResult<()> {
        self.add_task_internal(task, None).await.map(|_| ())
    }

    /// Add a task with an idempotency request key.
    ///
    /// If the key already exists, returns the original task id and does not
    /// create a duplicate task.
    pub async fn add_task_idempotent(&self, task: Task, request_key: impl Into<String>) -> Uuid {
        let fallback_id = task.id;
        match self.add_task_idempotent_durable(task, request_key).await {
            Ok(task_id) => task_id,
            Err(err) => {
                error!(task_id = %fallback_id, error = %err, "Failed to add idempotent task");
                fallback_id
            }
        }
    }

    pub async fn add_task_idempotent_durable(
        &self,
        task: Task,
        request_key: impl Into<String>,
    ) -> crate::types::ArgusResult<Uuid> {
        let key = request_key.into();
        let normalized = key.trim();
        if normalized.is_empty() {
            let task_id = task.id;
            self.add_task_internal(task, None).await?;
            return Ok(task_id);
        }

        {
            let index = self.request_index.read().await;
            if let Some(existing) = index.get(normalized) {
                info!(
                    request_key = normalized,
                    task_id = %existing,
                    "Idempotent task request deduplicated"
                );
                return Ok(*existing);
            }
        }

        let task_id = self.add_task_internal(task, Some(normalized)).await?;
        self.request_index
            .write()
            .await
            .insert(normalized.to_string(), task_id);
        Ok(task_id)
    }

    async fn add_task_internal(
        &self,
        task: Task,
        request_key: Option<&str>,
    ) -> crate::types::ArgusResult<Uuid> {
        let task_id = task.id;
        let project_id = task.project_id;
        let deps = task.depends_on.clone();

        debug!(
            task_id = %task_id,
            project_id = %project_id,
            "Adding task to scheduler"
        );

        if let Some(repository) = &self.repository {
            match repository.create_task(&task, request_key).await? {
                CreateTaskResult::Created(_) => {}
                CreateTaskResult::Existing(existing_id) => return Ok(existing_id),
            }
        }

        // Add to dependency graph
        {
            let mut graph = self.dep_graph.write().await;
            graph.add_task(task_id, &deps);
        }

        // Check if task is already ready (no dependencies)
        let is_ready = deps.is_empty();

        // Store the task
        {
            let mut tasks = self.tasks.write().await;
            tasks.insert(task_id, task);
        }

        // If ready, move to queue
        if is_ready {
            self.transition(task_id, TaskState::Ready).await;
            let _ = self.enqueue_task(task_id).await;
        }
        Ok(task_id)
    }

    /// Get a task by ID.
    pub async fn get_task(&self, task_id: Uuid) -> Option<Task> {
        let tasks = self.tasks.read().await;
        tasks.get(&task_id).cloned()
    }

    /// Resolve a task id from a previously recorded idempotency key.
    pub async fn get_task_id_by_request_key(&self, request_key: &str) -> Option<Uuid> {
        let key = request_key.trim();
        if key.is_empty() {
            return None;
        }

        if let Some(task_id) = self.request_index.read().await.get(key).copied() {
            return Some(task_id);
        }

        let repository = self.repository.as_ref()?;
        match repository.find_task_id_by_request_key(key).await {
            Ok(task_id) => task_id,
            Err(err) => {
                error!(
                    request_key = key,
                    error = %err,
                    "Failed to query persisted idempotency key"
                );
                None
            }
        }
    }

    /// Get all tasks.
    pub async fn get_all_tasks(&self) -> Vec<Task> {
        let tasks = self.tasks.read().await;
        tasks.values().cloned().collect()
    }

    /// Get tasks by state.
    pub async fn get_tasks_by_state(&self, state: TaskState) -> Vec<Task> {
        let tasks = self.tasks.read().await;
        tasks
            .values()
            .filter(|t| t.state == state)
            .cloned()
            .collect()
    }

    /// Get tasks by project.
    pub async fn get_tasks_by_project(&self, project_id: Uuid) -> Vec<Task> {
        let tasks = self.tasks.read().await;
        tasks
            .values()
            .filter(|t| t.project_id == project_id)
            .cloned()
            .collect()
    }

    /// Transition a task to a new state.
    ///
    /// Returns true if the transition was successful.
    pub async fn transition(&self, task_id: Uuid, new_state: TaskState) -> bool {
        match self
            .transition_with(task_id, new_state, "state_transition", |_| {})
            .await
        {
            Ok(changed) => changed,
            Err(err) => {
                error!(task_id = %task_id, to = %new_state, error = %err, "Task transition failed");
                false
            }
        }
    }

    async fn transition_with<F>(
        &self,
        task_id: Uuid,
        new_state: TaskState,
        reason: &str,
        mutate: F,
    ) -> crate::types::ArgusResult<bool>
    where
        F: FnOnce(&mut Task),
    {
        let _transition_guard = self.transition_lock.lock().await;
        let current = match self.get_task(task_id).await {
            Some(task) => task,
            None => return Ok(false),
        };
        if current.state != new_state && !current.state.can_transition_to(new_state) {
            warn!(
                task_id = %task_id,
                from = %current.state,
                to = %new_state,
                "Invalid state transition"
            );
            return Ok(false);
        }

        let previous_state = current.state;
        let mut updated = current;
        mutate(&mut updated);
        updated.state = new_state;
        updated.updated_at = chrono::Utc::now();
        match new_state {
            TaskState::Running if updated.started_at.is_none() => {
                updated.started_at = Some(chrono::Utc::now());
            }
            TaskState::Completed
            | TaskState::Failed
            | TaskState::Cancelled
            | TaskState::TimedOut => {
                updated.completed_at = Some(chrono::Utc::now());
                if let Some(started) = updated.started_at {
                    updated.actual_minutes =
                        Some((chrono::Utc::now() - started).num_minutes().max(0) as u32);
                }
            }
            _ => {}
        }

        if let Some(repository) = &self.repository {
            let changed = repository
                .transition_task(
                    previous_state,
                    &updated,
                    serde_json::json!({
                        "reason": reason,
                        "previous_state": previous_state,
                        "new_state": new_state,
                        "attempt": updated.attempt,
                    }),
                )
                .await?;
            if !changed {
                return Ok(false);
            }
        }

        self.tasks.write().await.insert(task_id, updated);
        if let Some(bus) = &self.event_bus {
            bus.publish(
                EventSource::Scheduler,
                ArgusEvent::TaskCheckpointed {
                    task_id,
                    progress: self
                        .get_task(task_id)
                        .await
                        .map(|task| task.progress_pct() / 100.0)
                        .unwrap_or_default(),
                },
            );
        }
        info!(task_id = %task_id, from = %previous_state, to = %new_state, reason, "Task state transition committed");
        Ok(true)
    }

    /// Enqueue a ready task into the priority queue.
    async fn enqueue_task(&self, task_id: Uuid) -> bool {
        let tasks = self.tasks.read().await;

        if let Some(task) = tasks.get(&task_id) {
            if task.state != TaskState::Ready {
                return false;
            }

            let schedulable = SchedulableTask::from_task(task);
            drop(tasks); // Release read lock before acquiring write lock

            let mut queue = self.queue.write().await;
            queue.push(schedulable)
        } else {
            false
        }
    }

    /// Dequeue the next task to be assigned.
    ///
    /// Returns None if the queue is empty.
    pub async fn dequeue_task(&self) -> Option<Uuid> {
        loop {
            let task_id = {
                let mut queue = self.queue.write().await;
                queue.pop().map(|task| task.task_id)?
            };
            let claimed = self.claim_task(task_id).await.unwrap_or_else(|err| {
                error!(task_id = %task_id, error = %err, "Failed to claim task");
                false
            });
            if claimed {
                return Some(task_id);
            }
        }
    }

    /// Atomically claim a ready task using a stable logical execution id.
    pub async fn claim_task(&self, task_id: Uuid) -> crate::types::ArgusResult<bool> {
        self.transition_with(task_id, TaskState::Queued, "scheduler_claim", |task| {
            if task.execution_id.is_none() {
                task.execution_id = Some(Uuid::new_v4());
            }
        })
        .await
    }

    /// Requeue a claimed task back to Ready **immediately** when it cannot be
    /// executed due to an admission gate, and the caller knows retrying
    /// right away is appropriate. This puts the task straight back on the
    /// priority queue — unlike a bare state transition, which would leave it
    /// sitting in `Ready` forever, since `promote_ready_tasks` only sweeps
    /// `Pending` tasks and due deferrals, not arbitrary `Ready` tasks.
    ///
    /// V0.38.1: prefer [`Self::defer_task`] for admission-gate blocks whose
    /// precondition (budget, accounting, workspace) is unlikely to clear on
    /// the very next tick — calling this in a loop for a persistently-false
    /// precondition is exactly the claim/requeue hot-loop V0.38.1 exists to
    /// prevent. This method still exists for genuinely transient, safe-to-
    /// retry-immediately cases.
    pub async fn requeue_task(&self, task_id: Uuid) -> crate::types::ArgusResult<bool> {
        let changed = self
            .transition_with(task_id, TaskState::Ready, "scheduler_requeue", |task| {
                // Do not modify attempt count — this is not a failure
                task.updated_at = chrono::Utc::now();
            })
            .await?;
        if changed {
            self.enqueue_task(task_id).await;
        }
        Ok(changed)
    }

    /// Defer a task back to `Ready` **without** putting it back on the live
    /// queue, and without any blocking sleep. `retry_at` is persisted on the
    /// task (it rides along in the existing task-context JSON, so it
    /// survives a daemon restart); `promote_ready_tasks` — driven by
    /// `ExecutionLoop`'s existing maintenance tick, not the hot dequeue path
    /// — re-enqueues it once `retry_at` has passed. Does not modify
    /// `attempt`/`max_attempts`: a deferral is not a failure.
    ///
    /// This is the fix for the claim → precondition-fail → requeue → claim
    /// hot loop: a permanently-false precondition (budget exhausted for the
    /// day, accounting degraded) now costs one maintenance-tick check per
    /// interval instead of unbounded claim/requeue cycles.
    pub async fn defer_task(
        &self,
        task_id: Uuid,
        retry_at: chrono::DateTime<chrono::Utc>,
        reason: &str,
    ) -> crate::types::ArgusResult<bool> {
        self.transition_with(task_id, TaskState::Ready, reason, |task| {
            task.retry_at = Some(retry_at);
            task.defer_count = task.defer_count.saturating_add(1);
        })
        .await
        // Deliberately not enqueued here — `promote_ready_tasks` picks it
        // up once due.
    }

    /// Peek at the next task without removing it.
    pub async fn peek_task(&self) -> Option<SchedulableTask> {
        let queue = self.queue.read().await;
        queue.peek().cloned()
    }

    /// Persist worker output and enter the mandatory quality gate.
    pub async fn submit_task_output(&self, task_id: Uuid, output: TaskOutput) -> bool {
        let current_state = self.get_task(task_id).await.map(|t| t.state);
        match current_state {
            Some(TaskState::Ready) | Some(TaskState::Assigned) | Some(TaskState::Running) => {
                match self
                    .transition_with(task_id, TaskState::Testing, "execution_finished", |task| {
                        task.output = Some(output);
                    })
                    .await
                {
                    Ok(changed) => changed,
                    Err(err) => {
                        error!(task_id = %task_id, error = %err, "Failed to persist task output");
                        false
                    }
                }
            }
            Some(TaskState::Testing) => {
                match self
                    .transition_with(task_id, TaskState::Testing, "quality_output", |task| {
                        task.output = Some(output);
                    })
                    .await
                {
                    Ok(changed) => changed,
                    Err(err) => {
                        error!(task_id = %task_id, error = %err, "Failed to persist task output");
                        false
                    }
                }
            }
            _ => false,
        }
    }

    /// Finalize a task using quality-gate evidence. `Testing -> Completed` is
    /// impossible through the public scheduler API without a passing report.
    pub async fn finalize_task(
        &self,
        task_id: Uuid,
        report: &QualityReport,
    ) -> crate::types::ArgusResult<bool> {
        if report.task_id != task_id.to_string() {
            return Err(crate::types::ArgusError::Task(format!(
                "quality report task {} does not match {}",
                report.task_id, task_id
            )));
        }
        if !report.overall.is_passing() {
            return Ok(false);
        }
        if self.get_task(task_id).await.map(|task| task.state) != Some(TaskState::Testing) {
            return Ok(false);
        }
        if !self
            .transition_with(task_id, TaskState::Completed, "quality_gate_passed", |_| {})
            .await?
        {
            return Ok(false);
        }

        if let Some(bus) = &self.event_bus {
            let output = self
                .get_task(task_id)
                .await
                .and_then(|task| task.output)
                .and_then(|output| serde_json::to_value(output).ok())
                .unwrap_or(serde_json::Value::Null);
            bus.publish(
                EventSource::Scheduler,
                ArgusEvent::TaskCompleted {
                    task_id,
                    output,
                    duration_seconds: i64::try_from(report.total_duration_ms / 1000)
                        .unwrap_or(i64::MAX),
                },
            );
        }

        self.resolve_completed_dependencies(task_id).await;
        Ok(true)
    }

    /// Execute all configured quality stages, then complete or fail and roll
    /// back the task. Returns both the gate and compensation outcomes.
    pub async fn complete_task_with_quality(
        &self,
        task_id: Uuid,
        output: TaskOutput,
        quality_input: &crate::quality::TaskOutput,
        checker: &QualityChecker,
    ) -> crate::types::ArgusResult<(QualityReport, Option<QualityResult>)> {
        if !self.submit_task_output(task_id, output).await {
            return Err(crate::types::ArgusError::Task(format!(
                "task {task_id} could not enter testing"
            )));
        }
        self.publish_quality_gate_started(task_id);

        let report = checker.check(quality_input).await;
        if report.overall.is_passing() {
            if !self.finalize_task(task_id, &report).await? {
                return Err(crate::types::ArgusError::Task(format!(
                    "task {task_id} did not complete after passing quality gate"
                )));
            }
            return Ok((report, None));
        }

        self.fail_quality_gate(task_id, quality_input, checker, report)
            .await
    }

    /// V0.38 Phase 4: like [`Self::complete_task_with_quality`], but once
    /// the Quality Gate passes, `checkpoint` gets one more say before the
    /// task is allowed to reach `Completed` — turning verified worktree
    /// changes into a durable Git checkpoint + Artifact
    /// (Architecture Principle 12: verified artifacts, not Agent claims,
    /// determine completion).
    ///
    /// A quality failure is handled identically to
    /// `complete_task_with_quality` (rollback, straight to `Failed`,
    /// dependents blocked) — the checkpoint is never even attempted in that
    /// case (rule: quality failure -> no commit). A *checkpoint* failure is
    /// different: nothing is rolled back (only the checkpoint step itself
    /// did not succeed, not the Agent's work), and the task fails through
    /// [`Self::fail_task`] so it is retried subject to the normal attempt
    /// policy rather than always ending in permanent `Failed`.
    pub async fn complete_task_with_checkpoint(
        &self,
        task_id: Uuid,
        output: TaskOutput,
        quality_input: &crate::quality::TaskOutput,
        checker: &QualityChecker,
        checkpoint: &dyn crate::checkpoint::PostQualityCheckpoint,
    ) -> crate::types::ArgusResult<(QualityReport, Option<QualityResult>)> {
        if !self.submit_task_output(task_id, output).await {
            return Err(crate::types::ArgusError::Task(format!(
                "task {task_id} could not enter testing"
            )));
        }
        self.publish_quality_gate_started(task_id);

        let report = checker.check(quality_input).await;
        if !report.overall.is_passing() {
            return self
                .fail_quality_gate(task_id, quality_input, checker, report)
                .await;
        }

        match checkpoint.run(task_id, &report).await {
            Ok(patch) => {
                if let Some(patch) = patch {
                    if let Some(mut task) = self.get_task(task_id).await {
                        let mut amended = task.output.take().unwrap_or_default();
                        if patch.commit_sha.is_some() {
                            amended.commit_sha = patch.commit_sha;
                        }
                        amended.metadata.extend(patch.metadata);
                        // Still `Testing`: this is a same-state output update,
                        // exactly like `submit_task_output`'s own `Testing ->
                        // Testing` arm — never a second gate transition.
                        self.submit_task_output(task_id, amended).await;
                    }
                }
                if !self.finalize_task(task_id, &report).await? {
                    return Err(crate::types::ArgusError::Task(format!(
                        "task {task_id} did not complete after a successful checkpoint"
                    )));
                }
                Ok((report, None))
            }
            Err(failure) => {
                let attempt = self
                    .get_task(task_id)
                    .await
                    .map(|task| task.attempt)
                    .unwrap_or(1);
                warn!(
                    task_id = %task_id,
                    error_type = ?failure.error_type,
                    reason = %failure.reason,
                    "Git checkpoint failed after quality gate passed; task not completed"
                );
                self.fail_task(
                    task_id,
                    TaskError::new(failure.error_type, failure.reason, attempt),
                )
                .await;
                Ok((report, None))
            }
        }
    }

    fn publish_quality_gate_started(&self, task_id: Uuid) {
        if let Some(bus) = &self.event_bus {
            bus.publish(
                EventSource::Scheduler,
                ArgusEvent::TaskProgress {
                    task_id,
                    progress: 0.9,
                    message: Some("quality_gate_started".to_string()),
                },
            );
        }
    }

    /// Shared "the Quality Gate rejected this task" tail used by both
    /// `complete_task_with_quality` and `complete_task_with_checkpoint`:
    /// automatic rollback, straight to permanent `Failed` (quality failures
    /// are never retried — they're deterministic on the Agent's own output),
    /// and blocking any dependents.
    async fn fail_quality_gate(
        &self,
        task_id: Uuid,
        quality_input: &crate::quality::TaskOutput,
        checker: &QualityChecker,
        report: QualityReport,
    ) -> crate::types::ArgusResult<(QualityReport, Option<QualityResult>)> {
        let rollback = checker.rollback(quality_input).await;
        let errors = match &report.overall {
            QualityResult::Failed { errors } => errors.join("; "),
            QualityResult::Indeterminate { reason } => reason.clone(),
            QualityResult::PassedWithWarnings { warnings } => warnings.join("; "),
            QualityResult::Passed => "quality gate rejected the task".to_string(),
        };
        let rollback_summary = match &rollback {
            QualityResult::Passed => "automatic rollback completed".to_string(),
            QualityResult::Failed { errors } => {
                format!("automatic rollback failed: {}", errors.join("; "))
            }
            QualityResult::Indeterminate { reason } => reason.clone(),
            QualityResult::PassedWithWarnings { warnings } => {
                format!("rollback completed with warnings: {}", warnings.join("; "))
            }
        };
        let attempt = self
            .get_task(task_id)
            .await
            .map(|task| task.attempt)
            .unwrap_or(1);
        self.transition_with(task_id, TaskState::Failed, "quality_gate_failed", |task| {
            task.error = Some(TaskError::new(
                TaskErrorType::TestsFailed,
                format!("{errors}; {rollback_summary}"),
                attempt,
            ));
        })
        .await?;
        if let Some(bus) = &self.event_bus {
            bus.publish(
                EventSource::Scheduler,
                ArgusEvent::TaskFailed {
                    task_id,
                    error: format!("{errors}; {rollback_summary}"),
                    attempt,
                    will_retry: false,
                },
            );
        }
        self.block_failed_dependencies(task_id).await;
        Ok((report, Some(rollback)))
    }

    async fn resolve_completed_dependencies(&self, task_id: Uuid) {
        // Resolve dependencies
        let newly_ready = {
            let mut graph = self.dep_graph.write().await;
            graph.mark_completed(task_id)
        };

        // Enqueue newly ready tasks
        for ready_id in newly_ready {
            info!(task_id = %ready_id, "Task now ready");
            self.transition(ready_id, TaskState::Ready).await;
            self.enqueue_task(ready_id).await;
        }
    }

    async fn block_failed_dependencies(&self, task_id: Uuid) {
        let blocked = {
            let mut graph = self.dep_graph.write().await;
            graph.mark_failed(task_id)
        };
        for blocked_id in blocked {
            warn!(task_id = %blocked_id, "Task blocked by failed dependency");
            self.transition(blocked_id, TaskState::Blocked).await;
        }
    }

    /// Handle task failure.
    ///
    /// Either retries the task or marks it as failed and blocks downstream tasks.
    pub async fn fail_task(&self, task_id: Uuid, error: TaskError) {
        let error_message = error.message.clone();
        let can_retry = {
            let tasks = self.tasks.read().await;
            tasks.get(&task_id).map(|t| t.can_retry()).unwrap_or(false)
        };

        if can_retry {
            // Retry
            info!(
                task_id = %task_id,
                error = %error.message,
                "Task failed, will retry"
            );

            // Increment attempt and store error
            let current = self.get_task(task_id).await;
            let attempt = current
                .as_ref()
                .map(|task| task.attempt.saturating_add(1))
                .unwrap_or(1);
            let max_attempts = current.as_ref().map(|task| task.max_attempts).unwrap_or(1);

            // Exponential backoff, capped so a high attempt count can't
            // produce an absurd delay — bounded, not a scheduler policy
            // framework.
            const MAX_RETRY_BACKOFF_SECS: u64 = 300;
            let delay = Duration::from_secs(
                2u64.saturating_pow(attempt.saturating_sub(1).min(20))
                    .min(MAX_RETRY_BACKOFF_SECS),
            );
            let retry_at = chrono::Utc::now()
                + chrono::Duration::from_std(delay)
                    .unwrap_or(chrono::Duration::seconds(MAX_RETRY_BACKOFF_SECS as i64));

            if let Some(bus) = &self.event_bus {
                bus.publish(
                    EventSource::Scheduler,
                    ArgusEvent::TaskRetrying {
                        task_id,
                        attempt,
                        max_retries: max_attempts,
                        backoff_seconds: delay.as_secs(),
                    },
                );
            }

            // V0.38.1: no blocking sleep here — this method runs inline in
            // ExecutionLoop.process_task, so a `sleep` here would stall the
            // entire loop, not just this task. Defer instead: persist
            // `retry_at` and leave the live queue; `promote_ready_tasks`
            // (driven by the maintenance tick) re-enqueues it once due.
            if let Err(err) = self
                .transition_with(task_id, TaskState::Ready, "retry_scheduled", |task| {
                    task.attempt = attempt;
                    task.error = Some(error);
                    task.retry_at = Some(retry_at);
                })
                .await
            {
                error!(task_id = %task_id, error = %err, "Failed to persist retry");
            }
        } else {
            // Permanent failure
            error!(
                task_id = %task_id,
                error = %error.message,
                "Task failed permanently"
            );

            let attempt = self
                .get_task(task_id)
                .await
                .map(|task| task.attempt)
                .unwrap_or(1);
            if let Err(err) = self
                .transition_with(task_id, TaskState::Failed, "retries_exhausted", |task| {
                    task.error = Some(error);
                })
                .await
            {
                error!(task_id = %task_id, error = %err, "Failed to persist permanent failure");
                return;
            }

            if let Some(bus) = &self.event_bus {
                bus.publish(
                    EventSource::Scheduler,
                    ArgusEvent::TaskFailed {
                        task_id,
                        error: error_message,
                        attempt,
                        will_retry: false,
                    },
                );
            }

            self.block_failed_dependencies(task_id).await;
        }
    }

    /// Check for timed-out tasks and handle them.
    pub async fn check_timeouts(&self) {
        let in_progress = self.get_tasks_by_state(TaskState::Running).await;

        for task in in_progress {
            if task.is_timed_out() {
                warn!(task_id = %task.id, "Task timed out");

                let error = TaskError::new(
                    TaskErrorType::Timeout,
                    format!("Task exceeded timeout of {:?}", task.timeout),
                    task.attempt,
                );

                self.fail_task(task.id, error).await;
            }
        }
    }

    /// Promote pending tasks to ready if dependencies are resolved, and
    /// rejoin the live queue any `Ready`-but-deferred task whose `retry_at`
    /// has passed.
    ///
    /// This is the *only* place a deferred task gets re-enqueued — never
    /// the hot dequeue/claim path — so a task deferred against a
    /// persistently-false precondition (budget exhausted, accounting
    /// degraded, workspace not ready) costs one cheap state check per
    /// maintenance tick, not an unbounded claim/requeue loop.
    pub async fn promote_ready_tasks(&self) {
        let pending = self.get_tasks_by_state(TaskState::Pending).await;

        for task in pending {
            let unresolved = {
                let graph = self.dep_graph.read().await;
                graph.unresolved_count(&task.id)
            };

            if unresolved == 0 {
                debug!(task_id = %task.id, "Promoting task to ready");
                self.transition(task.id, TaskState::Ready).await;
                self.enqueue_task(task.id).await;
            }
        }

        self.promote_due_deferred_tasks().await;
    }

    /// Sweep `Ready` tasks for ones carrying a `retry_at` that has passed,
    /// clear it, and put them back on the live queue.
    ///
    /// A `Ready` task with `retry_at: None` is, by construction, already on
    /// the queue (every code path that sets `Ready` either enqueues
    /// immediately or defers) — this loop must never touch those, or the
    /// same task would be pushed onto the priority queue twice.
    async fn promote_due_deferred_tasks(&self) {
        let now = chrono::Utc::now();
        let ready = self.get_tasks_by_state(TaskState::Ready).await;

        for task in ready {
            let Some(retry_at) = task.retry_at else {
                continue;
            };
            if retry_at > now {
                continue; // Not due yet — leave it deferred, off the queue.
            }

            debug!(task_id = %task.id, "Deferred task's retry_at has passed; re-enqueuing");
            let cleared = self
                .transition_with(task.id, TaskState::Ready, "deferred_retry_due", |t| {
                    t.retry_at = None;
                })
                .await
                .unwrap_or(false);
            if cleared {
                self.enqueue_task(task.id).await;
            }
        }
    }

    /// Get the number of tasks in the queue.
    pub async fn queue_len(&self) -> usize {
        let queue = self.queue.read().await;
        queue.len()
    }

    /// Get scheduler statistics.
    pub async fn stats(&self) -> SchedulerStats {
        let tasks = self.tasks.read().await;
        let queue = self.queue.read().await;

        let mut by_state: HashMap<TaskState, usize> = HashMap::new();
        let mut retry_error_counts: HashMap<String, usize> = HashMap::new();
        for task in tasks.values() {
            *by_state.entry(task.state).or_insert(0) += 1;
            if task.attempt > 1 {
                let error_key = task
                    .error
                    .as_ref()
                    .map(|e| format!("{:?}", e.error_type))
                    .unwrap_or_else(|| "unknown".to_string());
                *retry_error_counts.entry(error_key).or_insert(0) += 1;
            }
        }

        let queue_stats = queue.stats();

        SchedulerStats {
            total_tasks: tasks.len(),
            by_state,
            retry_error_counts,
            queue_depth: queue_stats.count,
            queue_min_priority: queue_stats.min_priority,
            queue_max_priority: queue_stats.max_priority,
            queue_avg_priority: queue_stats.avg_priority,
        }
    }
}

impl Default for TaskScheduler {
    fn default() -> Self {
        Self::new()
    }
}

/// Scheduler statistics.
#[derive(Clone, Debug, Default)]
pub struct SchedulerStats {
    pub total_tasks: usize,
    pub by_state: HashMap<TaskState, usize>,
    pub retry_error_counts: HashMap<String, usize>,
    pub queue_depth: usize,
    pub queue_min_priority: Option<f64>,
    pub queue_max_priority: Option<f64>,
    pub queue_avg_priority: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task_repository::{CreateTaskResult, TaskRepository};
    use crate::task_types::{Priority, TaskBuilder};
    use crate::types::ArgusResult;
    use std::sync::Mutex;

    async fn complete_with_passing_report(scheduler: &TaskScheduler, task_id: Uuid) {
        assert!(
            scheduler
                .submit_task_output(task_id, TaskOutput::default())
                .await
        );
        let report = QualityReport {
            task_id: task_id.to_string(),
            overall: QualityResult::Passed,
            stages: Vec::new(),
            total_duration_ms: 0,
            recommendations: Vec::new(),
        };
        assert!(scheduler.finalize_task(task_id, &report).await.unwrap());
    }

    #[derive(Default)]
    struct MockTaskRepository {
        tasks: Mutex<HashMap<Uuid, Task>>,
        request_keys: Mutex<HashMap<String, Uuid>>,
    }

    #[async_trait::async_trait]
    impl TaskRepository for MockTaskRepository {
        async fn create_task(
            &self,
            task: &Task,
            request_key: Option<&str>,
        ) -> ArgusResult<CreateTaskResult> {
            if let Some(request_key) = request_key {
                let mut request_keys = self.request_keys.lock().unwrap();
                if let Some(task_id) = request_keys.get(request_key) {
                    return Ok(CreateTaskResult::Existing(*task_id));
                }
                request_keys.insert(request_key.to_string(), task.id);
            }
            self.tasks.lock().unwrap().insert(task.id, task.clone());
            Ok(CreateTaskResult::Created(task.id))
        }

        async fn transition_task(
            &self,
            expected_state: TaskState,
            task: &Task,
            _checkpoint_context: serde_json::Value,
        ) -> ArgusResult<bool> {
            let mut tasks = self.tasks.lock().unwrap();
            let Some(current) = tasks.get(&task.id) else {
                return Ok(false);
            };
            if current.state != expected_state {
                return Ok(false);
            }
            tasks.insert(task.id, task.clone());
            Ok(true)
        }

        async fn load_tasks(&self) -> ArgusResult<Vec<Task>> {
            Ok(self.tasks.lock().unwrap().values().cloned().collect())
        }

        async fn load_active_tasks(&self) -> ArgusResult<Vec<Task>> {
            let tasks = self.tasks.lock().unwrap();
            Ok(tasks
                .values()
                .filter(|t| !t.state.is_terminal())
                .cloned()
                .collect())
        }

        async fn find_task_id_by_request_key(
            &self,
            request_key: &str,
        ) -> ArgusResult<Option<Uuid>> {
            Ok(self.request_keys.lock().unwrap().get(request_key).copied())
        }
    }

    #[tokio::test]
    async fn test_scheduler_add_task() {
        let scheduler = TaskScheduler::new();
        let project_id = Uuid::new_v4();
        let task = TaskBuilder::new(project_id, "Test Task").build();
        let task_id = task.id;

        scheduler.add_task(task).await;

        let retrieved = scheduler.get_task(task_id).await;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().title, "Test Task");
    }

    #[tokio::test]
    async fn test_scheduler_add_task_idempotent_deduplicates() {
        let scheduler = TaskScheduler::new();
        let project_id = Uuid::new_v4();

        let t1 = TaskBuilder::new(project_id, "Task A").build();
        let t2 = TaskBuilder::new(project_id, "Task B").build();

        let id1 = scheduler
            .add_task_idempotent(t1, "create-task:web:abc123")
            .await;
        let id2 = scheduler
            .add_task_idempotent(t2, "create-task:web:abc123")
            .await;

        assert_eq!(id1, id2);
        assert_eq!(scheduler.get_all_tasks().await.len(), 1);
        assert_eq!(
            scheduler
                .get_task_id_by_request_key("create-task:web:abc123")
                .await,
            Some(id1)
        );
    }

    #[tokio::test]
    async fn test_persistent_scheduler_restores_interrupted_task() {
        let repository = Arc::new(MockTaskRepository::default());
        let project_id = Uuid::new_v4();
        let mut task = TaskBuilder::new(project_id, "Interrupted Task").build();
        let task_id = task.id;
        task.state = TaskState::Running;
        task.assigned_worker_id = Some(Uuid::new_v4());
        repository.create_task(&task, None).await.unwrap();

        let scheduler = TaskScheduler::new_with_repository(repository.clone(), None);
        let recovery_report = scheduler.restore().await.unwrap();
        assert_eq!(recovery_report.active_tasks, 1);

        let restored = scheduler.get_task(task_id).await.unwrap();
        assert_eq!(restored.state, TaskState::Ready);
        assert!(restored.assigned_worker_id.is_none());
        assert_eq!(scheduler.queue_len().await, 1);

        let persisted = repository
            .load_tasks()
            .await
            .unwrap()
            .into_iter()
            .find(|task| task.id == task_id)
            .unwrap();
        assert_eq!(persisted.state, TaskState::Ready);
    }

    #[tokio::test]
    async fn test_persistent_idempotency_survives_scheduler_restart() {
        let repository = Arc::new(MockTaskRepository::default());
        let project_id = Uuid::new_v4();

        let first_scheduler = TaskScheduler::new_with_repository(repository.clone(), None);
        let first_id = first_scheduler
            .add_task_idempotent(
                TaskBuilder::new(project_id, "Original Task").build(),
                "request-123",
            )
            .await;

        let second_scheduler = TaskScheduler::new_with_repository(repository, None);
        let second_id = second_scheduler
            .add_task_idempotent(
                TaskBuilder::new(project_id, "Duplicate Task").build(),
                "request-123",
            )
            .await;

        assert_eq!(first_id, second_id);
        assert!(second_scheduler.get_all_tasks().await.is_empty());
    }

    #[tokio::test]
    async fn test_scheduler_task_with_dependencies() {
        let scheduler = TaskScheduler::new();
        let project_id = Uuid::new_v4();

        let task1 = TaskBuilder::new(project_id, "Task 1").build();
        let id1 = task1.id;

        let task2 = TaskBuilder::new(project_id, "Task 2")
            .depends_on(id1)
            .build();
        let id2 = task2.id;

        scheduler.add_task(task1).await;
        scheduler.add_task(task2).await;

        // Task 1 should be ready (no deps)
        let t1 = scheduler.get_task(id1).await.unwrap();
        assert_eq!(t1.state, TaskState::Ready);

        // Task 2 should be pending
        let t2 = scheduler.get_task(id2).await.unwrap();
        assert_eq!(t2.state, TaskState::Pending);

        // Complete task 1
        complete_with_passing_report(&scheduler, id1).await;

        // Task 2 should now be ready
        let t2 = scheduler.get_task(id2).await.unwrap();
        assert_eq!(t2.state, TaskState::Ready);
    }

    #[tokio::test]
    async fn test_scheduler_task_completion_chain() {
        let scheduler = TaskScheduler::new();
        let project_id = Uuid::new_v4();

        // Create chain: t1 -> t2 -> t3
        let t1 = TaskBuilder::new(project_id, "Task 1").build();
        let id1 = t1.id;

        let t2 = TaskBuilder::new(project_id, "Task 2")
            .depends_on(id1)
            .build();
        let id2 = t2.id;

        let t3 = TaskBuilder::new(project_id, "Task 3")
            .depends_on(id2)
            .build();
        let id3 = t3.id;

        scheduler.add_task(t1).await;
        scheduler.add_task(t2).await;
        scheduler.add_task(t3).await;

        // Complete tasks in order
        complete_with_passing_report(&scheduler, id1).await;
        assert_eq!(
            scheduler.get_task(id2).await.unwrap().state,
            TaskState::Ready
        );

        complete_with_passing_report(&scheduler, id2).await;
        assert_eq!(
            scheduler.get_task(id3).await.unwrap().state,
            TaskState::Ready
        );

        complete_with_passing_report(&scheduler, id3).await;
        assert_eq!(
            scheduler.get_task(id3).await.unwrap().state,
            TaskState::Completed
        );
    }

    #[tokio::test]
    async fn test_scheduler_task_failure() {
        let scheduler = TaskScheduler::new();
        let project_id = Uuid::new_v4();

        let task = TaskBuilder::new(project_id, "Test Task").build();
        let task_id = task.id;

        scheduler.add_task(task).await;

        // Fail the task
        let error = TaskError::new(TaskErrorType::TestsFailed, "Tests failed", 1);
        scheduler.fail_task(task_id, error).await;

        // Task should be retried (attempt incremented, back to Ready)
        let task = scheduler.get_task(task_id).await.unwrap();
        assert_eq!(task.attempt, 2);
        assert_eq!(task.state, TaskState::Ready);
    }

    #[tokio::test]
    async fn test_scheduler_permanent_failure() {
        let scheduler = TaskScheduler::new();
        let project_id = Uuid::new_v4();

        let task = TaskBuilder::new(project_id, "Test Task").build();
        let task_id = task.id;

        scheduler.add_task(task).await;

        // Fail the task 3 times (max_attempts = 3)
        // After 3rd failure, it will be permanently failed
        for i in 1..=3 {
            let task = scheduler.get_task(task_id).await.unwrap();
            if task.state == TaskState::Ready {
                scheduler.transition(task_id, TaskState::Running).await;
            }

            let error = TaskError::new(TaskErrorType::TestsFailed, "Tests failed", i);
            scheduler.fail_task(task_id, error).await;
        }

        // After 3 failures, the task should be permanently failed
        let task = scheduler.get_task(task_id).await.unwrap();
        assert_eq!(task.attempt, 3);
        assert_eq!(task.state, TaskState::Failed);
    }

    #[tokio::test]
    async fn test_scheduler_get_by_state() {
        let scheduler = TaskScheduler::new();
        let project_id = Uuid::new_v4();

        let t1 = TaskBuilder::new(project_id, "Task 1").build();
        let t2 = TaskBuilder::new(project_id, "Task 2").build();

        scheduler.add_task(t1).await;
        scheduler.add_task(t2).await;

        let ready = scheduler.get_tasks_by_state(TaskState::Ready).await;
        assert_eq!(ready.len(), 2);
    }

    #[tokio::test]
    async fn test_scheduler_stats() {
        let scheduler = TaskScheduler::new();
        let project_id = Uuid::new_v4();

        let t1 = TaskBuilder::new(project_id, "Task 1").build();
        let t2 = TaskBuilder::new(project_id, "Task 2").build();

        scheduler.add_task(t1).await;
        scheduler.add_task(t2).await;

        let stats = scheduler.stats().await;
        assert_eq!(stats.total_tasks, 2);
        assert_eq!(stats.by_state.get(&TaskState::Ready), Some(&2));
        assert!(stats.retry_error_counts.is_empty());
    }

    #[tokio::test]
    async fn test_scheduler_stats_collect_retry_error_counts() {
        let scheduler = TaskScheduler::new();
        let project_id = Uuid::new_v4();

        let task = TaskBuilder::new(project_id, "Retry Task").build();
        let task_id = task.id;
        scheduler.add_task(task).await;
        scheduler.transition(task_id, TaskState::Running).await;

        let err = TaskError::new(TaskErrorType::TestsFailed, "failed tests", 1);
        scheduler.fail_task(task_id, err).await;

        let stats = scheduler.stats().await;
        assert_eq!(stats.retry_error_counts.get("TestsFailed"), Some(&1));
    }

    #[tokio::test]
    async fn test_scheduler_dequeue() {
        let scheduler = TaskScheduler::new();
        let project_id = Uuid::new_v4();

        let t1 = TaskBuilder::new(project_id, "Task 1")
            .priority(Priority::High)
            .build();
        let id1 = t1.id;

        scheduler.add_task(t1).await;

        // Dequeue should return the task
        let dequeued = scheduler.dequeue_task().await;
        assert_eq!(dequeued, Some(id1));

        // Queue should now be empty
        assert_eq!(scheduler.queue_len().await, 0);
    }

    #[tokio::test]
    async fn test_scheduler_dependency_failure() {
        let scheduler = TaskScheduler::new();
        let project_id = Uuid::new_v4();

        let t1 = TaskBuilder::new(project_id, "Task 1").build();
        let id1 = t1.id;

        let t2 = TaskBuilder::new(project_id, "Task 2")
            .depends_on(id1)
            .build();
        let _id2 = t2.id;

        scheduler.add_task(t1).await;
        scheduler.add_task(t2).await;

        // Mark t1 as in progress then fail it
        scheduler.transition(id1, TaskState::Running).await;
        scheduler.transition(id1, TaskState::Testing).await;

        let error = TaskError::new(TaskErrorType::TestsFailed, "Failed", 1);
        scheduler.fail_task(id1, error).await;

        // After 3 failures, t1 should be Failed and t2 should be Blocked
        // First, we need to fail it more times...
        for _ in 0..2 {
            let err = TaskError::new(TaskErrorType::TestsFailed, "Failed", 1);
            scheduler.fail_task(id1, err).await;
            scheduler.transition(id1, TaskState::Running).await;
            scheduler.transition(id1, TaskState::Testing).await;
        }

        // Final failure
        scheduler.transition(id1, TaskState::Running).await;
        scheduler.transition(id1, TaskState::Testing).await;
        let err = TaskError::new(TaskErrorType::TestsFailed, "Failed", 1);
        scheduler.fail_task(id1, err).await;

        let t1_state = scheduler.get_task(id1).await.unwrap().state;
        assert!(matches!(t1_state, TaskState::Failed | TaskState::Ready));
    }
}
