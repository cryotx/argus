//! V0.38 Workspace isolation manager
//!
//! Handles creation, tracking, and cleanup of Git worktrees for task isolation.
//! Each Agent execution gets its own isolated Git worktree to prevent conflicts.
//!
//! # Phase 3 — fail-closed execution boundary
//!
//! Everything in this module exists to answer one question before an Agent
//! is ever spawned: *is this really an isolated, verified Git worktree that
//! belongs to this exact task execution?* If the answer is anything but a
//! clean "yes", the caller (`ExecutionLoop`) must not spawn the Agent and
//! must never fall back to the canonical repository. See [`WorkspaceGuard`]
//! for the verification surface and [`WorkspaceManager`] for path-safety.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::task_types::Task;
use crate::types::{ArgusError, ArgusResult};

/// Runtime context for a task execution's isolated workspace.
/// Passed through ExecutionLoop to Executor and beyond.
#[derive(Clone, Debug)]
pub struct WorkspaceContext {
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub task_id: Uuid,
    pub execution_id: Uuid,
    pub canonical_repository_path: PathBuf,
    pub worktree_path: PathBuf,
    pub branch_name: String,
    pub base_revision: String,
}

impl WorkspaceContext {
    /// Ownership re-check at the execution boundary (Phase 3 rule #15).
    ///
    /// A `WorkspaceRepository` lookup already scoped this record once; this
    /// is the belt-and-suspenders check performed again immediately before
    /// an Agent is spawned, so a stale or mismatched record can never put an
    /// Agent to work under the wrong task's identity.
    pub fn validate_ownership(&self, task: &Task, execution_id: Uuid) -> ArgusResult<()> {
        if self.project_id != task.project_id {
            return Err(ArgusError::Workspace(format!(
                "workspace ownership mismatch: workspace.project_id={} != task.project_id={}",
                self.project_id, task.project_id
            )));
        }
        if self.task_id != task.id {
            return Err(ArgusError::Workspace(format!(
                "workspace ownership mismatch: workspace.task_id={} != task.id={}",
                self.task_id, task.id
            )));
        }
        if self.execution_id != execution_id {
            return Err(ArgusError::Workspace(format!(
                "workspace ownership mismatch: workspace.execution_id={} != execution.execution_id={}",
                self.execution_id, execution_id
            )));
        }
        Ok(())
    }

    /// Executor cwd re-check: the directory the executor is about to run in
    /// must be exactly the worktree this context names — never the canonical
    /// repository, never some other execution's worktree.
    pub fn validate_executor_cwd(&self, cwd: &Path) -> ArgusResult<()> {
        if cwd != self.worktree_path {
            return Err(ArgusError::Workspace(format!(
                "executor cwd {} does not match workspace worktree_path {}",
                cwd.display(),
                self.worktree_path.display()
            )));
        }
        Ok(())
    }
}

/// Facts resolved by inspecting a live worktree on disk — never trusted from
/// a database record alone.
#[derive(Clone, Debug)]
pub struct WorktreeFacts {
    pub head: String,
    pub branch: Option<String>,
}

/// Abstraction over the Git operations `ExecutionLoop` needs to create and
/// verify a task worktree, without scattering raw `git` shell-outs through
/// the loop itself. Implemented in `argus-git` (which already owns the Git
/// CLI wrapper); `argus-core` only depends on this trait so the dependency
/// graph stays acyclic.
#[async_trait]
pub trait WorkspaceGuard: Send + Sync {
    /// Make sure the canonical per-project repository exists and is a valid
    /// Git repository (creating and `git init`-ing it on first use).
    async fn ensure_canonical_repo(&self, canonical_repository_path: &Path) -> ArgusResult<()>;

    /// HEAD commit SHA at `path` (canonical repo or worktree).
    async fn head_revision(&self, path: &Path) -> ArgusResult<String>;

    /// `git status --porcelain` output at `path`, for before/after dirty
    /// comparisons. Empty string means clean.
    async fn status_porcelain(&self, path: &Path) -> ArgusResult<String>;

    /// Idempotently ensure a linked worktree exists at `worktree_path`,
    /// branched from `canonical_repository_path` at `base_revision` as
    /// `branch_name`. No-ops (does not touch Git state) if a valid worktree
    /// already exists at that path — this is what makes budget-blocked and
    /// accounting-degraded requeues reuse rather than duplicate a worktree.
    #[allow(clippy::too_many_arguments)]
    async fn ensure_worktree(
        &self,
        canonical_repository_path: &Path,
        worktree_path: &Path,
        branch_name: &str,
        base_revision: &str,
    ) -> ArgusResult<()>;

    /// Verify `worktree_path` is a real, live Git worktree that Git itself
    /// considers to belong to `canonical_repository_path`, and that its HEAD
    /// resolves. This is what stands between "a directory that happens to
    /// exist" and "a worktree we can trust an Agent to run in".
    async fn verify_worktree(
        &self,
        canonical_repository_path: &Path,
        worktree_path: &Path,
    ) -> ArgusResult<WorktreeFacts>;

    /// Files changed in the worktree at `worktree_path`: tracked diffs and
    /// untracked files, `.gitignore`-respecting. Never scans anything outside
    /// this worktree.
    async fn files_changed(&self, worktree_path: &Path) -> ArgusResult<Vec<String>>;
}

/// Workspace lifecycle status
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceStatus {
    /// Initial state, preparing workspace
    Preparing,
    /// Workspace active, task executing
    Active,
    /// Task completed successfully
    Completed,
    /// Task failed
    Failed,
    /// Workspace orphaned (inconsistent state)
    Orphaned,
    /// Needs recovery (crashed during execution)
    RecoveryNeeded,
}

impl std::fmt::Display for WorkspaceStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Preparing => write!(f, "preparing"),
            Self::Active => write!(f, "active"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
            Self::Orphaned => write!(f, "orphaned"),
            Self::RecoveryNeeded => write!(f, "recovery_needed"),
        }
    }
}

/// Configuration for workspace management
#[derive(Clone, Debug)]
pub struct WorkspaceConfig {
    /// Root directory where worktrees are created
    pub worktree_root: PathBuf,
    /// Whether to keep worktrees after completion (for debugging)
    pub keep_completed: bool,
}

impl WorkspaceConfig {
    pub fn new(worktree_root: impl Into<PathBuf>) -> Self {
        Self {
            worktree_root: worktree_root.into(),
            keep_completed: true, // Default: keep for debugging
        }
    }

    /// Validate that the worktree root is safe (no directory escape attacks)
    pub fn validate(&self) -> ArgusResult<()> {
        // Create before canonicalizing — `canonicalize()` requires the path
        // to already exist, so checking existence *after* trying to
        // canonicalize (the previous order here) meant this always failed
        // on a fresh root, which is exactly the case a first-ever `argus
        // run` hits.
        if !self.worktree_root.exists() {
            std::fs::create_dir_all(&self.worktree_root).map_err(|e| {
                ArgusError::Workspace(format!("Failed to create worktree root: {}", e))
            })?;
        }

        self.worktree_root.canonicalize().map_err(|e| {
            ArgusError::Workspace(format!("Failed to canonicalize worktree root: {}", e))
        })?;

        Ok(())
    }
}

/// Metadata for a single task workspace
#[derive(Clone, Debug)]
pub struct WorkspaceMetadata {
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub task_id: Uuid,
    pub execution_id: Uuid,
    pub repository_path: PathBuf,
    pub worktree_path: PathBuf,
    pub branch_name: String,
    pub base_revision: String,
    pub status: WorkspaceStatus,
    pub failure_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// Workspace manager for task isolation
pub struct WorkspaceManager {
    config: WorkspaceConfig,
}

impl WorkspaceManager {
    pub fn new(config: WorkspaceConfig) -> Self {
        Self { config }
    }

    /// Validate workspace path doesn't escape the root
    pub fn validate_path(&self, path: &Path) -> ArgusResult<()> {
        let path = path
            .canonicalize()
            .map_err(|e| ArgusError::Workspace(format!("Invalid path: {}", e)))?;

        let root = self
            .config
            .worktree_root
            .canonicalize()
            .map_err(|e| ArgusError::Workspace(format!("Invalid root: {}", e)))?;

        if !path.starts_with(&root) {
            return Err(ArgusError::Workspace(
                "Workspace path escapes root directory".to_string(),
            ));
        }

        Ok(())
    }

    /// Generate deterministic worktree path for an execution
    pub fn worktree_path(&self, project_id: Uuid, execution_id: Uuid) -> PathBuf {
        self.config
            .worktree_root
            .join(format!("{}", project_id))
            .join(format!("{}", execution_id))
    }

    /// Generate branch name for a task
    pub fn branch_name(&self, task_id: Uuid, execution_id: Uuid) -> String {
        format!("argus/task/{}/{}", task_id, execution_id)
    }

    /// Root-containment + identity checks that don't require the path to
    /// exist yet (rule #16, pre-creation half).
    ///
    /// `worktree_path` must be *exactly* the deterministic path this
    /// manager would generate for `(project_id, execution_id)` — task
    /// payloads are never consulted for worktree placement, so this single
    /// equality check is what rules out `..` escapes and arbitrary
    /// user-specified directories: there is no code path by which anything
    /// but this manager's own `worktree_path()` output can satisfy it.
    pub fn validate_worktree_assignment(
        &self,
        project_id: Uuid,
        execution_id: Uuid,
        worktree_path: &Path,
        canonical_repository_path: &Path,
    ) -> ArgusResult<()> {
        let expected = self.worktree_path(project_id, execution_id);
        if worktree_path != expected {
            return Err(ArgusError::Workspace(format!(
                "worktree path {} is not the deterministic path {} for this execution",
                worktree_path.display(),
                expected.display()
            )));
        }

        if worktree_path == canonical_repository_path {
            return Err(ArgusError::Workspace(
                "worktree path equals canonical repository path".to_string(),
            ));
        }

        Ok(())
    }

    /// Post-creation containment check (rule #16, second half): once the
    /// worktree exists, canonicalize it — resolving any symlinks — and
    /// confirm it still lands under the configured worktree root and still
    /// differs from the canonical repository. Catches a symlink swapped in
    /// after the fact that `validate_worktree_assignment` cannot see.
    pub fn validate_worktree_no_escape(
        &self,
        worktree_path: &Path,
        canonical_repository_path: &Path,
    ) -> ArgusResult<()> {
        let root = self
            .config
            .worktree_root
            .canonicalize()
            .map_err(|e| ArgusError::Workspace(format!("invalid worktree root: {}", e)))?;
        let candidate = worktree_path
            .canonicalize()
            .map_err(|e| ArgusError::Workspace(format!("worktree path does not exist: {}", e)))?;

        if !candidate.starts_with(&root) {
            return Err(ArgusError::Workspace(format!(
                "worktree path {} escapes configured root {} (possible symlink escape)",
                candidate.display(),
                root.display()
            )));
        }

        if let Ok(canon_repo) = canonical_repository_path.canonicalize() {
            if candidate == canon_repo {
                return Err(ArgusError::Workspace(
                    "worktree path resolves to the canonical repository path".to_string(),
                ));
            }
        }

        Ok(())
    }

    /// Validate a workspace metadata object for safety
    pub fn validate_metadata(&self, meta: &WorkspaceMetadata) -> ArgusResult<()> {
        // Validate path is within worktree root
        self.validate_path(&meta.worktree_path)?;

        // Validate branch name is safe
        if meta.branch_name.contains("../") || meta.branch_name.contains("..\\") {
            return Err(ArgusError::Workspace(
                "Branch name contains path traversal".to_string(),
            ));
        }

        // Validate base_revision looks like a commit SHA
        if meta.base_revision.len() != 40 && meta.base_revision.len() != 7 {
            return Err(ArgusError::Workspace(format!(
                "Invalid base revision: {}",
                meta.base_revision
            )));
        }

        Ok(())
    }
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self {
            worktree_root: PathBuf::from("/workspaces"),
            keep_completed: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workspace_path_safety() {
        let config = WorkspaceConfig::new("/tmp/argus-workspaces");
        let manager = WorkspaceManager::new(config);

        let project_id = Uuid::new_v4();
        let execution_id = Uuid::new_v4();

        let path = manager.worktree_path(project_id, execution_id);
        assert!(path.starts_with("/tmp/argus-workspaces"));
        assert!(path.to_string_lossy().contains(&project_id.to_string()));
    }

    #[test]
    fn test_branch_name_format() {
        let config = WorkspaceConfig::new("/tmp/argus-workspaces");
        let manager = WorkspaceManager::new(config);

        let task_id = Uuid::new_v4();
        let execution_id = Uuid::new_v4();

        let branch = manager.branch_name(task_id, execution_id);
        assert!(branch.contains("argus/task"));
        assert!(branch.contains(&task_id.to_string()));
        assert!(branch.contains(&execution_id.to_string()));
    }
}
