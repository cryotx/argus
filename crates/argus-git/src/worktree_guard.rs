//! [`WorkspaceGuard`] implementation backed by the real `git` CLI via
//! [`RepoManager`].
//!
//! This is the only place in the V0.38 Phase 3 chain that shells out to
//! `git` for worktree lifecycle/verification — `ExecutionLoop` and the
//! workspace orchestrator only ever call trait methods.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio::process::Command;

use argus_core::types::{ArgusError, ArgusResult};
use argus_core::workspace_manager::{WorkspaceGuard, WorktreeFacts};

use crate::repo::RepoManager;

/// Real Git implementation of [`WorkspaceGuard`].
#[derive(Clone, Copy, Debug, Default)]
pub struct GitWorkspaceGuard;

impl GitWorkspaceGuard {
    pub fn new() -> Self {
        Self
    }
}

/// Run a blocking [`RepoManager`] call on the blocking thread pool so it
/// never stalls the async executor loop. `pub(crate)` so `checkpoint.rs`
/// (the other module in this crate that wraps sync `RepoManager` calls for
/// async callers) can reuse it instead of duplicating the same three lines.
pub(crate) async fn blocking<F, T>(f: F) -> ArgusResult<T>
where
    F: FnOnce() -> ArgusResult<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ArgusError::Git(format!("blocking git task panicked: {e}")))?
}

#[async_trait]
impl WorkspaceGuard for GitWorkspaceGuard {
    async fn ensure_canonical_repo(&self, canonical_repository_path: &Path) -> ArgusResult<()> {
        let path = canonical_repository_path.to_path_buf();
        tokio::fs::create_dir_all(&path)
            .await
            .map_err(|e| ArgusError::Git(format!("failed to create canonical repo dir: {e}")))?;

        if path.join(".git").exists() {
            return Ok(());
        }

        let out = Command::new("git")
            .args(["init", "-q"])
            .current_dir(&path)
            .output()
            .await
            .map_err(|e| ArgusError::Git(format!("failed to run git init: {e}")))?;
        if !out.status.success() {
            return Err(ArgusError::Git(format!(
                "git init failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }

        for (k, v) in [
            ("user.name", "Argus Agent"),
            ("user.email", "argus@localhost"),
        ] {
            let _ = Command::new("git")
                .args(["config", k, v])
                .current_dir(&path)
                .output()
                .await;
        }

        // A brand-new repo has no commits, so HEAD does not resolve — and a
        // base_revision that can't be resolved means no worktree can ever
        // be created from it. Guarantee a resolvable HEAD up front.
        let has_head = Command::new("git")
            .args(["rev-parse", "--verify", "-q", "HEAD"])
            .current_dir(&path)
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !has_head {
            let out = Command::new("git")
                .args([
                    "commit",
                    "--allow-empty",
                    "-q",
                    "-m",
                    "Argus: initial commit",
                ])
                .current_dir(&path)
                .output()
                .await
                .map_err(|e| ArgusError::Git(format!("failed to run git commit: {e}")))?;
            if !out.status.success() {
                return Err(ArgusError::Git(format!(
                    "failed to create initial commit: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                )));
            }
        }

        Ok(())
    }

    async fn head_revision(&self, path: &Path) -> ArgusResult<String> {
        let path = path.to_path_buf();
        blocking(move || RepoManager::head_revision_at(&path)).await
    }

    async fn status_porcelain(&self, path: &Path) -> ArgusResult<String> {
        let path = path.to_path_buf();
        let out = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&path)
            .output()
            .await
            .map_err(|e| ArgusError::Git(format!("git status failed: {e}")))?;
        if !out.status.success() {
            return Err(ArgusError::Git(format!(
                "git status failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }

    async fn ensure_worktree(
        &self,
        canonical_repository_path: &Path,
        worktree_path: &Path,
        branch_name: &str,
        base_revision: &str,
    ) -> ArgusResult<()> {
        // Idempotent: a valid, already-owned worktree means a prior attempt
        // (e.g. before a budget block or accounting-degraded requeue)
        // already created it. Do not touch Git state again — this is the
        // property that #13/#14/#24 depend on: no second worktree for the
        // same execution.
        if self
            .verify_worktree(canonical_repository_path, worktree_path)
            .await
            .is_ok()
        {
            return Ok(());
        }

        let repo_path = canonical_repository_path.to_path_buf();
        let worktree_path = worktree_path.to_path_buf();
        let branch_name = branch_name.to_string();
        let base_revision = base_revision.to_string();

        blocking(move || {
            RepoManager::new(&repo_path).create_worktree(
                &worktree_path,
                &branch_name,
                Some(base_revision.as_str()),
            )
        })
        .await
    }

    async fn verify_worktree(
        &self,
        canonical_repository_path: &Path,
        worktree_path: &Path,
    ) -> ArgusResult<WorktreeFacts> {
        if !worktree_path.is_dir() {
            return Err(ArgusError::Workspace(format!(
                "worktree path {} does not exist",
                worktree_path.display()
            )));
        }
        if !RepoManager::is_linked_worktree(worktree_path) {
            return Err(ArgusError::Workspace(format!(
                "{} is not a Git worktree (.git is not a linked-worktree pointer file)",
                worktree_path.display()
            )));
        }

        let canonical = canonical_repository_path.to_path_buf();
        let wt = worktree_path.to_path_buf();
        let owns = blocking({
            let wt = wt.clone();
            move || RepoManager::new(&canonical).owns_worktree(&wt)
        })
        .await?;
        if !owns {
            return Err(ArgusError::Workspace(format!(
                "{} is not registered as a worktree of {}",
                worktree_path.display(),
                canonical_repository_path.display()
            )));
        }

        let head_path = wt.clone();
        let head = blocking(move || RepoManager::head_revision_at(&head_path)).await?;

        let branch_path = wt.clone();
        let branch = blocking(move || RepoManager::current_branch_at(&branch_path)).await?;

        Ok(WorktreeFacts { head, branch })
    }

    async fn files_changed(&self, worktree_path: &Path) -> ArgusResult<Vec<String>> {
        let path = worktree_path.to_path_buf();
        blocking(move || RepoManager::changed_files_at(&path)).await
    }
}

/// Convenience: canonical-per-project repository path, matching the layout
/// `AgentTaskExecutor` has always used (`<root>/<project_id>`).
pub fn canonical_repo_path(canonical_repo_root: &Path, project_id: uuid::Uuid) -> PathBuf {
    canonical_repo_root.join(project_id.to_string())
}
