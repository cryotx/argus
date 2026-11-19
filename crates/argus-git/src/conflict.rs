//! Conflict detection and resolution

use std::path::Path;
use std::process::Command;

use argus_core::types::{ArgusError, ArgusResult};

/// Detect if there are merge conflicts in the repository
pub fn detect_conflicts(repo_path: &Path) -> ArgusResult<bool> {
    let output = Command::new("git")
        .arg("diff")
        .arg("--name-only")
        .arg("--diff-filter=U")
        .current_dir(repo_path)
        .output()?;

    if !output.status.success() {
        return Err(ArgusError::Git("git diff failed".to_string()));
    }

    let conflicts = String::from_utf8_lossy(&output.stdout);
    Ok(!conflicts.trim().is_empty())
}

/// Get list of conflicted files
pub fn get_conflicted_files(repo_path: &Path) -> ArgusResult<Vec<String>> {
    let output = Command::new("git")
        .arg("diff")
        .arg("--name-only")
        .arg("--diff-filter=U")
        .current_dir(repo_path)
        .output()?;

    if !output.status.success() {
        return Err(ArgusError::Git("git diff failed".to_string()));
    }

    let files: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .collect();

    Ok(files)
}

/// Conflict resolution strategy
#[derive(Debug, Clone)]
pub enum ConflictResolution {
    /// Accept ours (our changes)
    Ours,
    /// Accept theirs (their changes)
    Theirs,
    /// Accept both
    Both,
    /// Manual resolution required
    Manual,
}

/// Resolve a conflict using the specified strategy
pub fn resolve_conflict(file_path: &Path, resolution: ConflictResolution) -> ArgusResult<()> {
    let args = match resolution {
        ConflictResolution::Ours => vec!["checkout", "--ours", file_path.to_str().unwrap_or("")],
        ConflictResolution::Theirs => {
            vec!["checkout", "--theirs", file_path.to_str().unwrap_or("")]
        }
        ConflictResolution::Both => vec!["checkout", "--merge", file_path.to_str().unwrap_or("")],
        ConflictResolution::Manual => {
            return Err(ArgusError::Git("Manual resolution required".to_string()));
        }
    };

    let status = Command::new("git").args(&args).status()?;

    if status.success() {
        Ok(())
    } else {
        Err(ArgusError::Git("git checkout failed".to_string()))
    }
}
