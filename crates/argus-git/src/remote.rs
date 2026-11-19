//! Remote operations using git CLI.

use std::process::Command;

use argus_core::types::{ArgusError, ArgusResult};

use crate::repo::RepoManager;

/// Remote manager.
pub struct RemoteManager {
    repo: RepoManager,
}

impl RemoteManager {
    /// Create a new remote manager.
    pub fn new(repo: RepoManager) -> Self {
        Self { repo }
    }

    /// Fetch from the default remote.
    pub fn fetch(&self) -> ArgusResult<()> {
        let status = Command::new("git")
            .arg("fetch")
            .arg("origin")
            .current_dir(&self.repo.repo_path)
            .status()?;

        if status.success() {
            Ok(())
        } else {
            Err(ArgusError::Git("git fetch failed".to_string()))
        }
    }

    /// Push the current branch to origin.
    pub fn push(&self) -> ArgusResult<()> {
        let status = Command::new("git")
            .arg("push")
            .arg("-u")
            .arg("origin")
            .arg("HEAD")
            .current_dir(&self.repo.repo_path)
            .status()?;

        if status.success() {
            Ok(())
        } else {
            Err(ArgusError::Git("git push failed".to_string()))
        }
    }

    /// Pull from origin for current branch.
    pub fn pull(&self) -> ArgusResult<()> {
        let status = Command::new("git")
            .arg("pull")
            .arg("origin")
            .arg("HEAD")
            .current_dir(&self.repo.repo_path)
            .status()?;

        if status.success() {
            Ok(())
        } else {
            Err(ArgusError::Git("git pull failed".to_string()))
        }
    }
}
