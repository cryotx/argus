//! V0.38 Phase 4 — Git Checkpoint & Task Artifact.
//!
//! Closes the loop the rest of V0.38 built up to:
//!
//! ```text
//! Agent execution -> Spend accounting -> Quality Gate -> Git Checkpoint
//!     -> Artifact persist -> Workspace Completed -> Task Completed
//! ```
//!
//! # Why this is a separate module from `execution_loop`
//!
//! [`PostQualityCheckpoint`] is invoked by `TaskScheduler::complete_task_with_checkpoint`
//! *between* a passing Quality Gate report and the `Testing -> Completed`
//! transition — the one place in the whole pipeline where "the Agent's work
//! is verified" and "the task is allowed to finish" are not yet the same
//! thing. A task must never reach `Completed` ahead of its Artifact
//! (Architecture Principle 12); this module is the gate that enforces it.
//!
//! # Layering
//!
//! Exactly like [`crate::workspace_manager::WorkspaceGuard`]: `argus-core`
//! defines the [`GitCheckpointer`] trait (the raw Git mechanics this needs —
//! stage, commit, look up an existing checkpoint, diff a commit), and
//! `argus-git` implements it against the real `git` CLI. `argus-core` never
//! depends on `argus-git`, so the dependency graph stays acyclic.

use std::collections::HashMap;
use std::path::Path;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tracing::{error, warn};
use uuid::Uuid;

use crate::artifact_repository::{ArtifactRecord, ArtifactRepository, NewArtifact};
use crate::quality::{QualityReport, QualityResult};
use crate::task_types::{Task, TaskErrorType};
use crate::types::ArgusResult;
use crate::workspace_manager::{WorkspaceContext, WorkspaceGuard};
use crate::workspace_repository::WorkspaceRepository;

// ============================================================
// Raw Git checkpoint mechanics — implemented in argus-git
// ============================================================

/// Explicit commit identity for ARGUS-authored checkpoint commits.
///
/// Never derived from ambient/global Git config (rule #8): a checkpoint
/// commit's authorship must not depend on whatever happens to be configured
/// on the machine running the daemon.
#[derive(Clone, Debug)]
pub struct CommitIdentity {
    pub name: String,
    pub email: String,
}

impl Default for CommitIdentity {
    fn default() -> Self {
        Self {
            name: "ARGUS Agent".to_string(),
            email: "argus@localhost".to_string(),
        }
    }
}

/// The raw Git operations a checkpoint needs, scoped to a single worktree.
/// Never touches the canonical repository — every method takes the
/// worktree path explicitly and only that path.
#[async_trait]
pub trait GitCheckpointer: Send + Sync {
    /// `git add .` in the worktree — `.gitignore`-respecting, so Agent
    /// scratch files and ARGUS runtime metadata that are already ignored
    /// never get staged.
    async fn stage_all(&self, worktree_path: &Path) -> ArgusResult<()>;

    /// Whether anything is actually staged for commit. This is the check
    /// that keeps ARGUS from ever needing `--allow-empty`: no staged
    /// changes means there is nothing valid to checkpoint, full stop.
    async fn has_staged_changes(&self, worktree_path: &Path) -> ArgusResult<bool>;

    /// Commit whatever is currently staged, with an explicit identity
    /// (never ambient config) and message, returning the resulting commit
    /// SHA.
    async fn commit(
        &self,
        worktree_path: &Path,
        identity: &CommitIdentity,
        message: &str,
    ) -> ArgusResult<String>;

    /// Walk this worktree's commit history (from HEAD) for a commit whose
    /// message already carries the `Execution-ID: <execution_id>` trailer —
    /// i.e. a checkpoint commit for this exact execution already exists.
    /// This is what makes `ensure_git_checkpoint` idempotent across a crash
    /// between "commit succeeded" and "artifact persisted" (rule #16):
    /// a finalize replay finds and reuses it instead of committing again.
    async fn find_existing_checkpoint(
        &self,
        worktree_path: &Path,
        execution_id: Uuid,
    ) -> ArgusResult<Option<String>>;

    /// Files that differ between `base_revision` and `commit_sha` — the
    /// authoritative content of a checkpoint commit, computed from Git
    /// itself rather than trusted from executor-reported state.
    async fn files_in_commit(
        &self,
        worktree_path: &Path,
        base_revision: &str,
        commit_sha: &str,
    ) -> ArgusResult<Vec<String>>;
}

/// A completed Git checkpoint — the structured result of
/// [`GitCheckpointFinalizer::run`], never just a raw stdout string.
#[derive(Clone, Debug)]
pub struct GitCheckpoint {
    pub commit_sha: String,
    pub branch_name: String,
    pub base_revision: String,
    pub files_committed: Vec<String>,
    pub created_at: DateTime<Utc>,
}

// ============================================================
// Post-quality-gate checkpoint hook
// ============================================================

/// Fields to merge into a task's final `TaskOutput` once its checkpoint
/// exists — set here, not by the executor, since the commit (and the
/// Artifact it produces) doesn't exist until after the Quality Gate passes.
#[derive(Clone, Debug, Default)]
pub struct CheckpointOutputPatch {
    pub commit_sha: Option<String>,
    pub metadata: HashMap<String, String>,
}

/// Why a post-quality checkpoint did not complete. The task fails (subject
/// to the scheduler's normal retry policy via `fail_task`) — but, unlike a
/// Quality Gate failure, nothing is rolled back: only the checkpoint step
/// itself did not succeed, and the Agent's verified worktree changes are
/// preserved exactly as-is for the next attempt (or a human) to inspect.
#[derive(Clone, Debug)]
pub struct CheckpointFailure {
    pub error_type: TaskErrorType,
    pub reason: String,
}

/// Hook invoked by `TaskScheduler::complete_task_with_checkpoint` after the
/// Quality Gate has passed and before a task is allowed to reach
/// `Completed`. `argus-core` depends only on this trait; the concrete
/// implementation ([`GitCheckpointFinalizer`]) is constructed per-execution
/// by `ExecutionLoop`, which has the `WorkspaceContext` and Agent evidence
/// this needs.
#[async_trait]
pub trait PostQualityCheckpoint: Send + Sync {
    async fn run(
        &self,
        task_id: Uuid,
        report: &QualityReport,
    ) -> Result<Option<CheckpointOutputPatch>, CheckpointFailure>;
}

/// Build the trailer-carrying commit message every checkpoint commit uses.
/// Deliberately carries only ids and the task's own (short, user-authored)
/// title — never a full prompt, secret, or auth token (rule #9).
fn build_commit_message(task: &Task, execution_id: Uuid, workspace_id: Uuid) -> String {
    let short_id = task.id.simple().to_string();
    let short_id = &short_id[..8.min(short_id.len())];
    format!(
        "argus(task-{short_id}): {title}\n\nTask-ID: {task_id}\nExecution-ID: {execution_id}\nWorkspace-ID: {workspace_id}\n",
        title = task.title,
        task_id = task.id,
    )
}

/// Human-readable one-line summary of a `QualityReport`, stored as Artifact
/// evidence. Full stage-by-stage detail stays in the task's own quality
/// report; this is just enough to answer "why was this allowed to ship"
/// without re-running anything (rule #24 — deliberately not a full
/// Evaluation Harness, which is Planned in `docs/MASTER_ROADMAP.md`).
fn summarize_quality(report: &QualityReport) -> String {
    let stage_summary = report
        .stages
        .iter()
        .map(|s| format!("{:?}={:?}", s.stage, s.result))
        .collect::<Vec<_>>()
        .join(", ");
    if stage_summary.is_empty() {
        format!("{:?}", report.overall)
    } else {
        format!("{:?}; stages: {stage_summary}", report.overall)
    }
}

fn quality_status_label(report: &QualityReport) -> &'static str {
    match &report.overall {
        QualityResult::Passed => "passed",
        QualityResult::PassedWithWarnings { .. } => "passed_with_warnings",
        QualityResult::Failed { .. } => "failed",
        QualityResult::Indeterminate { .. } => "indeterminate",
    }
}

/// Orchestrates one execution's Git Checkpoint + Artifact, constructed
/// fresh per execution by `ExecutionLoop` (it closes over that execution's
/// `WorkspaceContext`, `Task`, and Agent evidence — there is no shared,
/// long-lived state to reason about across executions).
pub struct GitCheckpointFinalizer {
    checkpointer: std::sync::Arc<dyn GitCheckpointer>,
    workspace_guard: std::sync::Arc<dyn WorkspaceGuard>,
    workspace_repo: std::sync::Arc<dyn WorkspaceRepository>,
    artifacts: std::sync::Arc<dyn ArtifactRepository>,
    ctx: WorkspaceContext,
    task: Task,
    identity: CommitIdentity,
    agent_runtime: Option<String>,
    model: Option<String>,
    session_id: Option<String>,
}

impl GitCheckpointFinalizer {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        checkpointer: std::sync::Arc<dyn GitCheckpointer>,
        workspace_guard: std::sync::Arc<dyn WorkspaceGuard>,
        workspace_repo: std::sync::Arc<dyn WorkspaceRepository>,
        artifacts: std::sync::Arc<dyn ArtifactRepository>,
        ctx: WorkspaceContext,
        task: Task,
    ) -> Self {
        Self {
            checkpointer,
            workspace_guard,
            workspace_repo,
            artifacts,
            ctx,
            task,
            identity: CommitIdentity::default(),
            agent_runtime: None,
            model: None,
            session_id: None,
        }
    }

    /// Override the commit identity (defaults to `ARGUS Agent <argus@localhost>`).
    pub fn with_identity(mut self, identity: CommitIdentity) -> Self {
        self.identity = identity;
        self
    }

    /// Attach Agent evidence (driver name, model, external session id) for
    /// the Artifact record. All best-effort — not every driver reports all
    /// three.
    pub fn with_agent_evidence(
        mut self,
        agent_runtime: Option<String>,
        model: Option<String>,
        session_id: Option<String>,
    ) -> Self {
        self.agent_runtime = agent_runtime;
        self.model = model;
        self.session_id = session_id;
        self
    }

    fn patch_from_artifact(&self, artifact: &ArtifactRecord) -> CheckpointOutputPatch {
        let mut metadata = HashMap::new();
        metadata.insert("artifact_id".to_string(), artifact.artifact_id.to_string());
        metadata.insert("branch_name".to_string(), artifact.branch_name.clone());
        metadata.insert("base_revision".to_string(), artifact.base_revision.clone());
        CheckpointOutputPatch {
            commit_sha: Some(artifact.commit_sha.clone()),
            metadata,
        }
    }
}

#[async_trait]
impl PostQualityCheckpoint for GitCheckpointFinalizer {
    async fn run(
        &self,
        task_id: Uuid,
        report: &QualityReport,
    ) -> Result<Option<CheckpointOutputPatch>, CheckpointFailure> {
        if self.task.id != task_id {
            return Err(CheckpointFailure {
                error_type: TaskErrorType::Unknown,
                reason: format!(
                    "checkpoint finalizer task mismatch: constructed for {}, called for {}",
                    self.task.id, task_id
                ),
            });
        }

        // --- Rule #19 fast path: an Artifact for this execution already
        // exists (a prior finalize call already completed the whole
        // checkpoint — including Crash Window C, where the commit and
        // Artifact both succeeded but the daemon crashed before the task
        // itself reached Completed). Reuse it verbatim; never commit or
        // persist a second one. Idempotently (re-)mark the workspace
        // Completed too, in case that step is what a prior crash skipped.
        if let Ok(Some(existing)) = self
            .artifacts
            .get_by_execution_id(self.ctx.execution_id)
            .await
        {
            if let Err(e) = self
                .workspace_repo
                .mark_completed(self.ctx.workspace_id)
                .await
            {
                warn!(
                    workspace_id = %self.ctx.workspace_id,
                    error = %e,
                    "failed to (re-)mark workspace completed on artifact-replay path"
                );
            }
            return Ok(Some(self.patch_from_artifact(&existing)));
        }

        // --- Rule #5/#6: re-verify ownership and worktree validity
        // immediately before touching Git, exactly like the spawn-boundary
        // re-verification in `ExecutionLoop` — nothing may have changed,
        // but this is not a step that gets trusted-by-construction.
        if let Err(e) = self
            .ctx
            .validate_ownership(&self.task, self.ctx.execution_id)
        {
            return Err(CheckpointFailure {
                error_type: TaskErrorType::SecurityViolation,
                reason: e.to_string(),
            });
        }
        let facts = match self
            .workspace_guard
            .verify_worktree(&self.ctx.canonical_repository_path, &self.ctx.worktree_path)
            .await
        {
            Ok(facts) => facts,
            Err(e) => {
                return Err(CheckpointFailure {
                    error_type: TaskErrorType::CheckpointFailed,
                    reason: format!("worktree verification failed before checkpoint: {e}"),
                });
            }
        };
        if facts.branch.as_deref() != Some(self.ctx.branch_name.as_str()) {
            return Err(CheckpointFailure {
                error_type: TaskErrorType::SecurityViolation,
                reason: format!(
                    "branch mismatch before checkpoint: worktree is on {:?}, expected {}",
                    facts.branch, self.ctx.branch_name
                ),
            });
        }

        // --- Rule #16 (Crash Window A): a checkpoint commit for this exact
        // execution may already exist on disk even though no Artifact was
        // ever persisted for it (daemon crashed between commit and DB
        // insert). Find and reuse it rather than staging/committing again.
        let commit_sha = match self
            .checkpointer
            .find_existing_checkpoint(&self.ctx.worktree_path, self.ctx.execution_id)
            .await
        {
            Ok(Some(sha)) => sha,
            Ok(None) => match self.create_new_checkpoint().await {
                Ok(sha) => sha,
                Err(failure) => return Err(failure),
            },
            Err(e) => {
                return Err(CheckpointFailure {
                    error_type: TaskErrorType::CheckpointFailed,
                    reason: format!("failed to check for an existing checkpoint commit: {e}"),
                });
            }
        };

        let files_changed = self
            .checkpointer
            .files_in_commit(
                &self.ctx.worktree_path,
                &self.ctx.base_revision,
                &commit_sha,
            )
            .await
            .unwrap_or_default();

        let new_artifact = NewArtifact {
            project_id: self.ctx.project_id,
            task_id: self.ctx.task_id,
            execution_id: self.ctx.execution_id,
            workspace_id: self.ctx.workspace_id,
            base_revision: self.ctx.base_revision.clone(),
            branch_name: self.ctx.branch_name.clone(),
            commit_sha: commit_sha.clone(),
            files_changed,
            quality_status: Some(quality_status_label(report).to_string()),
            quality_summary: Some(summarize_quality(report)),
            agent_runtime: self.agent_runtime.clone(),
            model: self.model.clone(),
            session_id: self.session_id.clone(),
        };

        let artifact = match self
            .artifacts
            .ensure_git_checkpoint_artifact(new_artifact)
            .await
        {
            Ok(a) => a,
            Err(e) => {
                // --- Rule #22 (Crash Window B, or a plain DB failure): the
                // commit is real and durable in Git, but its Artifact record
                // is not. Mark the workspace RecoveryNeeded — never
                // Completed — so a human sees it, and so a blind retry that
                // skipped straight to a fresh commit is never the recovery
                // path: `find_existing_checkpoint` above will find this
                // exact commit next time and reuse it instead.
                let reason = format!(
                    "git commit {commit_sha} succeeded but artifact persistence failed: {e}"
                );
                if let Err(mark_err) = self
                    .workspace_repo
                    .mark_recovery_needed(self.ctx.workspace_id, &reason)
                    .await
                {
                    error!(
                        workspace_id = %self.ctx.workspace_id,
                        error = %mark_err,
                        "failed to mark workspace recovery_needed after artifact persistence failure"
                    );
                }
                return Err(CheckpointFailure {
                    error_type: TaskErrorType::CheckpointFailed,
                    reason,
                });
            }
        };

        // Only now — the Artifact is durably persisted — may the workspace
        // complete. A failure here is logged, not fatal: the Artifact (not
        // the workspace row) is the source of truth for "this execution
        // shipped", and the next `list_recoverable`/reconciliation pass (or
        // a future finalize replay, via the fast path above) can still
        // catch a workspace stuck short of `completed`.
        if let Err(e) = self
            .workspace_repo
            .mark_completed(self.ctx.workspace_id)
            .await
        {
            warn!(
                workspace_id = %self.ctx.workspace_id,
                error = %e,
                "artifact persisted but failed to mark workspace completed"
            );
        }

        Ok(Some(self.patch_from_artifact(&artifact)))
    }
}

impl GitCheckpointFinalizer {
    /// The "no prior commit exists" path: stage, refuse an empty checkpoint,
    /// commit with explicit identity and a traceable message.
    async fn create_new_checkpoint(&self) -> Result<String, CheckpointFailure> {
        if let Err(e) = self.checkpointer.stage_all(&self.ctx.worktree_path).await {
            return Err(CheckpointFailure {
                error_type: TaskErrorType::CheckpointFailed,
                reason: format!("git add failed: {e}"),
            });
        }

        let has_changes = self
            .checkpointer
            .has_staged_changes(&self.ctx.worktree_path)
            .await
            .map_err(|e| CheckpointFailure {
                error_type: TaskErrorType::CheckpointFailed,
                reason: format!("failed to check for staged changes: {e}"),
            })?;
        if !has_changes {
            // Rule #4: never `--allow-empty`. A quality-passing task with no
            // actual worktree changes is a distinct failure/no-change path,
            // not a silent Completed.
            return Err(CheckpointFailure {
                error_type: TaskErrorType::CheckpointFailed,
                reason: "quality gate passed but no changes exist in the worktree to checkpoint"
                    .to_string(),
            });
        }

        let message =
            build_commit_message(&self.task, self.ctx.execution_id, self.ctx.workspace_id);
        self.checkpointer
            .commit(&self.ctx.worktree_path, &self.identity, &message)
            .await
            .map_err(|e| CheckpointFailure {
                error_type: TaskErrorType::CheckpointFailed,
                reason: format!("git commit failed: {e}"),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task_types::{TaskBuilder, TaskState};
    use crate::workspace_manager::WorktreeFacts;
    use std::collections::HashMap as StdHashMap;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::Mutex;
    use tokio::sync::Mutex as AsyncMutex;

    fn passing_report(task_id: Uuid) -> QualityReport {
        QualityReport {
            task_id: task_id.to_string(),
            overall: QualityResult::Passed,
            stages: vec![],
            total_duration_ms: 0,
            recommendations: vec![],
        }
    }

    /// A fixed project id shared by `test_ctx` and `test_task` in every
    /// test below — `WorkspaceContext::validate_ownership` (correctly)
    /// rejects a workspace and task that disagree about which project
    /// they belong to, so the two fixtures must never be built with
    /// independently-random project ids.
    fn test_project_id() -> Uuid {
        Uuid::from_u128(0xa9c5_9e11_0000_0000_0000_0000_0000_0001)
    }

    fn test_ctx(task_id: Uuid, execution_id: Uuid, workspace_id: Uuid) -> WorkspaceContext {
        WorkspaceContext {
            workspace_id,
            project_id: test_project_id(),
            task_id,
            execution_id,
            canonical_repository_path: PathBuf::from("/tmp/argus-test/canonical"),
            worktree_path: PathBuf::from("/tmp/argus-test/worktree"),
            branch_name: "argus/task/x/y".to_string(),
            base_revision: "base000".to_string(),
        }
    }

    /// In-memory fake covering every `GitCheckpointer` call a test needs to
    /// script deterministically — commit failure, an existing on-disk
    /// commit (Crash Window A), or a normal fresh commit.
    #[derive(Default)]
    struct FakeCheckpointer {
        has_staged: Mutex<bool>,
        existing_commit: Mutex<Option<String>>,
        commit_result: Mutex<Option<Result<String, String>>>,
        commit_calls: Mutex<u32>,
    }

    #[async_trait]
    impl GitCheckpointer for FakeCheckpointer {
        async fn stage_all(&self, _worktree_path: &Path) -> ArgusResult<()> {
            Ok(())
        }
        async fn has_staged_changes(&self, _worktree_path: &Path) -> ArgusResult<bool> {
            Ok(*self.has_staged.lock().unwrap())
        }
        async fn commit(
            &self,
            _worktree_path: &Path,
            _identity: &CommitIdentity,
            _message: &str,
        ) -> ArgusResult<String> {
            *self.commit_calls.lock().unwrap() += 1;
            match self.commit_result.lock().unwrap().clone() {
                Some(Ok(sha)) => Ok(sha),
                Some(Err(e)) => Err(crate::types::ArgusError::Git(e)),
                None => Ok("deadbeef".to_string()),
            }
        }
        async fn find_existing_checkpoint(
            &self,
            _worktree_path: &Path,
            _execution_id: Uuid,
        ) -> ArgusResult<Option<String>> {
            Ok(self.existing_commit.lock().unwrap().clone())
        }
        async fn files_in_commit(
            &self,
            _worktree_path: &Path,
            _base_revision: &str,
            _commit_sha: &str,
        ) -> ArgusResult<Vec<String>> {
            Ok(vec!["feature.txt".to_string()])
        }
    }

    /// Always reports the worktree as valid and on the expected branch.
    struct FakeGuardAlwaysValid;
    #[async_trait]
    impl WorkspaceGuard for FakeGuardAlwaysValid {
        async fn ensure_canonical_repo(&self, _p: &Path) -> ArgusResult<()> {
            Ok(())
        }
        async fn head_revision(&self, _p: &Path) -> ArgusResult<String> {
            Ok("base000".to_string())
        }
        async fn status_porcelain(&self, _p: &Path) -> ArgusResult<String> {
            Ok(String::new())
        }
        async fn ensure_worktree(
            &self,
            _c: &Path,
            _w: &Path,
            _b: &str,
            _base: &str,
        ) -> ArgusResult<()> {
            Ok(())
        }
        async fn verify_worktree(&self, _c: &Path, _w: &Path) -> ArgusResult<WorktreeFacts> {
            Ok(WorktreeFacts {
                head: "base000".to_string(),
                branch: Some("argus/task/x/y".to_string()),
            })
        }
        async fn files_changed(&self, _w: &Path) -> ArgusResult<Vec<String>> {
            Ok(vec!["feature.txt".to_string()])
        }
    }

    /// In-memory `ArtifactRepository` fake with a scriptable failure mode,
    /// so "commit succeeded but persistence failed" (rule #22) is testable
    /// deterministically without a real database.
    #[derive(Default)]
    struct FakeArtifacts {
        by_execution: AsyncMutex<StdHashMap<Uuid, ArtifactRecord>>,
        fail_next_ensure: Mutex<bool>,
    }

    #[async_trait]
    impl ArtifactRepository for FakeArtifacts {
        async fn ensure_git_checkpoint_artifact(
            &self,
            artifact: NewArtifact,
        ) -> ArgusResult<ArtifactRecord> {
            if *self.fail_next_ensure.lock().unwrap() {
                *self.fail_next_ensure.lock().unwrap() = false;
                return Err(crate::types::ArgusError::Database(sqlx::Error::PoolClosed));
            }
            let mut map = self.by_execution.lock().await;
            if let Some(existing) = map.get(&artifact.execution_id) {
                return Ok(existing.clone());
            }
            let record = ArtifactRecord {
                artifact_id: Uuid::new_v4(),
                artifact_type: "git_commit".to_string(),
                project_id: artifact.project_id,
                task_id: artifact.task_id,
                execution_id: artifact.execution_id,
                workspace_id: artifact.workspace_id,
                base_revision: artifact.base_revision,
                branch_name: artifact.branch_name,
                commit_sha: artifact.commit_sha,
                files_changed: artifact.files_changed,
                quality_status: artifact.quality_status,
                quality_summary: artifact.quality_summary,
                agent_runtime: artifact.agent_runtime,
                model: artifact.model,
                session_id: artifact.session_id,
                created_at: Utc::now(),
            };
            map.insert(artifact.execution_id, record.clone());
            Ok(record)
        }

        async fn get_by_execution_id(
            &self,
            execution_id: Uuid,
        ) -> ArgusResult<Option<ArtifactRecord>> {
            Ok(self.by_execution.lock().await.get(&execution_id).cloned())
        }

        async fn get_by_task_id(&self, task_id: Uuid) -> ArgusResult<Vec<ArtifactRecord>> {
            Ok(self
                .by_execution
                .lock()
                .await
                .values()
                .filter(|a| a.task_id == task_id)
                .cloned()
                .collect())
        }
    }

    /// In-memory `WorkspaceRepository` fake, just enough to observe which
    /// terminal call the finalizer made.
    #[derive(Default)]
    struct FakeWorkspaceRepo {
        completed: Mutex<u32>,
        recovery_needed: Mutex<u32>,
        last_reason: Mutex<Option<String>>,
    }

    #[async_trait]
    impl WorkspaceRepository for FakeWorkspaceRepo {
        async fn ensure_workspace(
            &self,
            _execution_id: Uuid,
            _project_id: Uuid,
            _task_id: Uuid,
            _repository_path: String,
            _worktree_path: String,
            _branch_name: String,
            _base_revision: String,
        ) -> ArgusResult<crate::workspace_repository::WorkspaceRecord> {
            unimplemented!("not exercised by checkpoint tests")
        }
        async fn get_workspace(
            &self,
            _workspace_id: Uuid,
        ) -> ArgusResult<Option<crate::workspace_repository::WorkspaceRecord>> {
            Ok(None)
        }
        async fn get_by_execution_id(
            &self,
            _execution_id: Uuid,
        ) -> ArgusResult<Option<crate::workspace_repository::WorkspaceRecord>> {
            Ok(None)
        }
        async fn update_status(
            &self,
            _workspace_id: Uuid,
            status: &str,
            failure_reason: Option<&str>,
        ) -> ArgusResult<()> {
            if status == "recovery_needed" {
                *self.recovery_needed.lock().unwrap() += 1;
                *self.last_reason.lock().unwrap() = failure_reason.map(str::to_string);
            }
            Ok(())
        }
        async fn mark_active(&self, _workspace_id: Uuid) -> ArgusResult<()> {
            Ok(())
        }
        async fn mark_completed(&self, _workspace_id: Uuid) -> ArgusResult<()> {
            *self.completed.lock().unwrap() += 1;
            Ok(())
        }
        async fn mark_failed(&self, _workspace_id: Uuid, _reason: &str) -> ArgusResult<()> {
            Ok(())
        }
        async fn list_recoverable(
            &self,
        ) -> ArgusResult<Vec<crate::workspace_repository::WorkspaceRecord>> {
            Ok(vec![])
        }
        async fn get_active_for_task(
            &self,
            _task_id: Uuid,
        ) -> ArgusResult<Option<crate::workspace_repository::WorkspaceRecord>> {
            Ok(None)
        }
    }

    fn test_task(task_id: Uuid) -> Task {
        let mut task = TaskBuilder::new(test_project_id(), "checkpoint test task").build();
        task.id = task_id;
        task.state = TaskState::Testing;
        task
    }

    #[tokio::test]
    async fn test_successful_checkpoint_creates_artifact_and_completes_workspace() {
        let task_id = Uuid::new_v4();
        let execution_id = Uuid::new_v4();
        let workspace_id = Uuid::new_v4();
        let ctx = test_ctx(task_id, execution_id, workspace_id);

        let checkpointer = Arc::new(FakeCheckpointer::default());
        *checkpointer.has_staged.lock().unwrap() = true;
        *checkpointer.commit_result.lock().unwrap() = Some(Ok("commit1".to_string()));

        let artifacts = Arc::new(FakeArtifacts::default());
        let workspace_repo = Arc::new(FakeWorkspaceRepo::default());

        let finalizer = GitCheckpointFinalizer::new(
            checkpointer.clone(),
            Arc::new(FakeGuardAlwaysValid),
            workspace_repo.clone(),
            artifacts.clone(),
            ctx,
            test_task(task_id),
        );

        let patch = finalizer
            .run(task_id, &passing_report(task_id))
            .await
            .expect("checkpoint should succeed")
            .expect("patch should be present");

        assert_eq!(patch.commit_sha.as_deref(), Some("commit1"));
        assert_eq!(*workspace_repo.completed.lock().unwrap(), 1);
        assert_eq!(*checkpointer.commit_calls.lock().unwrap(), 1);
        assert!(
            artifacts
                .get_by_execution_id(execution_id)
                .await
                .unwrap()
                .is_some(),
            "artifact must be persisted"
        );
    }

    #[tokio::test]
    async fn test_no_staged_changes_fails_without_allow_empty() {
        let task_id = Uuid::new_v4();
        let execution_id = Uuid::new_v4();
        let ctx = test_ctx(task_id, execution_id, Uuid::new_v4());

        let checkpointer = Arc::new(FakeCheckpointer::default());
        *checkpointer.has_staged.lock().unwrap() = false; // nothing to commit

        let finalizer = GitCheckpointFinalizer::new(
            checkpointer.clone(),
            Arc::new(FakeGuardAlwaysValid),
            Arc::new(FakeWorkspaceRepo::default()),
            Arc::new(FakeArtifacts::default()),
            ctx,
            test_task(task_id),
        );

        let result = finalizer.run(task_id, &passing_report(task_id)).await;
        assert!(result.is_err(), "no changes must not silently succeed");
        assert_eq!(
            *checkpointer.commit_calls.lock().unwrap(),
            0,
            "must never commit"
        );
        let failure = result.unwrap_err();
        assert!(failure.reason.contains("no changes"));
    }

    #[tokio::test]
    async fn test_commit_failure_does_not_complete_task_or_persist_artifact() {
        let task_id = Uuid::new_v4();
        let execution_id = Uuid::new_v4();
        let ctx = test_ctx(task_id, execution_id, Uuid::new_v4());

        let checkpointer = Arc::new(FakeCheckpointer::default());
        *checkpointer.has_staged.lock().unwrap() = true;
        *checkpointer.commit_result.lock().unwrap() = Some(Err("lock file exists".to_string()));

        let artifacts = Arc::new(FakeArtifacts::default());
        let workspace_repo = Arc::new(FakeWorkspaceRepo::default());

        let finalizer = GitCheckpointFinalizer::new(
            checkpointer,
            Arc::new(FakeGuardAlwaysValid),
            workspace_repo.clone(),
            artifacts.clone(),
            ctx,
            test_task(task_id),
        );

        let result = finalizer.run(task_id, &passing_report(task_id)).await;
        assert!(result.is_err());
        assert_eq!(*workspace_repo.completed.lock().unwrap(), 0);
        assert!(
            artifacts
                .get_by_execution_id(execution_id)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn test_artifact_persistence_failure_marks_recovery_needed_not_completed() {
        let task_id = Uuid::new_v4();
        let execution_id = Uuid::new_v4();
        let ctx = test_ctx(task_id, execution_id, Uuid::new_v4());

        let checkpointer = Arc::new(FakeCheckpointer::default());
        *checkpointer.has_staged.lock().unwrap() = true;
        *checkpointer.commit_result.lock().unwrap() = Some(Ok("commit-crash-b".to_string()));

        let artifacts = Arc::new(FakeArtifacts::default());
        *artifacts.fail_next_ensure.lock().unwrap() = true;
        let workspace_repo = Arc::new(FakeWorkspaceRepo::default());

        let finalizer = GitCheckpointFinalizer::new(
            checkpointer.clone(),
            Arc::new(FakeGuardAlwaysValid),
            workspace_repo.clone(),
            artifacts.clone(),
            ctx.clone(),
            test_task(task_id),
        );

        let result = finalizer.run(task_id, &passing_report(task_id)).await;
        assert!(
            result.is_err(),
            "artifact persistence failure must fail the task"
        );
        assert_eq!(*workspace_repo.completed.lock().unwrap(), 0);
        assert_eq!(*workspace_repo.recovery_needed.lock().unwrap(), 1);
        assert!(
            workspace_repo
                .last_reason
                .lock()
                .unwrap()
                .as_deref()
                .unwrap()
                .contains("commit-crash-b"),
            "recovery reason should name the surviving commit"
        );

        // --- Crash Window B recovery: a retry must find the SAME commit via
        // `find_existing_checkpoint` (simulated here by scripting the fake
        // to now report it) rather than creating a second one, then persist
        // the artifact and complete normally.
        *checkpointer.existing_commit.lock().unwrap() = Some("commit-crash-b".to_string());
        let commit_calls_before = *checkpointer.commit_calls.lock().unwrap();

        let retry_finalizer = GitCheckpointFinalizer::new(
            checkpointer.clone(),
            Arc::new(FakeGuardAlwaysValid),
            workspace_repo.clone(),
            artifacts.clone(),
            ctx,
            test_task(task_id),
        );
        let patch = retry_finalizer
            .run(task_id, &passing_report(task_id))
            .await
            .expect("retry should succeed")
            .expect("patch present");
        assert_eq!(patch.commit_sha.as_deref(), Some("commit-crash-b"));
        assert_eq!(
            *checkpointer.commit_calls.lock().unwrap(),
            commit_calls_before,
            "retry must reuse the existing commit, never create a second one"
        );
        assert_eq!(*workspace_repo.completed.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn test_idempotent_finalize_replay_reuses_same_artifact_and_commit() {
        let task_id = Uuid::new_v4();
        let execution_id = Uuid::new_v4();
        let ctx = test_ctx(task_id, execution_id, Uuid::new_v4());

        let checkpointer = Arc::new(FakeCheckpointer::default());
        *checkpointer.has_staged.lock().unwrap() = true;
        *checkpointer.commit_result.lock().unwrap() = Some(Ok("only-commit".to_string()));
        let artifacts = Arc::new(FakeArtifacts::default());
        let workspace_repo = Arc::new(FakeWorkspaceRepo::default());

        let build = || {
            GitCheckpointFinalizer::new(
                checkpointer.clone(),
                Arc::new(FakeGuardAlwaysValid),
                workspace_repo.clone(),
                artifacts.clone(),
                ctx.clone(),
                test_task(task_id),
            )
        };

        let first = build()
            .run(task_id, &passing_report(task_id))
            .await
            .unwrap()
            .unwrap();
        let second = build()
            .run(task_id, &passing_report(task_id))
            .await
            .unwrap()
            .unwrap();

        assert_eq!(first.commit_sha, second.commit_sha);
        assert_eq!(
            first.metadata.get("artifact_id"),
            second.metadata.get("artifact_id"),
            "replay must return the SAME artifact id, not a new one"
        );
        assert_eq!(
            *checkpointer.commit_calls.lock().unwrap(),
            1,
            "only the first call may actually commit"
        );
        assert_eq!(
            artifacts.get_by_task_id(task_id).await.unwrap().len(),
            1,
            "exactly one artifact must exist after a replayed finalize"
        );
    }

    #[test]
    fn test_commit_message_carries_trailers_not_secrets() {
        let task_id = Uuid::new_v4();
        let execution_id = Uuid::new_v4();
        let workspace_id = Uuid::new_v4();
        let mut task = TaskBuilder::new(Uuid::new_v4(), "Add a greet function").build();
        task.id = task_id;

        let message = build_commit_message(&task, execution_id, workspace_id);
        assert!(message.starts_with("argus(task-"));
        assert!(message.contains("Add a greet function"));
        assert!(message.contains(&format!("Task-ID: {task_id}")));
        assert!(message.contains(&format!("Execution-ID: {execution_id}")));
        assert!(message.contains(&format!("Workspace-ID: {workspace_id}")));
    }
}
