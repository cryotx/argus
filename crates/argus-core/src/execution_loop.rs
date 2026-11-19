//! ExecutionLoop — the background driver that makes the scheduler run itself.
//!
//! Before this module existed, a task created through `POST /api/tasks` was
//! persisted, promoted to `Ready`, pushed onto the priority queue — and then
//! sat there forever, because nothing ever called [`TaskScheduler::dequeue_task`]
//! outside of a hidden test command. This loop is the missing driver.
//!
//! # Responsibilities
//!
//! - **Maintenance tick** — periodically calls [`TaskScheduler::promote_ready_tasks`]
//!   and [`TaskScheduler::check_timeouts`].
//! - **Dequeue tick** — claims the next task, walks it through the existing
//!   state machine, and hands it to a [`TaskExecutor`].
//! - **Finalization** — routes the result through the existing quality gate so
//!   `Completed` still requires passing evidence.
//!
//! # What this module deliberately does *not* do
//!
//! It does not re-implement scheduling, dependency resolution, persistence or
//! crash recovery. Every state change goes through the existing public
//! `TaskScheduler` API so checkpoints, events and the DAG stay consistent.
//!
//! Concurrency is fixed at one task at a time for Vertical Slice 1.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::accounting_state::RuntimeAccountingState;
use crate::artifact_repository::ArtifactRepository;
use crate::budget_service::BudgetService;
use crate::checkpoint::{CommitIdentity, GitCheckpointFinalizer, GitCheckpointer};
use crate::quality::{QualityChecker, QualityConfig};
use crate::spend_repository::{SpendRecord, SpendRepository};
use crate::task_scheduler::TaskScheduler;
use crate::task_types::{Task, TaskError, TaskErrorType, TaskOutput, TaskState};
use crate::workspace_manager::{WorkspaceContext, WorkspaceGuard, WorkspaceManager};
use crate::workspace_repository::WorkspaceRepository;

// ============================================================
// Execution result
// ============================================================

/// Structured outcome of executing a single task.
///
/// Named `TaskExecutionResult` rather than `ExecutionResult` because
/// [`crate::execution_runtime::ExecutionResult`] already exists in this crate
/// and means something narrower (the result of one shell invocation).
#[derive(Clone, Debug)]
pub struct TaskExecutionResult {
    pub task_id: Uuid,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stdout_summary: String,
    pub stderr_summary: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    /// Machine-set reason the task failed. `None` when `success` is true.
    pub failure_reason: Option<String>,
    /// Repository-relative paths the executor actually changed.
    ///
    /// The quality gate scans these, so an executor that edits a repo must
    /// report them or the gate has nothing to check.
    pub files_changed: Vec<String>,
    /// Directory the work happened in. The quality gate runs test commands
    /// here; `None` falls back to the process working directory.
    ///
    /// From V0.38 Phase 3 onward, when workspace isolation is active this is
    /// always the task's Git worktree — never the canonical repository.
    pub workspace: Option<PathBuf>,
    /// Executor-specific evidence (model, cost, token counts, session id...).
    pub metadata: std::collections::HashMap<String, String>,
    /// V0.38: Workspace isolation support
    /// UUID of the task workspace (worktree) this execution ran in
    pub workspace_id: Option<Uuid>,
    /// Git branch name the worktree was based on
    pub branch_name: Option<String>,
    /// Git commit SHA (base revision) worktree was created from
    pub base_revision: Option<String>,
    /// V0.38 Phase 3: explicit worktree path, mirroring `workspace` above.
    /// Kept as a separate strongly-typed field (rather than only in
    /// `metadata`) so callers don't have to guess whether `workspace` is the
    /// worktree or something else.
    pub worktree_path: Option<PathBuf>,
}

impl TaskExecutionResult {
    /// Build a failure result that never ran a command.
    pub fn failed(task_id: Uuid, started_at: DateTime<Utc>, reason: impl Into<String>) -> Self {
        Self {
            task_id,
            success: false,
            exit_code: None,
            stdout_summary: String::new(),
            stderr_summary: String::new(),
            started_at,
            finished_at: Utc::now(),
            failure_reason: Some(reason.into()),
            files_changed: Vec::new(),
            workspace: None,
            metadata: std::collections::HashMap::new(),
            workspace_id: None,
            branch_name: None,
            base_revision: None,
            worktree_path: None,
        }
    }

    /// Wall-clock duration of the execution.
    pub fn duration(&self) -> chrono::Duration {
        self.finished_at - self.started_at
    }
}

// ============================================================
// Executor abstraction
// ============================================================

/// Everything an executor needs to run one task: the task itself, plus the
/// isolated workspace it must run in.
///
/// `workspace` is `None` only when the loop has no workspace integration
/// attached at all (legacy/test configurations). Whenever workspace
/// integration *is* attached, `ExecutionLoop` guarantees an executor never
/// sees a request without a verified `workspace` — see rule #4 in the V0.38
/// Phase 3 spec: a missing or unverifiable workspace means the Agent is
/// never spawned in the first place, so there is no request to build.
#[derive(Clone, Debug)]
pub struct ExecutionRequest {
    pub task: Task,
    pub workspace: Option<WorkspaceContext>,
}

/// Something that can execute one task.
///
/// Vertical Slice 1 shipped `ShellTaskExecutor`. The real agent-backed
/// executor (`AgentTaskExecutor`) implements this same trait and, from
/// V0.38 Phase 3 onward, must run in `request.workspace.worktree_path` when
/// a workspace is present — never in the canonical repository.
#[async_trait]
pub trait TaskExecutor: Send + Sync {
    /// Short label used in logs and task metadata.
    fn name(&self) -> &str;

    /// Execute the task. Implementations should not panic, but the loop
    /// isolates panics anyway.
    async fn execute(&self, request: &ExecutionRequest) -> TaskExecutionResult;
}

/// **NoOp executor — used for testing the loop structure without actual execution.**
///
/// In production, this would be replaced by an AgentExecutor that spawns
/// Claude Code or another agent. This stub allows tests to verify the loop's
/// state machine and budget enforcement without depending on agent infrastructure.
pub struct NoOpTaskExecutor;

#[async_trait]
impl TaskExecutor for NoOpTaskExecutor {
    fn name(&self) -> &str {
        "noop"
    }

    async fn execute(&self, request: &ExecutionRequest) -> TaskExecutionResult {
        let started_at = Utc::now();
        let ws = request.workspace.as_ref();
        TaskExecutionResult {
            task_id: request.task.id,
            success: true,
            exit_code: Some(0),
            stdout_summary: "NoOp executor (stub for testing)".to_string(),
            stderr_summary: String::new(),
            started_at,
            finished_at: Utc::now(),
            failure_reason: None,
            files_changed: Vec::new(),
            workspace: ws.map(|w| w.worktree_path.clone()),
            metadata: Default::default(),
            workspace_id: ws.map(|w| w.workspace_id),
            branch_name: ws.map(|w| w.branch_name.clone()),
            base_revision: ws.map(|w| w.base_revision.clone()),
            worktree_path: ws.map(|w| w.worktree_path.clone()),
        }
    }
}

// ============================================================
// Loop configuration
// ============================================================

#[derive(Clone, Debug)]
pub struct ExecutionLoopConfig {
    /// How long to wait before polling again when the queue is empty.
    pub idle_interval: Duration,
    /// How often to run `promote_ready_tasks` + `check_timeouts`.
    pub maintenance_interval: Duration,
}

impl Default for ExecutionLoopConfig {
    fn default() -> Self {
        Self {
            idle_interval: Duration::from_millis(500),
            maintenance_interval: Duration::from_secs(5),
        }
    }
}

// ============================================================
// Handle
// ============================================================

/// Handle to a running [`ExecutionLoop`]. Dropping it does not stop the loop;
/// call [`ExecutionLoopHandle::shutdown`].
pub struct ExecutionLoopHandle {
    shutdown_tx: watch::Sender<bool>,
    join: JoinHandle<()>,
}

impl ExecutionLoopHandle {
    /// Signal the loop to stop and wait for it to finish the current task.
    pub async fn shutdown(self) {
        let _ = self.shutdown_tx.send(true);
        match self.join.await {
            Ok(()) => info!("ExecutionLoop stopped"),
            Err(e) if e.is_cancelled() => info!("ExecutionLoop task cancelled"),
            Err(e) => error!(error = %e, "ExecutionLoop stopped abnormally"),
        }
    }

    /// Whether the loop task has finished.
    pub fn is_finished(&self) -> bool {
        self.join.is_finished()
    }
}

// ============================================================
// The loop
// ============================================================

/// Everything needed to create and verify a task's isolated Git worktree,
/// bundled so `ExecutionLoop` either has full workspace integration or none
/// — there is no partially-configured state to reason about.
#[derive(Clone)]
struct WorkspaceIntegration {
    repo: Arc<dyn WorkspaceRepository>,
    manager: Arc<WorkspaceManager>,
    guard: Arc<dyn WorkspaceGuard>,
    /// Root directory under which each project's canonical (non-worktree)
    /// repository lives, one subdirectory per `project_id`.
    canonical_repo_root: PathBuf,
}

/// V0.38 Phase 4: everything needed to turn a quality-passing execution's
/// worktree changes into a durable Git Checkpoint + Artifact. Bundled
/// exactly like [`WorkspaceIntegration`] — attached only alongside
/// Workspace integration (a checkpoint is meaningless without a worktree to
/// checkpoint), never independently.
#[derive(Clone)]
struct CheckpointIntegration {
    checkpointer: Arc<dyn GitCheckpointer>,
    artifacts: Arc<dyn ArtifactRepository>,
    identity: CommitIdentity,
}

/// Why a task was pulled back before the Agent was spawned.
///
/// This is the enforcement point for rule #4 (no canonical-repo fallback):
/// every branch that can produce a `WorkspaceGate` ends in either a requeue
/// (transient — try again with the same, reused workspace) or a task
/// failure (an integrity violation the loop refuses to paper over). Neither
/// branch ever calls the executor.
enum WorkspaceGate {
    /// Transient condition (DB hiccup, first-time worktree creation error).
    /// Safe to retry — the workspace record (if any) is left exactly as is.
    Requeue(String),
    /// An integrity violation: ownership, path safety, branch, or base
    /// revision did not check out. The task fails; a human decides what
    /// happens to the workspace next.
    Fail(TaskErrorType, String),
}

// ============================================================
// V0.38.1 — deferred-retry backoff (never a blocking sleep)
// ============================================================
//
// Every admission-gate block below goes through `TaskScheduler::defer_task`
// rather than an immediate `requeue_task`: the task is persisted with a
// future `retry_at` and left off the live queue, and `promote_ready_tasks`
// (the maintenance tick, not the hot dequeue/claim path) re-enqueues it once
// due. This is what turns "precondition permanently false" from an
// unbounded claim/requeue hot loop into one cheap state check per interval.
//
// Deliberately not a policy framework: four fixed categories, each a small
// constant or a simple bounded-exponential helper.

/// Transient workspace-preparation issues (canonical repo not ready yet, a
/// DB lookup blip, first-time worktree creation failing) — short bounded
/// exponential backoff.
const WORKSPACE_TRANSIENT_BACKOFF_BASE: Duration = Duration::from_secs(2);
const WORKSPACE_TRANSIENT_BACKOFF_CAP: Duration = Duration::from_secs(60);

/// Generic preflight check failures (DB hiccup, timeout) that aren't the
/// distinct accounting-degraded condition — short bounded exponential
/// backoff, same shape as the workspace case.
const TRANSIENT_PREFLIGHT_BACKOFF_BASE: Duration = Duration::from_secs(5);
const TRANSIENT_PREFLIGHT_BACKOFF_CAP: Duration = Duration::from_secs(60);

/// Accounting DEGRADED (V0.37.2) requires an explicit operator
/// `mark_healthy()` — there is nothing to exponentially back off from, so a
/// flat, moderate re-check interval is simplest and correct.
const ACCOUNTING_DEGRADED_BACKOFF: Duration = Duration::from_secs(60);

/// Fallback for a `Blocked` budget decision with no natural reset point
/// (per-task limit, an estimate exceeding remaining budget) — long and flat,
/// since retrying sooner cannot help and retrying much later costs nothing.
const BUDGET_BLOCKED_FALLBACK_BACKOFF: Duration = Duration::from_secs(15 * 60);

/// `now + delay`, as a `DateTime<Utc>` `retry_at`.
fn defer_until(delay: Duration) -> DateTime<Utc> {
    Utc::now() + chrono::Duration::from_std(delay).unwrap_or_else(|_| chrono::Duration::seconds(60))
}

/// `base * 2^defer_count`, capped — bounded exponential backoff sized by how
/// many times this task has already been deferred (any reason; it's a
/// generic "how long has this been stuck" signal, not per-category state).
fn bounded_backoff(defer_count: u32, base: Duration, cap: Duration) -> Duration {
    let multiplier = 1u32.checked_shl(defer_count.min(20)).unwrap_or(u32::MAX);
    base.saturating_mul(multiplier).min(cap)
}

/// Start of the next UTC day — the real reset point for a daily budget.
fn next_utc_midnight(now: DateTime<Utc>) -> DateTime<Utc> {
    let tomorrow = (now + chrono::Duration::days(1)).date_naive();
    tomorrow
        .and_hms_opt(0, 0, 0)
        .expect("00:00:00 is always a valid time")
        .and_utc()
}

#[cfg(test)]
mod backoff_tests {
    use super::*;

    #[test]
    fn test_bounded_backoff_grows_then_caps() {
        let base = Duration::from_secs(2);
        let cap = Duration::from_secs(60);
        assert_eq!(bounded_backoff(0, base, cap), Duration::from_secs(2));
        assert_eq!(bounded_backoff(1, base, cap), Duration::from_secs(4));
        assert_eq!(bounded_backoff(2, base, cap), Duration::from_secs(8));
        assert_eq!(
            bounded_backoff(10, base, cap),
            cap,
            "must saturate at cap, not overflow"
        );
        assert_eq!(bounded_backoff(u32::MAX, base, cap), cap);
    }

    #[test]
    fn test_next_utc_midnight_is_start_of_tomorrow() {
        let now = DateTime::parse_from_rfc3339("2026-01-15T13:45:30Z")
            .unwrap()
            .with_timezone(&Utc);
        let next = next_utc_midnight(now);
        assert_eq!(next.to_rfc3339(), "2026-01-16T00:00:00+00:00");
        assert!(next > now);
    }
}

pub struct ExecutionLoop {
    scheduler: Arc<TaskScheduler>,
    executor: Arc<dyn TaskExecutor>,
    config: ExecutionLoopConfig,
    /// Optional budget service for Agent spend enforcement.
    /// If present, every paid Agent execution requires a passing preflight check.
    budget_service: Option<Arc<BudgetService>>,
    /// Optional spend repository for recording Agent costs.
    /// If present, execution costs are persisted after completion.
    spend_repo: Option<Arc<dyn SpendRepository>>,
    /// Optional accounting state tracker for spend ledger health.
    /// If present, record_spend failures mark system as degraded.
    accounting_state: Option<RuntimeAccountingState>,
    /// V0.38 Phase 3: workspace (Git worktree) isolation. When attached,
    /// every execution runs in a verified worktree and the canonical
    /// repository is never used as Agent cwd — see rule #4.
    workspace: Option<WorkspaceIntegration>,
    /// V0.38 Phase 4: Git Checkpoint + Artifact persistence. When attached
    /// (alongside `workspace`), a task cannot reach `Completed` until its
    /// verified worktree changes are a durable Git commit with a persisted
    /// Artifact record — see `crate::checkpoint`.
    checkpoint: Option<CheckpointIntegration>,
    evaluation: Option<Arc<crate::evaluation::EvaluationHarness>>,
}

impl ExecutionLoop {
    pub fn new(
        scheduler: Arc<TaskScheduler>,
        executor: Arc<dyn TaskExecutor>,
        config: ExecutionLoopConfig,
    ) -> Self {
        Self {
            scheduler,
            executor,
            config,
            budget_service: None,
            spend_repo: None,
            accounting_state: None,
            workspace: None,
            checkpoint: None,
            evaluation: None,
        }
    }

    /// Attach a budget service to enforce spend limits.
    pub fn with_budget_service(mut self, budget_service: Arc<BudgetService>) -> Self {
        self.budget_service = Some(budget_service);
        self
    }

    /// Attach a spend repository for recording costs.
    pub fn with_spend_repo(mut self, spend_repo: Arc<dyn SpendRepository>) -> Self {
        self.spend_repo = Some(spend_repo);
        self
    }

    /// Attach an accounting state tracker for spend ledger health.
    pub fn with_accounting_state(mut self, accounting_state: RuntimeAccountingState) -> Self {
        self.accounting_state = Some(accounting_state);
        self
    }

    /// Attach Git worktree isolation: every execution from here on gets its
    /// own verified worktree under `canonical_repo_root/<project_id>`, and
    /// the Agent is never spawned without one (rule #4).
    pub fn with_workspace(
        mut self,
        repo: Arc<dyn WorkspaceRepository>,
        manager: Arc<WorkspaceManager>,
        guard: Arc<dyn WorkspaceGuard>,
        canonical_repo_root: impl Into<PathBuf>,
    ) -> Self {
        self.workspace = Some(WorkspaceIntegration {
            repo,
            manager,
            guard,
            canonical_repo_root: canonical_repo_root.into(),
        });
        self
    }

    /// Attach V0.38 Phase 4 Git Checkpoint + Artifact persistence. Only
    /// takes effect together with [`Self::with_workspace`] — without a
    /// workspace there is no worktree to checkpoint, so `finalize` falls
    /// back to the pre-Phase-4 `complete_task_with_quality` path regardless
    /// of whether this is attached.
    pub fn with_checkpoint(
        mut self,
        checkpointer: Arc<dyn GitCheckpointer>,
        artifacts: Arc<dyn ArtifactRepository>,
    ) -> Self {
        self.checkpoint = Some(CheckpointIntegration {
            checkpointer,
            artifacts,
            identity: CommitIdentity::default(),
        });
        self
    }

    /// Override the default `ARGUS Agent <argus@localhost>` checkpoint
    /// commit identity. Must be called after [`Self::with_checkpoint`].
    pub fn with_evaluation(mut self, harness: Arc<crate::evaluation::EvaluationHarness>) -> Self {
        self.evaluation = Some(harness);
        self
    }

    pub fn with_checkpoint_identity(mut self, identity: CommitIdentity) -> Self {
        if let Some(cp) = &mut self.checkpoint {
            cp.identity = identity;
        }
        self
    }

    /// Spawn the loop onto the current tokio runtime.
    pub fn spawn(self) -> ExecutionLoopHandle {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let join = tokio::spawn(async move { self.run(shutdown_rx).await });
        ExecutionLoopHandle { shutdown_tx, join }
    }

    /// Run until the shutdown signal fires.
    ///
    /// This never returns early on a task error: a failing task is recorded
    /// and the loop moves on to the next one.
    async fn run(self, mut shutdown: watch::Receiver<bool>) {
        info!(
            executor = self.executor.name(),
            idle_ms = self.config.idle_interval.as_millis(),
            maintenance_s = self.config.maintenance_interval.as_secs(),
            "ExecutionLoop started"
        );

        let mut last_maintenance = tokio::time::Instant::now();
        // Run maintenance once up front so restored tasks are promoted
        // immediately instead of after the first interval.
        self.maintenance_tick().await;

        loop {
            if *shutdown.borrow() {
                break;
            }

            if last_maintenance.elapsed() >= self.config.maintenance_interval {
                self.maintenance_tick().await;
                last_maintenance = tokio::time::Instant::now();
            }

            match self.scheduler.dequeue_task().await {
                Some(task_id) => {
                    // A task failure must never take the loop down.
                    self.process_task(task_id).await;
                }
                None => {
                    // Nothing to do — sleep, but stay interruptible so
                    // shutdown is never blocked behind the idle interval.
                    tokio::select! {
                        _ = tokio::time::sleep(self.config.idle_interval) => {}
                        _ = shutdown.changed() => {}
                    }
                }
            }
        }

        info!("ExecutionLoop shutdown signal received");
    }

    /// Advance scheduler bookkeeping that nothing else drives.
    async fn maintenance_tick(&self) {
        self.scheduler.promote_ready_tasks().await;
        self.scheduler.check_timeouts().await;
    }

    /// Take one claimed task from `Queued` through to a terminal state.
    ///
    /// `dequeue_task` has already claimed the task (`Ready -> Queued`) and
    /// assigned it an `execution_id`.
    ///
    /// Budget check happens here: if BudgetService is attached and a preflight
    /// check fails, the task is immediately failed without execution.
    async fn process_task(&self, task_id: Uuid) {
        let Some(task) = self.scheduler.get_task(task_id).await else {
            warn!(task_id = %task_id, "Dequeued task vanished before execution");
            return;
        };

        let project_id = task.project_id;
        let attempt = task.attempt;
        let execution_id = task.execution_id;

        info!(
            task_id = %task_id,
            project_id = %project_id,
            attempt,
            execution_id = ?execution_id,
            title = %task.title,
            "Task dequeued"
        );

        // Queued -> Assigned -> Running, through the existing state machine.
        // This happens before workspace prep / budget check so a block or a
        // requeue always has a valid `Running -> Ready` transition available.
        if !self
            .scheduler
            .transition(task_id, TaskState::Assigned)
            .await
        {
            warn!(task_id = %task_id, state = %task.state, "Could not assign task; skipping");
            return;
        }
        if !self.scheduler.transition(task_id, TaskState::Running).await {
            warn!(task_id = %task_id, "Could not start task; skipping");
            return;
        }

        // Re-read so downstream steps see started_at / Running / execution_id.
        let running_task = self
            .scheduler
            .get_task(task_id)
            .await
            .unwrap_or_else(|| task.clone());

        // --- WORKSPACE PREPARATION (rule #13: created before budget preflight) ---
        //
        // Fail-closed per rule #4: if workspace integration is attached and
        // anything about the workspace cannot be established or verified,
        // the Agent is never spawned. `workspace_ctx` stays `None` only when
        // no workspace integration is configured at all (legacy/tests).
        let workspace_ctx = match self.prepare_workspace(&running_task, execution_id).await {
            Ok(ctx) => ctx,
            Err(WorkspaceGate::Requeue(reason)) => {
                let retry_at = defer_until(bounded_backoff(
                    running_task.defer_count,
                    WORKSPACE_TRANSIENT_BACKOFF_BASE,
                    WORKSPACE_TRANSIENT_BACKOFF_CAP,
                ));
                warn!(task_id = %task_id, reason = %reason, retry_at = %retry_at, "Workspace not ready; deferring (not looping)");
                if !self
                    .scheduler
                    .defer_task(task_id, retry_at, "workspace_not_ready")
                    .await
                    .unwrap_or(false)
                {
                    error!(task_id = %task_id, "Failed to defer task after workspace prep failure");
                }
                return;
            }
            Err(WorkspaceGate::Fail(error_type, reason)) => {
                error!(task_id = %task_id, reason = %reason, "Workspace integrity check failed; Agent NOT SPAWNED");
                self.scheduler
                    .fail_task(task_id, TaskError::new(error_type, reason, attempt))
                    .await;
                self.log_final_state(task_id, project_id).await;
                return;
            }
        };

        // --- PRE-FLIGHT BUDGET CHECK ---
        // If budget is insufficient, the task is requeued without executing.
        // The workspace prepared above is left exactly as is (rule #13/#24):
        // preserved, not marked Failed, reused verbatim on the next attempt.
        if let Some(budget_service) = &self.budget_service {
            // Estimated cost is unknown for now; in production this comes from task config
            let estimated_cost = None;
            let decision = budget_service
                .preflight_check(project_id, task_id, estimated_cost)
                .await;

            match decision {
                crate::budget_service::BudgetDecision::Allowed {
                    remaining_today_usd,
                } => {
                    info!(
                        task_id = %task_id,
                        remaining_today_usd,
                        "Budget check passed"
                    );
                }
                crate::budget_service::BudgetDecision::Blocked { reason, .. } => {
                    // Daily budget exhausted has a real, known reset point —
                    // defer to it instead of a short poll. Any other Blocked
                    // reason (per-task limit, estimate exceeds remaining)
                    // has no natural window, so fall back to a bounded,
                    // persistent delay rather than busy-retrying it.
                    let retry_at = if reason.contains("daily budget exhausted") {
                        next_utc_midnight(Utc::now())
                    } else {
                        defer_until(BUDGET_BLOCKED_FALLBACK_BACKOFF)
                    };
                    warn!(
                        task_id = %task_id,
                        project_id = %project_id,
                        reason = %reason,
                        retry_at = %retry_at,
                        "Budget check blocked execution; deferring to next window (not looping)"
                    );
                    // Deferred, NOT failed — this is a temporary admission gate
                    // block; attempt count is untouched.
                    if !self
                        .scheduler
                        .defer_task(task_id, retry_at, "budget_blocked")
                        .await
                        .unwrap_or(false)
                    {
                        error!(
                            task_id = %task_id,
                            "Failed to defer task after budget block; task state may be inconsistent"
                        );
                    }
                    return;
                }
                crate::budget_service::BudgetDecision::CheckFailed { error } => {
                    // Accounting-degraded is a distinct, longer-lived
                    // condition (V0.37.2: requires an explicit
                    // `mark_healthy()` operator action) from a plain
                    // transient preflight error (DB hiccup) — give it a
                    // longer, flat defer instead of hammering the same
                    // degraded check every tick.
                    let retry_at = if error.contains("accounting_degraded") {
                        defer_until(ACCOUNTING_DEGRADED_BACKOFF)
                    } else {
                        defer_until(bounded_backoff(
                            running_task.defer_count,
                            TRANSIENT_PREFLIGHT_BACKOFF_BASE,
                            TRANSIENT_PREFLIGHT_BACKOFF_CAP,
                        ))
                    };
                    error!(
                        task_id = %task_id,
                        project_id = %project_id,
                        error = %error,
                        retry_at = %retry_at,
                        "Budget check failed (fail-closed); deferring (not looping)"
                    );
                    // Fail-closed: deny execution now, but defer for retry —
                    // assumes the failure is temporary or awaiting operator
                    // recovery, not a reason to fail the task outright.
                    if !self
                        .scheduler
                        .defer_task(task_id, retry_at, "budget_check_failed")
                        .await
                        .unwrap_or(false)
                    {
                        error!(
                            task_id = %task_id,
                            "Failed to defer task after budget check error"
                        );
                    }
                    return;
                }
            }
        }

        // --- EXECUTION-BOUNDARY RE-VERIFICATION (rule #15) ---
        //
        // Even though `prepare_workspace` just verified this, ownership and
        // worktree validity are checked again immediately before spawning —
        // nothing may have changed in a single-threaded loop, but this is
        // the one gate that must never be trusted-by-construction.
        if let Some(ctx) = &workspace_ctx {
            if let Err(WorkspaceGate::Fail(error_type, reason)) = self
                .verify_workspace_before_spawn(&running_task, ctx, execution_id)
                .await
            {
                error!(task_id = %task_id, reason = %reason, "Workspace re-verification failed at spawn boundary; Agent NOT SPAWNED");
                self.scheduler
                    .fail_task(task_id, TaskError::new(error_type, reason, attempt))
                    .await;
                self.log_final_state(task_id, project_id).await;
                return;
            }
        }

        info!(
            task_id = %task_id,
            project_id = %project_id,
            attempt,
            executor = self.executor.name(),
            cwd = ?workspace_ctx.as_ref().map(|c| c.worktree_path.display().to_string()),
            "Task execution started"
        );

        // --- CANONICAL REPOSITORY BASELINE (rule #8) ---
        let canonical_before = self.canonical_baseline(workspace_ctx.as_ref()).await;

        let mut result = self
            .execute_isolated(&running_task, workspace_ctx.clone())
            .await;

        // --- CANONICAL REPOSITORY BASELINE COMPARISON (rule #8) ---
        if let Some(ctx) = &workspace_ctx {
            self.verify_canonical_untouched(ctx, canonical_before, &mut result)
                .await;
        }

        // --- files_changed MUST COME FROM THE TASK WORKTREE (rule #9) ---
        // Recomputed authoritatively here so this holds regardless of what
        // an individual executor implementation reported.
        if let Some(ws) = &self.workspace {
            if let Some(ctx) = &workspace_ctx {
                match ws.guard.files_changed(&ctx.worktree_path).await {
                    Ok(files) => result.files_changed = files,
                    Err(e) => warn!(
                        task_id = %task_id,
                        error = %e,
                        "Failed to compute files_changed from worktree; leaving executor-reported list"
                    ),
                }
                result.workspace_id.get_or_insert(ctx.workspace_id);
                result
                    .branch_name
                    .get_or_insert_with(|| ctx.branch_name.clone());
                result
                    .base_revision
                    .get_or_insert_with(|| ctx.base_revision.clone());
                result
                    .workspace
                    .get_or_insert_with(|| ctx.worktree_path.clone());
                result
                    .worktree_path
                    .get_or_insert_with(|| ctx.worktree_path.clone());
            }
        }

        if result.success {
            info!(
                task_id = %task_id,
                project_id = %project_id,
                attempt,
                exit_code = ?result.exit_code,
                duration_ms = result.duration().num_milliseconds(),
                "Task execution completed"
            );
        } else {
            warn!(
                task_id = %task_id,
                project_id = %project_id,
                attempt,
                exit_code = ?result.exit_code,
                reason = %result.failure_reason.as_deref().unwrap_or("unknown"),
                "Task execution failed"
            );

            // --- WORKSPACE LIFECYCLE ON AGENT FAILURE (rule #12/#25) ---
            // Active -> Failed. The worktree itself is never deleted here —
            // cleanup is out of scope for Phase 3 (rule #28).
            if let (Some(ws), Some(ctx)) = (&self.workspace, &workspace_ctx) {
                let reason = result
                    .failure_reason
                    .clone()
                    .unwrap_or_else(|| "agent execution failed".to_string());
                if let Err(e) = ws.repo.mark_failed(ctx.workspace_id, &reason).await {
                    error!(task_id = %task_id, workspace_id = %ctx.workspace_id, error = %e, "Failed to mark workspace failed");
                }
            }
        }
        // On success the workspace intentionally stays Active — Phase 4 owns
        // the Quality Gate -> Git Checkpoint -> Completed transition (rule #12).

        self.finalize(&running_task, workspace_ctx, result).await;
    }

    /// Ensure a verified [`WorkspaceContext`] exists for this execution, or
    /// return the reason the Agent must not be spawned. Idempotent: safe to
    /// call again after a budget-blocked or accounting-degraded requeue —
    /// it reuses the existing workspace record and worktree rather than
    /// creating a second one (rules #13/#14/#24).
    async fn prepare_workspace(
        &self,
        task: &Task,
        execution_id: Option<Uuid>,
    ) -> Result<Option<WorkspaceContext>, WorkspaceGate> {
        let Some(ws) = &self.workspace else {
            return Ok(None);
        };

        // Rule #4: a missing WorkspaceContext (no execution_id assigned by
        // the scheduler) is a hard stop, not a fallback to the canonical repo.
        let Some(execution_id) = execution_id else {
            return Err(WorkspaceGate::Fail(
                TaskErrorType::Unknown,
                "workspace context missing: task has no execution_id".to_string(),
            ));
        };

        let project_id = task.project_id;
        let canonical_repository_path = ws.canonical_repo_root.join(project_id.to_string());
        let worktree_path = ws.manager.worktree_path(project_id, execution_id);
        let branch_name = ws.manager.branch_name(task.id, execution_id);

        if let Err(e) = ws.manager.validate_worktree_assignment(
            project_id,
            execution_id,
            &worktree_path,
            &canonical_repository_path,
        ) {
            return Err(WorkspaceGate::Fail(
                TaskErrorType::SecurityViolation,
                format!("path safety validation failed before workspace creation: {e}"),
            ));
        }

        if let Err(e) = ws
            .guard
            .ensure_canonical_repo(&canonical_repository_path)
            .await
        {
            return Err(WorkspaceGate::Requeue(format!(
                "canonical repository not ready: {e}"
            )));
        }

        // Idempotent lookup: if a workspace record already exists for this
        // execution_id (e.g. a prior attempt that got budget-blocked), reuse
        // its base_revision verbatim — it must never be recomputed, or a
        // worktree created on attempt 1 could silently disagree with a
        // freshly-read canonical HEAD on attempt 2.
        let existing = ws
            .repo
            .get_by_execution_id(execution_id)
            .await
            .map_err(|e| {
                WorkspaceGate::Requeue(format!("workspace repository lookup failed: {e}"))
            })?;

        let base_revision = if let Some(existing) = &existing {
            if existing.project_id != project_id || existing.task_id != task.id {
                return Err(WorkspaceGate::Fail(
                    TaskErrorType::SecurityViolation,
                    format!(
                        "workspace ownership mismatch: execution_id {} already maps to project_id={} task_id={}, not {}/{}",
                        execution_id, existing.project_id, existing.task_id, project_id, task.id
                    ),
                ));
            }
            existing.base_revision.clone()
        } else {
            ws.guard
                .head_revision(&canonical_repository_path)
                .await
                .map_err(|e| {
                    WorkspaceGate::Requeue(format!("cannot resolve canonical HEAD: {e}"))
                })?
        };

        let record = ws
            .repo
            .ensure_workspace(
                execution_id,
                project_id,
                task.id,
                canonical_repository_path.display().to_string(),
                worktree_path.display().to_string(),
                branch_name.clone(),
                base_revision.clone(),
            )
            .await
            .map_err(|e| WorkspaceGate::Requeue(format!("ensure_workspace failed: {e}")))?;

        // Rule #5/#13/#24/#29: whether the worktree is *created* or merely
        // *re-verified* depends on whether a workspace record already
        // existed. This is the line between legitimate reuse and forbidden
        // auto-repair:
        // - No prior record -> this is genuinely the first attempt; create it.
        // - A prior record exists -> a worktree should already be on disk.
        //   If it verifies, reuse it untouched (budget-block / accounting-
        //   degraded requeue). If it does NOT verify, something deleted or
        //   corrupted it after creation — that is exactly rule #21's
        //   "missing worktree" scenario, and it is never silently recreated.
        if existing.is_none() {
            if let Err(e) = ws
                .guard
                .ensure_worktree(
                    &canonical_repository_path,
                    &worktree_path,
                    &branch_name,
                    &base_revision,
                )
                .await
            {
                return Err(WorkspaceGate::Requeue(format!(
                    "worktree creation failed: {e}"
                )));
            }
        } else if let Err(e) = ws
            .guard
            .verify_worktree(&canonical_repository_path, &worktree_path)
            .await
        {
            let reason = format!(
                "workspace record exists but worktree failed verification (not auto-repaired): {e}"
            );
            let _ = ws
                .repo
                .mark_recovery_needed(record.workspace_id, &reason)
                .await;
            return Err(WorkspaceGate::Fail(TaskErrorType::WorkerCrashed, reason));
        }

        if let Err(e) = ws.repo.mark_active(record.workspace_id).await {
            warn!(workspace_id = %record.workspace_id, error = %e, "Failed to mark workspace active");
        }

        Ok(Some(WorkspaceContext {
            workspace_id: record.workspace_id,
            project_id,
            task_id: task.id,
            execution_id,
            canonical_repository_path,
            worktree_path,
            branch_name,
            base_revision,
        }))
    }

    /// Re-verify a previously prepared workspace immediately before spawning
    /// the Agent (rule #15). Checks ownership, path safety (including
    /// symlink escape, which only becomes checkable once the path exists),
    /// that the worktree is a real, live Git worktree owned by the
    /// canonical repository, that its branch matches, and that its HEAD has
    /// not drifted from `base_revision` (rule #6/#7).
    async fn verify_workspace_before_spawn(
        &self,
        task: &Task,
        ctx: &WorkspaceContext,
        execution_id: Option<Uuid>,
    ) -> Result<(), WorkspaceGate> {
        let Some(ws) = &self.workspace else {
            return Ok(());
        };

        let execution_id = execution_id.ok_or_else(|| {
            WorkspaceGate::Fail(
                TaskErrorType::Unknown,
                "workspace context missing: task has no execution_id".to_string(),
            )
        })?;

        ctx.validate_ownership(task, execution_id)
            .map_err(|e| WorkspaceGate::Fail(TaskErrorType::SecurityViolation, e.to_string()))?;

        if let Err(e) = ws
            .manager
            .validate_worktree_no_escape(&ctx.worktree_path, &ctx.canonical_repository_path)
        {
            let _ = ws
                .repo
                .mark_recovery_needed(ctx.workspace_id, &e.to_string())
                .await;
            return Err(WorkspaceGate::Fail(
                TaskErrorType::SecurityViolation,
                e.to_string(),
            ));
        }

        let facts = match ws
            .guard
            .verify_worktree(&ctx.canonical_repository_path, &ctx.worktree_path)
            .await
        {
            Ok(facts) => facts,
            Err(e) => {
                let reason = format!("worktree verification failed: {e}");
                let _ = ws
                    .repo
                    .mark_recovery_needed(ctx.workspace_id, &reason)
                    .await;
                return Err(WorkspaceGate::Fail(TaskErrorType::WorkerCrashed, reason));
            }
        };

        if facts.head != ctx.base_revision {
            let reason = format!(
                "base revision drift: worktree HEAD is {} but WorkspaceContext.base_revision is {}",
                facts.head, ctx.base_revision
            );
            let _ = ws
                .repo
                .mark_recovery_needed(ctx.workspace_id, &reason)
                .await;
            return Err(WorkspaceGate::Fail(
                TaskErrorType::SecurityViolation,
                reason,
            ));
        }

        if facts.branch.as_deref() != Some(ctx.branch_name.as_str()) {
            let reason = format!(
                "workspace branch mismatch: worktree is on {:?} but WorkspaceContext.branch_name is {}",
                facts.branch, ctx.branch_name
            );
            let _ = ws
                .repo
                .mark_recovery_needed(ctx.workspace_id, &reason)
                .await;
            return Err(WorkspaceGate::Fail(
                TaskErrorType::SecurityViolation,
                reason,
            ));
        }

        Ok(())
    }

    /// Snapshot `(HEAD, git status --porcelain)` of the canonical repository
    /// before execution, for the rule #8 before/after comparison.
    async fn canonical_baseline(&self, ctx: Option<&WorkspaceContext>) -> Option<(String, String)> {
        let ws = self.workspace.as_ref()?;
        let ctx = ctx?;
        let head = ws
            .guard
            .head_revision(&ctx.canonical_repository_path)
            .await
            .ok()?;
        let status = ws
            .guard
            .status_porcelain(&ctx.canonical_repository_path)
            .await
            .unwrap_or_default();
        Some((head, status))
    }

    /// Compare the canonical repository before/after the Agent ran (rule
    /// #8). Never assumes the repo started clean — only flags *new* dirt
    /// that appeared during this execution. Findings are recorded into the
    /// result's metadata for the final report rather than failing the task:
    /// worktree isolation is what structurally prevents this, this is the
    /// verification that it held.
    async fn verify_canonical_untouched(
        &self,
        ctx: &WorkspaceContext,
        before: Option<(String, String)>,
        result: &mut TaskExecutionResult,
    ) {
        let Some(ws) = &self.workspace else { return };
        let Some((head_before, status_before)) = before else {
            return;
        };

        let head_after = ws
            .guard
            .head_revision(&ctx.canonical_repository_path)
            .await
            .unwrap_or_else(|_| "unresolvable".to_string());
        let status_after = ws
            .guard
            .status_porcelain(&ctx.canonical_repository_path)
            .await
            .unwrap_or_default();

        let before_lines: std::collections::HashSet<&str> = status_before.lines().collect();
        let new_dirt: Vec<&str> = status_after
            .lines()
            .filter(|l| !before_lines.contains(l))
            .collect();

        result
            .metadata
            .insert("canonical_head_before".to_string(), head_before.clone());
        result
            .metadata
            .insert("canonical_head_after".to_string(), head_after.clone());
        result.metadata.insert(
            "canonical_repo_contaminated".to_string(),
            (head_after != head_before || !new_dirt.is_empty()).to_string(),
        );

        if head_after != head_before {
            error!(
                task_id = %result.task_id,
                head_before = %head_before,
                head_after = %head_after,
                "CRITICAL: canonical repository HEAD moved during Agent execution — worktree isolation was bypassed"
            );
        }
        if !new_dirt.is_empty() {
            error!(
                task_id = %result.task_id,
                new_dirt = ?new_dirt,
                "CRITICAL: canonical repository gained new uncommitted changes during Agent execution"
            );
        }
    }

    /// Execute the task on a separate tokio task so a panic in the executor
    /// is contained and reported instead of unwinding the loop.
    async fn execute_isolated(
        &self,
        task: &Task,
        workspace: Option<WorkspaceContext>,
    ) -> TaskExecutionResult {
        let executor = Arc::clone(&self.executor);
        let task_for_exec = task.clone();
        let task_id = task.id;
        let started_at = Utc::now();

        match tokio::spawn(async move {
            let request = ExecutionRequest {
                task: task_for_exec,
                workspace,
            };
            executor.execute(&request).await
        })
        .await
        {
            Ok(result) => result,
            Err(join_err) => {
                let reason = if join_err.is_panic() {
                    "executor panicked during execution"
                } else {
                    "executor task was cancelled"
                };
                error!(task_id = %task_id, error = %join_err, "{reason}");
                TaskExecutionResult::failed(task_id, started_at, reason)
            }
        }
    }

    /// Route the execution result into the existing terminal-state APIs.
    async fn finalize(
        &self,
        task: &Task,
        workspace_ctx: Option<WorkspaceContext>,
        mut result: TaskExecutionResult,
    ) {
        let task_id = task.id;
        let project_id = task.project_id;
        let agent_type = result
            .metadata
            .entry("agent".into())
            .or_insert_with(|| {
                self.executor
                    .name()
                    .trim_start_matches("agent:")
                    .to_string()
            })
            .clone();

        // --- RECORD SPEND ---
        // Record the execution cost to the spend ledger BEFORE quality gate.
        // Agent has already been paid; this is accounting, not a failure point.
        if let Some(spend_repo) = &self.spend_repo {
            if let Some(execution_id) = task.execution_id {
                // Extract cost from metadata (set by executor)
                let cost_usd = result
                    .metadata
                    .get("cost_usd")
                    .and_then(|s| s.parse::<f64>().ok());

                let tokens_in = result
                    .metadata
                    .get("agent_tokens_input")
                    .and_then(|s| s.parse::<u32>().ok());

                let tokens_out = result
                    .metadata
                    .get("agent_tokens_output")
                    .and_then(|s| s.parse::<u32>().ok());

                let model = result.metadata.get("model").cloned();
                let session_id = result.metadata.get("agent_session_id").cloned();

                let spend_record = SpendRecord {
                    id: Uuid::new_v4(),
                    task_id,
                    project_id,
                    execution_id: Some(execution_id),
                    attempt_number: task.attempt,
                    agent_type: agent_type.clone(),
                    model: model.unwrap_or_else(|| "unknown".to_string()),
                    provider: "anthropic".to_string(),
                    external_session_id: session_id,
                    input_tokens: tokens_in,
                    output_tokens: tokens_out,
                    total_tokens: tokens_in.and_then(|i| tokens_out.map(|o| i + o)),
                    cost_usd,
                    duration_ms: Some(result.duration().num_milliseconds() as u32),
                    success: result.success,
                    error: result.failure_reason.clone(),
                    created_at: chrono::Utc::now(),
                };

                match spend_repo.record_spend(spend_record).await {
                    Ok(_) => {
                        info!(
                            task_id = %task_id,
                            cost_usd = ?cost_usd,
                            "Spend recorded successfully"
                        );
                    }
                    Err(e) => {
                        // CRITICAL: Spend record failed but agent already executed.
                        // Mark system as DEGRADED to prevent further paid executions.
                        error!(
                            task_id = %task_id,
                            cost_usd = ?cost_usd,
                            error = %e,
                            "CRITICAL: Failed to record spend; system accounting state now DEGRADED"
                        );

                        // Mark accounting as degraded immediately
                        if let Some(accounting_state) = &self.accounting_state {
                            let reason =
                                format!("Failed to record spend for task {}: {}", task_id, e);
                            accounting_state.mark_degraded(reason).await;
                            error!(
                                "Accounting state transitioned to DEGRADED; \
                                 subsequent Paid Agents will be blocked"
                            );
                        }
                    }
                }
            }
        }

        if !result.success {
            self.scheduler
                .submit_task_output(task_id, build_task_output(&result, self.executor.name()))
                .await;
            let reason = result
                .failure_reason
                .clone()
                .unwrap_or_else(|| "execution failed without a reason".to_string());
            let detail = if result.stderr_summary.is_empty() {
                reason
            } else {
                format!("{reason}; stderr: {}", result.stderr_summary)
            };

            // `fail_task` decides retry vs permanent failure and persists both.
            self.scheduler
                .fail_task(
                    task_id,
                    TaskError::new(classify_failure(&result), detail, task.attempt),
                )
                .await;

            self.log_final_state(task_id, task.project_id).await;
            return;
        }

        // Success: go through the mandatory quality gate.
        let output = build_task_output(&result, self.executor.name());
        let quality_input = crate::quality::TaskOutput {
            task_id: task_id.to_string(),
            workspace_dir: result
                .workspace
                .clone()
                .unwrap_or_else(|| PathBuf::from(".")),
            files_modified: result.files_changed.clone(),
            test_commands: task.test_commands.clone(),
            ..Default::default()
        };
        let checker = QualityChecker::new(QualityConfig {
            // Only run stages that are meaningful without an LLM and without
            // a file-level diff from the executor.
            run_tests: !task.test_commands.is_empty(),
            run_code_review: false,
            run_security: true,
            run_lint: false,
            run_build: false,
            run_health_checks: false,
            review_model: None,
        });

        // --- V0.38 Phase 4: Git Checkpoint & Task Artifact ---
        //
        // Only reachable when BOTH Workspace and Checkpoint integration are
        // attached — a checkpoint without a real worktree is meaningless.
        // Absent either, this falls back to the pre-Phase-4 behavior
        // unchanged (`complete_task_with_quality`): quality passing is
        // enough to complete. When present, quality passing is necessary
        // but not sufficient — see `crate::checkpoint::PostQualityCheckpoint`.
        let finalizer = match (&self.workspace, &self.checkpoint, &workspace_ctx) {
            (Some(ws), Some(cp), Some(ctx)) => Some(
                GitCheckpointFinalizer::new(
                    cp.checkpointer.clone(),
                    ws.guard.clone(),
                    ws.repo.clone(),
                    cp.artifacts.clone(),
                    ctx.clone(),
                    task.clone(),
                )
                .with_identity(cp.identity.clone())
                .with_agent_evidence(
                    Some(self.executor.name().to_string()),
                    result.metadata.get("model").cloned(),
                    result.metadata.get("agent_session_id").cloned(),
                ),
            ),
            _ => None,
        };

        let completion = match &finalizer {
            Some(finalizer) => {
                self.scheduler
                    .complete_task_with_checkpoint(
                        task_id,
                        output,
                        &quality_input,
                        &checker,
                        finalizer,
                    )
                    .await
            }
            None => {
                self.scheduler
                    .complete_task_with_quality(task_id, output, &quality_input, &checker)
                    .await
            }
        };

        match completion {
            Ok((report, rollback)) => {
                if report.overall.is_passing() {
                    debug!(task_id = %task_id, checkpoint = finalizer.is_some(), "Quality gate passed");
                    if let Some(harness) = &self.evaluation {
                        if let Err(error) =
                            harness.evaluate_linked_task(task.project_id, task_id).await
                        {
                            // The task artifact stays durable. WorkItem verification
                            // fails closed and can be retried through the API/CLI.
                            error!(%task_id, %error, "WorkItem evaluation requires retry");
                        }
                    }
                } else {
                    warn!(
                        task_id = %task_id,
                        rollback = ?rollback.is_some(),
                        "Quality gate rejected task"
                    );
                }
            }
            Err(e) => {
                // The gate itself errored (e.g. the task could not enter
                // Testing). Record a failure rather than leaving it stuck.
                error!(task_id = %task_id, error = %e, "Quality gate errored; failing task");
                self.scheduler
                    .fail_task(
                        task_id,
                        TaskError::new(
                            TaskErrorType::Unknown,
                            format!("quality gate error: {e}"),
                            task.attempt,
                        ),
                    )
                    .await;
            }
        }

        self.log_final_state(task_id, task.project_id).await;
    }

    async fn log_final_state(&self, task_id: Uuid, project_id: Uuid) {
        let state = self
            .scheduler
            .get_task(task_id)
            .await
            .map(|t| t.state)
            .unwrap_or(TaskState::Pending);
        info!(
            task_id = %task_id,
            project_id = %project_id,
            state = %state,
            terminal = state.is_terminal(),
            "Task state finalized"
        );
    }
}

/// Map an execution result onto the existing task error taxonomy.
fn classify_failure(result: &TaskExecutionResult) -> TaskErrorType {
    match result.failure_reason.as_deref() {
        Some(reason) if reason.contains("panicked") || reason.contains("cancelled") => {
            TaskErrorType::WorkerCrashed
        }
        Some(reason) if reason.contains("executor error") => TaskErrorType::WorkerCrashed,
        Some(reason) if reason.contains("no command") => TaskErrorType::Unknown,
        _ if result.exit_code.is_some() => TaskErrorType::TestsFailed,
        _ => TaskErrorType::Unknown,
    }
}

/// Convert an execution result into the scheduler's `TaskOutput`.
fn build_task_output(result: &TaskExecutionResult, executor_name: &str) -> TaskOutput {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("executor".to_string(), executor_name.to_string());
    metadata.insert(
        "exit_code".to_string(),
        result
            .exit_code
            .map(|c| c.to_string())
            .unwrap_or_else(|| "none".to_string()),
    );
    metadata.insert("started_at".to_string(), result.started_at.to_rfc3339());
    metadata.insert("finished_at".to_string(), result.finished_at.to_rfc3339());
    metadata.insert(
        "duration_ms".to_string(),
        result.duration().num_milliseconds().to_string(),
    );
    if !result.stderr_summary.is_empty() {
        metadata.insert("stderr".to_string(), result.stderr_summary.clone());
    }
    // Executor-supplied evidence (model, cost, tokens, session id...).
    metadata.extend(result.metadata.clone());

    TaskOutput {
        summary: result.stdout_summary.clone(),
        files_changed: result.files_changed.clone(),
        metadata,
        ..Default::default()
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task_types::{Priority, TaskBuilder};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn test_reported_agent_type_is_persisted_to_task_and_spend() {
        for agent in ["codex", "claude_code"] {
            let scheduler = Arc::new(TaskScheduler::new());
            let repo = Arc::new(RecordingSpendRepository::default());
            let mut task = TaskBuilder::new(Uuid::new_v4(), "type trace").build();
            task.max_attempts = 1;
            let id = task.id;
            scheduler.add_task(task).await;
            assert_eq!(scheduler.dequeue_task().await, Some(id));
            assert!(scheduler.transition(id, TaskState::Assigned).await);
            assert!(scheduler.transition(id, TaskState::Running).await);
            let task = scheduler.get_task(id).await.unwrap();
            let (executor, _) = ScriptedExecutor::new(false);
            let mut result = executor
                .execute(&ExecutionRequest {
                    task: task.clone(),
                    workspace: None,
                })
                .await;
            result.metadata.insert("agent".into(), agent.into());
            ExecutionLoop::new(scheduler.clone(), executor, fast_config())
                .with_spend_repo(repo.clone())
                .finalize(&task, None, result)
                .await;
            let saved = scheduler.get_task(id).await.unwrap();
            assert_eq!(
                saved
                    .output
                    .unwrap()
                    .metadata
                    .get("agent")
                    .map(String::as_str),
                Some(agent)
            );
            assert_eq!(repo.task_spend(id).await.unwrap()[0].agent_type, agent);
        }
    }

    #[tokio::test]
    async fn test_failed_task_persists_executor_metadata_and_spend_type() {
        let (repo, task) = recorded_failure("charged failure", false).await;
        let output = task.output.expect("failed execution output persisted");
        assert_eq!(
            output.metadata.get("agent").map(String::as_str),
            Some("charged-test-agent")
        );
        assert_eq!(
            repo.task_spend(task.id).await.unwrap()[0].agent_type,
            "charged-test-agent"
        );
    }

    #[derive(Default)]
    struct RecordingSpendRepository {
        records: std::sync::Mutex<Vec<SpendRecord>>,
    }

    #[async_trait]
    impl SpendRepository for RecordingSpendRepository {
        async fn record_spend(&self, record: SpendRecord) -> crate::types::ArgusResult<bool> {
            self.records.lock().unwrap().push(record);
            Ok(true)
        }

        async fn spent_today_usd(
            &self,
            project_id: Uuid,
        ) -> crate::types::ArgusResult<Option<f64>> {
            Ok(Some(
                self.records
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|record| {
                        record.project_id == project_id
                            && record.created_at.date_naive() == Utc::now().date_naive()
                    })
                    .filter_map(|record| record.cost_usd)
                    .sum(),
            ))
        }

        async fn spent_this_month_usd(
            &self,
            project_id: Uuid,
        ) -> crate::types::ArgusResult<Option<f64>> {
            self.spent_today_usd(project_id).await
        }

        async fn task_spend(&self, task_id: Uuid) -> crate::types::ArgusResult<Vec<SpendRecord>> {
            Ok(self
                .records
                .lock()
                .unwrap()
                .iter()
                .filter(|record| record.task_id == task_id)
                .cloned()
                .collect())
        }

        async fn execution_spend(
            &self,
            execution_id: Uuid,
        ) -> crate::types::ArgusResult<Vec<SpendRecord>> {
            Ok(self
                .records
                .lock()
                .unwrap()
                .iter()
                .filter(|record| record.execution_id == Some(execution_id))
                .cloned()
                .collect())
        }

        async fn get_spend(
            &self,
            spend_id: Uuid,
        ) -> crate::types::ArgusResult<Option<SpendRecord>> {
            Ok(self
                .records
                .lock()
                .unwrap()
                .iter()
                .find(|record| record.id == spend_id)
                .cloned())
        }
    }

    struct ChargedFailureExecutor {
        reason: &'static str,
        timeout: bool,
    }

    #[async_trait]
    impl TaskExecutor for ChargedFailureExecutor {
        fn name(&self) -> &str {
            "charged-test-agent"
        }

        async fn execute(&self, request: &ExecutionRequest) -> TaskExecutionResult {
            let started = Utc::now();
            if self.timeout {
                assert!(
                    tokio::time::timeout(request.task.timeout, std::future::pending::<()>())
                        .await
                        .is_err()
                );
            }
            let mut result = TaskExecutionResult::failed(request.task.id, started, self.reason);
            result.metadata.insert("cost_usd".into(), "2.0".into());
            result
        }
    }

    async fn recorded_failure(
        reason: &'static str,
        timeout: bool,
    ) -> (Arc<RecordingSpendRepository>, Task) {
        let scheduler = Arc::new(TaskScheduler::new());
        let repo = Arc::new(RecordingSpendRepository::default());
        let mut task = TaskBuilder::new(Uuid::new_v4(), "charged failure").build();
        task.max_attempts = 1;
        task.timeout = Duration::from_millis(1);
        let task_id = task.id;
        scheduler.add_task(task).await;
        let handle = ExecutionLoop::new(
            scheduler.clone(),
            Arc::new(ChargedFailureExecutor { reason, timeout }),
            fast_config(),
        )
        .with_spend_repo(repo.clone())
        .spawn();
        assert!(
            wait_for_state(
                &scheduler,
                task_id,
                TaskState::Failed,
                Duration::from_secs(5)
            )
            .await
        );
        handle.shutdown().await;
        (repo, scheduler.get_task(task_id).await.unwrap())
    }

    #[tokio::test]
    async fn test_failed_execution_records_unsuccessful_spend() {
        let (repo, task) = recorded_failure("command exited with status 1", false).await;
        let records = repo.task_spend(task.id).await.unwrap();
        assert_eq!(records.len(), 1);
        assert!(!records[0].success);
        assert_eq!(records[0].cost_usd, Some(2.0));
        assert_eq!(records[0].execution_id, task.execution_id);
        assert_eq!(records[0].attempt_number, task.attempt);
        assert_eq!(
            records[0].error.as_deref(),
            Some("command exited with status 1")
        );
    }

    #[tokio::test]
    async fn test_timed_out_execution_records_unsuccessful_spend() {
        let (repo, task) = recorded_failure("agent execution timed out", true).await;
        let records = repo.task_spend(task.id).await.unwrap();
        assert_eq!(records.len(), 1);
        assert!(!records[0].success);
        assert_eq!(records[0].cost_usd, Some(2.0));
        assert_eq!(
            records[0].error.as_deref(),
            Some("agent execution timed out")
        );
    }

    #[tokio::test]
    async fn test_zero_change_execution_records_unsuccessful_spend() {
        let (repo, task) = recorded_failure(
            "agent completed without modifying any file in the repository",
            false,
        )
        .await;
        let records = repo.task_spend(task.id).await.unwrap();
        assert_eq!(records.len(), 1);
        assert!(!records[0].success);
        assert_eq!(records[0].cost_usd, Some(2.0));
    }

    #[tokio::test]
    async fn test_failed_execution_exhausts_daily_and_task_budgets() {
        use crate::budget_service::{BudgetDecision, BudgetPolicy};
        let (repo, task) = recorded_failure("charged failure", false).await;
        for (daily, per_task, expected) in [
            (Some(1.0), None, "daily budget exhausted"),
            (None, Some(1.0), "task budget exhausted"),
        ] {
            let service = BudgetService::new(
                repo.clone(),
                BudgetPolicy {
                    daily_budget_usd: daily,
                    max_per_task_usd: per_task,
                    strict_mode: true,
                },
            );
            let decision = service
                .preflight_check(task.project_id, task.id, None)
                .await;
            assert!(
                matches!(decision, BudgetDecision::Blocked { ref reason, .. } if reason.contains(expected)),
                "{decision:?}"
            );
        }
    }

    /// Deterministic executor for loop-level tests — no shell, no filesystem.
    struct ScriptedExecutor {
        calls: Arc<AtomicUsize>,
        succeed: bool,
    }

    impl ScriptedExecutor {
        fn new(succeed: bool) -> (Arc<Self>, Arc<AtomicUsize>) {
            let calls = Arc::new(AtomicUsize::new(0));
            (
                Arc::new(Self {
                    calls: Arc::clone(&calls),
                    succeed,
                }),
                calls,
            )
        }
    }

    #[async_trait]
    impl TaskExecutor for ScriptedExecutor {
        fn name(&self) -> &str {
            "scripted-test-executor"
        }

        async fn execute(&self, request: &ExecutionRequest) -> TaskExecutionResult {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let task = &request.task;
            let started_at = Utc::now();
            if self.succeed {
                TaskExecutionResult {
                    task_id: task.id,
                    success: true,
                    exit_code: Some(0),
                    stdout_summary: format!("ran {}", task.title),
                    stderr_summary: String::new(),
                    started_at,
                    finished_at: Utc::now(),
                    failure_reason: None,
                    files_changed: vec!["scripted.rs".to_string()],
                    workspace: None,
                    metadata: std::collections::HashMap::new(),
                    workspace_id: None,
                    branch_name: None,
                    base_revision: None,
                    worktree_path: None,
                }
            } else {
                TaskExecutionResult {
                    task_id: task.id,
                    success: false,
                    exit_code: Some(1),
                    stdout_summary: String::new(),
                    stderr_summary: "scripted failure".to_string(),
                    started_at,
                    finished_at: Utc::now(),
                    failure_reason: Some("command exited with status 1".to_string()),
                    files_changed: Vec::new(),
                    workspace: None,
                    metadata: std::collections::HashMap::new(),
                    workspace_id: None,
                    branch_name: None,
                    base_revision: None,
                    worktree_path: None,
                }
            }
        }
    }

    /// Executor that panics, to prove the loop survives it.
    struct PanickingExecutor;

    #[async_trait]
    impl TaskExecutor for PanickingExecutor {
        fn name(&self) -> &str {
            "panicking-test-executor"
        }
        async fn execute(&self, _request: &ExecutionRequest) -> TaskExecutionResult {
            panic!("intentional executor panic");
        }
    }

    async fn wait_for_state(
        scheduler: &TaskScheduler,
        task_id: Uuid,
        want: TaskState,
        timeout: Duration,
    ) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        while tokio::time::Instant::now() < deadline {
            if scheduler.get_task(task_id).await.map(|t| t.state) == Some(want) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        false
    }

    fn fast_config() -> ExecutionLoopConfig {
        ExecutionLoopConfig {
            idle_interval: Duration::from_millis(10),
            maintenance_interval: Duration::from_millis(50),
        }
    }

    #[tokio::test]
    async fn test_loop_drives_task_to_completed() {
        let scheduler = Arc::new(TaskScheduler::new());
        let (executor, calls) = ScriptedExecutor::new(true);

        let task = TaskBuilder::new(Uuid::new_v4(), "in-memory success")
            .priority(Priority::High)
            .build();
        let task_id = task.id;
        scheduler.add_task(task).await;

        let handle = ExecutionLoop::new(Arc::clone(&scheduler), executor, fast_config()).spawn();

        assert!(
            wait_for_state(
                &scheduler,
                task_id,
                TaskState::Completed,
                Duration::from_secs(5)
            )
            .await,
            "task should reach Completed without any manual dequeue"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1, "executor ran exactly once");

        let done = scheduler.get_task(task_id).await.unwrap();
        let output = done.output.expect("execution output recorded");
        assert_eq!(output.summary, "ran in-memory success");
        assert_eq!(
            output.metadata.get("exit_code").map(String::as_str),
            Some("0")
        );

        handle.shutdown().await;
    }

    // TODO: test_loop_marks_failure_and_keeps_running disabled — TaskBuilder::max_attempts() doesn't exist
    // This test should be updated to work with the current Task structure once max_attempts
    // is available through the builder or set directly on the Task.
    #[tokio::test]
    async fn test_loop_marks_failure_and_keeps_running_disabled() {
        let scheduler = Arc::new(TaskScheduler::new());
        let (executor, calls) = ScriptedExecutor::new(false);

        // max_attempts(1) => no retry backoff, straight to Failed.
        let mut failing = TaskBuilder::new(Uuid::new_v4(), "always fails").build();
        failing.max_attempts = 1; // Set directly for now
        let failing_id = failing.id;
        scheduler.add_task(failing).await;

        let handle = ExecutionLoop::new(Arc::clone(&scheduler), executor, fast_config()).spawn();

        assert!(
            wait_for_state(
                &scheduler,
                failing_id,
                TaskState::Failed,
                Duration::from_secs(5)
            )
            .await,
            "failing task should reach Failed"
        );

        let failed = scheduler.get_task(failing_id).await.unwrap();
        let err = failed.error.expect("failure reason recorded");
        assert!(
            err.message.contains("status 1"),
            "failure reason should carry the exit status, got: {}",
            err.message
        );

        // The loop must still be alive and pick up the next task.
        let mut follow_up = TaskBuilder::new(Uuid::new_v4(), "after failure").build();
        follow_up.max_attempts = 1; // Set directly for now
        let follow_up_id = follow_up.id;
        scheduler.add_task(follow_up).await;

        assert!(
            wait_for_state(
                &scheduler,
                follow_up_id,
                TaskState::Failed,
                Duration::from_secs(5)
            )
            .await,
            "loop should keep processing after a task failure"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(!handle.is_finished(), "loop must not exit on task failure");

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn test_loop_survives_executor_panic() {
        let scheduler = Arc::new(TaskScheduler::new());

        let mut task = TaskBuilder::new(Uuid::new_v4(), "panics").build();
        task.max_attempts = 1; // Set directly for now
        let task_id = task.id;
        scheduler.add_task(task).await;

        let handle = ExecutionLoop::new(
            Arc::clone(&scheduler),
            Arc::new(PanickingExecutor),
            fast_config(),
        )
        .spawn();

        assert!(
            wait_for_state(
                &scheduler,
                task_id,
                TaskState::Failed,
                Duration::from_secs(5)
            )
            .await,
            "panicking task should be recorded as Failed"
        );
        let failed = scheduler.get_task(task_id).await.unwrap();
        assert!(
            failed
                .error
                .as_ref()
                .is_some_and(|e| e.message.contains("panicked")),
            "panic should be recorded as the failure reason"
        );
        assert!(!handle.is_finished(), "loop must survive an executor panic");

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn test_loop_processes_tasks_sequentially_without_duplicates() {
        let scheduler = Arc::new(TaskScheduler::new());
        let (executor, calls) = ScriptedExecutor::new(true);

        let mut ids = Vec::new();
        for i in 0..5 {
            let task = TaskBuilder::new(Uuid::new_v4(), format!("batch task {i}")).build();
            ids.push(task.id);
            scheduler.add_task(task).await;
        }

        let handle = ExecutionLoop::new(Arc::clone(&scheduler), executor, fast_config()).spawn();

        for id in &ids {
            assert!(
                wait_for_state(
                    &scheduler,
                    *id,
                    TaskState::Completed,
                    Duration::from_secs(10)
                )
                .await,
                "task {id} should complete"
            );
        }

        // Exactly one execution per task: nothing dropped, nothing repeated.
        assert_eq!(calls.load(Ordering::SeqCst), ids.len());
        assert_eq!(scheduler.queue_len().await, 0);

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn test_shutdown_stops_the_loop() {
        let scheduler = Arc::new(TaskScheduler::new());
        let (executor, _) = ScriptedExecutor::new(true);
        let handle = ExecutionLoop::new(
            Arc::clone(&scheduler),
            executor,
            ExecutionLoopConfig {
                // Deliberately long idle sleep: shutdown must interrupt it
                // rather than wait it out.
                idle_interval: Duration::from_secs(30),
                maintenance_interval: Duration::from_secs(30),
            },
        )
        .spawn();

        tokio::time::sleep(Duration::from_millis(50)).await;
        tokio::time::timeout(Duration::from_secs(2), handle.shutdown())
            .await
            .expect("shutdown must not block on the idle interval");
    }

    // TODO: ShellTaskExecutor tests removed — the executor has been replaced
    // with NoOpTaskExecutor. In production, this will be AgentExecutor.
    // The loop logic is tested through TaskExecutor trait tests above.
}
