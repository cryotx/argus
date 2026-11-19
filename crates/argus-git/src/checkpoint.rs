//! [`GitCheckpointer`] implementation backed by the real `git` CLI via
//! [`RepoManager`] — the V0.38 Phase 4 counterpart to
//! [`crate::worktree_guard::GitWorkspaceGuard`]. Every method here operates
//! only on the worktree path it is given; none of them ever touch the
//! canonical repository.

use std::path::Path;

use async_trait::async_trait;
use uuid::Uuid;

use argus_core::checkpoint::{CommitIdentity, GitCheckpointer};
use argus_core::types::ArgusResult;

use crate::repo::RepoManager;
use crate::worktree_guard::blocking;

/// Real Git implementation of [`GitCheckpointer`].
#[derive(Clone, Copy, Debug, Default)]
pub struct GitCommitCheckpointer;

impl GitCommitCheckpointer {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl GitCheckpointer for GitCommitCheckpointer {
    async fn stage_all(&self, worktree_path: &Path) -> ArgusResult<()> {
        let path = worktree_path.to_path_buf();
        blocking(move || RepoManager::new(&path).stage_all()).await
    }

    async fn has_staged_changes(&self, worktree_path: &Path) -> ArgusResult<bool> {
        let path = worktree_path.to_path_buf();
        blocking(move || RepoManager::new(&path).has_staged_changes()).await
    }

    async fn commit(
        &self,
        worktree_path: &Path,
        identity: &CommitIdentity,
        message: &str,
    ) -> ArgusResult<String> {
        let path = worktree_path.to_path_buf();
        let name = identity.name.clone();
        let email = identity.email.clone();
        let message = message.to_string();
        blocking(move || RepoManager::new(&path).commit_with_identity(&name, &email, &message))
            .await
    }

    async fn find_existing_checkpoint(
        &self,
        worktree_path: &Path,
        execution_id: Uuid,
    ) -> ArgusResult<Option<String>> {
        let path = worktree_path.to_path_buf();
        // Matches the trailer `build_commit_message` (argus-core) writes:
        // `Execution-ID: <uuid>`. A fixed-string, not regex, search — the
        // UUID needs no escaping and this can never be tricked by
        // grep-metacharacters in a task title, since the title never
        // reaches this search string.
        let needle = format!("Execution-ID: {execution_id}");
        blocking(move || RepoManager::new(&path).find_commit_by_message_substring(&needle)).await
    }

    async fn files_in_commit(
        &self,
        worktree_path: &Path,
        base_revision: &str,
        commit_sha: &str,
    ) -> ArgusResult<Vec<String>> {
        let path = worktree_path.to_path_buf();
        let base = base_revision.to_string();
        let head = commit_sha.to_string();
        blocking(move || RepoManager::new(&path).diff_name_only(&base, &head)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn init_repo(dir: &Path) {
        Command::new("git")
            .arg("init")
            .arg("-q")
            .current_dir(dir)
            .status()
            .unwrap();
        for (k, v) in [("user.name", "Test"), ("user.email", "test@example.com")] {
            Command::new("git")
                .args(["config", k, v])
                .current_dir(dir)
                .status()
                .unwrap();
        }
        std::fs::write(dir.join("README.md"), "base\n").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(dir)
            .status()
            .unwrap();
        Command::new("git")
            .args(["commit", "-q", "-m", "base commit"])
            .current_dir(dir)
            .status()
            .unwrap();
    }

    #[tokio::test]
    async fn test_no_staged_changes_reports_false_not_error() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        let checkpointer = GitCommitCheckpointer::new();
        assert!(!checkpointer.has_staged_changes(dir.path()).await.unwrap());
    }

    #[tokio::test]
    async fn test_commit_with_identity_and_find_existing_checkpoint_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        let checkpointer = GitCommitCheckpointer::new();
        let execution_id = Uuid::new_v4();

        std::fs::write(dir.path().join("feature.txt"), "hello\n").unwrap();
        checkpointer.stage_all(dir.path()).await.unwrap();
        assert!(checkpointer.has_staged_changes(dir.path()).await.unwrap());

        let identity = CommitIdentity {
            name: "ARGUS Agent".to_string(),
            email: "argus@localhost".to_string(),
        };
        let message = format!("argus(task-abc12345): test\n\nExecution-ID: {execution_id}\n");
        let sha = checkpointer
            .commit(dir.path(), &identity, &message)
            .await
            .expect("commit should succeed");
        assert_eq!(sha.len(), 40, "should return a full commit SHA");

        // Nothing left staged after a real commit.
        assert!(!checkpointer.has_staged_changes(dir.path()).await.unwrap());

        let found = checkpointer
            .find_existing_checkpoint(dir.path(), execution_id)
            .await
            .unwrap();
        assert_eq!(found, Some(sha.clone()));

        // A different execution id must not match this commit.
        let not_found = checkpointer
            .find_existing_checkpoint(dir.path(), Uuid::new_v4())
            .await
            .unwrap();
        assert_eq!(not_found, None);

        // Confirm identity was applied, not inherited from ambient config.
        let show = Command::new("git")
            .args(["show", "-s", "--format=%an <%ae>", &sha])
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&show.stdout).trim(),
            "ARGUS Agent <argus@localhost>"
        );
    }

    #[tokio::test]
    async fn test_files_in_commit_reports_the_commits_actual_diff() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        let repo = RepoManager::new(dir.path());
        let base = repo.get_head_revision().unwrap();

        std::fs::write(dir.path().join("a.txt"), "a\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), "b\n").unwrap();
        let unusual = "带 空格\n文件.txt";
        std::fs::write(dir.path().join(unusual), "content\n").unwrap();
        repo.stage_all().unwrap();
        let head = repo
            .commit_with_identity("T", "t@example.com", "two files")
            .unwrap();

        let checkpointer = GitCommitCheckpointer::new();
        let mut files = checkpointer
            .files_in_commit(dir.path(), &base, &head)
            .await
            .unwrap();
        files.sort();
        assert_eq!(
            files,
            vec![
                "a.txt".to_string(),
                "b.txt".to_string(),
                unusual.to_string()
            ]
        );
    }
}
