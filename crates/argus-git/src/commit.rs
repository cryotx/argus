//! Commit workflow using git CLI.

use std::process::Command;

use argus_core::types::{ArgusError, ArgusResult};

use crate::repo::RepoManager;

/// Commit manager.
pub struct CommitManager {
    repo: RepoManager,
}

impl CommitManager {
    /// Create a new commit manager.
    pub fn new(repo: RepoManager) -> Self {
        Self { repo }
    }

    /// Stage all changes.
    pub fn stage_all(&self) -> ArgusResult<()> {
        let status = Command::new("git")
            .arg("add")
            .arg(".")
            .current_dir(&self.repo.repo_path)
            .status()?;

        if status.success() {
            Ok(())
        } else {
            Err(ArgusError::Git("git add failed".to_string()))
        }
    }

    /// Create a commit with the given message.
    pub fn commit(&self, message: &str) -> ArgusResult<()> {
        let status = Command::new("git")
            .arg("commit")
            .arg("-m")
            .arg(message)
            .current_dir(&self.repo.repo_path)
            .status()?;

        if status.success() {
            Ok(())
        } else {
            Err(ArgusError::Git("git commit failed".to_string()))
        }
    }
}
