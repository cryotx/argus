//! Repository manager for git operations.
//!
//! Uses git CLI for basic repo management to avoid heavy dependencies.

use std::path::PathBuf;

use argus_core::types::{ArgusError, ArgusResult};
use std::process::Command;

/// Repository manager.
pub struct RepoManager {
    pub repo_path: PathBuf,
}

impl RepoManager {
    /// Create a new repo manager.
    pub fn new(repo_path: impl Into<PathBuf>) -> Self {
        Self {
            repo_path: repo_path.into(),
        }
    }

    /// Initialize a repository at the path.
    pub fn init(&self) -> ArgusResult<()> {
        let status = Command::new("git")
            .arg("init")
            .current_dir(&self.repo_path)
            .status()?
            .success();

        if status {
            Ok(())
        } else {
            Err(ArgusError::Git("git init failed".to_string()))
        }
    }

    /// Clone a repository into the path.
    pub fn clone(&self, url: &str) -> ArgusResult<()> {
        let status = Command::new("git")
            .arg("clone")
            .arg(url)
            .arg(&self.repo_path)
            .status()?
            .success();

        if status {
            Ok(())
        } else {
            Err(ArgusError::Git("git clone failed".to_string()))
        }
    }

    /// Get the current branch name (if any).
    pub fn current_branch(&self) -> ArgusResult<Option<String>> {
        let output = Command::new("git")
            .arg("rev-parse")
            .arg("--abbrev-ref")
            .arg("HEAD")
            .current_dir(&self.repo_path)
            .output()?;

        if !output.status.success() {
            return Ok(None);
        }

        let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if name.is_empty() || name == "HEAD" {
            Ok(None)
        } else {
            Ok(Some(name))
        }
    }

    /// Check if the repository has uncommitted changes.
    pub fn is_dirty(&self) -> ArgusResult<bool> {
        let output = Command::new("git")
            .arg("status")
            .arg("--porcelain")
            .current_dir(&self.repo_path)
            .output()?;

        if !output.status.success() {
            return Err(ArgusError::Git("git status failed".to_string()));
        }

        Ok(!output.stdout.is_empty())
    }

    /// Create a new branch.
    pub fn create_branch(&self, name: &str) -> ArgusResult<()> {
        let status = Command::new("git")
            .arg("branch")
            .arg(name)
            .current_dir(&self.repo_path)
            .status()?;

        if status.success() {
            Ok(())
        } else {
            Err(ArgusError::Git("git branch failed".to_string()))
        }
    }

    /// List local branches.
    pub fn list_branches(&self) -> ArgusResult<Vec<String>> {
        let output = Command::new("git")
            .arg("branch")
            .arg("--list")
            .current_dir(&self.repo_path)
            .output()?;

        if !output.status.success() {
            return Err(ArgusError::Git("git branch --list failed".to_string()));
        }

        let branches = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|line| line.trim().trim_start_matches('*').trim().to_string())
            .filter(|line| !line.is_empty())
            .collect();

        Ok(branches)
    }

    /// Check out a branch.
    pub fn checkout_branch(&self, name: &str) -> ArgusResult<()> {
        let status = Command::new("git")
            .arg("checkout")
            .arg(name)
            .current_dir(&self.repo_path)
            .status()?;

        if status.success() {
            Ok(())
        } else {
            Err(ArgusError::Git("git checkout failed".to_string()))
        }
    }

    /// Create a new git worktree at the specified path.
    ///
    /// `base_ref` is the commit-ish `branch_name` is created from. When
    /// `None`, `branch_name` must already exist and the worktree simply
    /// checks it out.
    ///
    /// The parent directory is created if needed, but the leaf path itself
    /// is left for `git worktree add` to create — git refuses to add a
    /// worktree at a path that already exists and is non-empty, and creating
    /// the leaf ourselves first only invites that failure mode.
    pub fn create_worktree(
        &self,
        worktree_path: impl Into<PathBuf>,
        branch_name: &str,
        base_ref: Option<&str>,
    ) -> ArgusResult<()> {
        let path = worktree_path.into();

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ArgusError::Git(format!("failed to create worktree parent dir: {}", e))
            })?;
        }

        let mut cmd = Command::new("git");
        cmd.arg("worktree").arg("add");
        match base_ref {
            // `-b <branch> <path> <base>` — create a new branch at base_ref.
            Some(base) => {
                cmd.arg("-b").arg(branch_name).arg(&path).arg(base);
            }
            // `<path> <branch>` — check out an existing branch.
            None => {
                cmd.arg(&path).arg(branch_name);
            }
        }

        let output = cmd
            .current_dir(&self.repo_path)
            .output()
            .map_err(|e| ArgusError::Git(format!("git worktree command failed: {}", e)))?;

        if output.status.success() {
            Ok(())
        } else {
            Err(ArgusError::Git(format!(
                "git worktree add failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )))
        }
    }

    /// Whether `path` is the working directory of a *linked* git worktree
    /// (as opposed to a plain directory, a bare repo, or the main worktree).
    ///
    /// Linked worktrees have a `.git` *file* (not directory) pointing at the
    /// real gitdir under the main repository's `.git/worktrees/<name>`.
    pub fn is_linked_worktree(path: &std::path::Path) -> bool {
        let dot_git = path.join(".git");
        dot_git.is_file()
            && std::fs::read_to_string(&dot_git)
                .map(|s| s.trim_start().starts_with("gitdir:"))
                .unwrap_or(false)
    }

    /// Whether `worktree_path` is registered as one of this repository's
    /// worktrees (per `git worktree list`), i.e. it is actually owned by
    /// this canonical repository and not just some unrelated checkout.
    pub fn owns_worktree(&self, worktree_path: &std::path::Path) -> ArgusResult<bool> {
        let canonical_target = worktree_path
            .canonicalize()
            .map_err(|e| ArgusError::Git(format!("cannot canonicalize worktree path: {}", e)))?;

        let worktrees = self.list_worktrees()?;
        Ok(worktrees.iter().any(|w| {
            w.canonicalize()
                .map(|c| c == canonical_target)
                .unwrap_or(false)
        }))
    }

    /// HEAD revision (commit SHA) resolved *at `path`* rather than at
    /// `self.repo_path` — for a linked worktree this is the worktree's own
    /// HEAD, since each worktree keeps an independent HEAD pointer.
    pub fn head_revision_at(path: &std::path::Path) -> ArgusResult<String> {
        RepoManager::new(path).get_head_revision()
    }

    /// Current branch resolved at `path` (see [`Self::head_revision_at`]).
    pub fn current_branch_at(path: &std::path::Path) -> ArgusResult<Option<String>> {
        RepoManager::new(path).current_branch()
    }

    /// Files that differ from `HEAD` in the work tree at `path`: tracked
    /// modifications, additions, deletions, renames, and untracked files —
    /// but never anything `.gitignore` excludes, because `git status`
    /// already omits ignored paths unless asked to include them.
    pub fn changed_files_at(path: &std::path::Path) -> ArgusResult<Vec<String>> {
        let output = Command::new("git")
            .arg("status")
            .arg("--porcelain")
            .arg("--untracked-files=all")
            .current_dir(path)
            .output()
            .map_err(|e| ArgusError::Git(format!("git status failed: {}", e)))?;

        if !output.status.success() {
            return Err(ArgusError::Git(format!(
                "git status failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }

        let mut files: Vec<String> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| {
                // Porcelain format: "XY <path>" or "XY <old> -> <new>" for renames.
                let rest = line.get(3..)?.trim();
                let path = rest.rsplit(" -> ").next().unwrap_or(rest);
                let path = path.trim_matches('"').trim();
                (!path.is_empty()).then(|| path.to_string())
            })
            .collect();
        files.sort();
        files.dedup();
        Ok(files)
    }

    /// Remove a git worktree.
    pub fn remove_worktree(&self, worktree_path: impl Into<PathBuf>) -> ArgusResult<()> {
        let path = worktree_path.into();

        let status = Command::new("git")
            .arg("worktree")
            .arg("remove")
            .arg(&path)
            .current_dir(&self.repo_path)
            .status()
            .map_err(|e| ArgusError::Git(format!("git worktree command failed: {}", e)))?;

        if status.success() {
            Ok(())
        } else {
            Err(ArgusError::Git("git worktree remove failed".to_string()))
        }
    }

    /// List all worktrees in the repository.
    pub fn list_worktrees(&self) -> ArgusResult<Vec<PathBuf>> {
        let output = Command::new("git")
            .arg("worktree")
            .arg("list")
            .arg("--porcelain")
            .current_dir(&self.repo_path)
            .output()
            .map_err(|e| ArgusError::Git(format!("git worktree list failed: {}", e)))?;

        if !output.status.success() {
            return Err(ArgusError::Git("git worktree list failed".to_string()));
        }

        // Porcelain format is a blank-line-separated record per worktree,
        // e.g. `worktree <path>` / `HEAD <sha>` / `branch <ref>` — only the
        // `worktree` line names a path; every other line's first token is
        // metadata, not a path.
        let worktrees = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| line.strip_prefix("worktree "))
            .map(|path| PathBuf::from(path.trim()))
            .collect();

        Ok(worktrees)
    }

    /// Get the HEAD revision (commit SHA).
    pub fn get_head_revision(&self) -> ArgusResult<String> {
        let output = Command::new("git")
            .arg("rev-parse")
            .arg("HEAD")
            .current_dir(&self.repo_path)
            .output()?;

        if !output.status.success() {
            return Err(ArgusError::Git("git rev-parse HEAD failed".to_string()));
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Commit all staged changes with a message.
    pub fn commit(&self, message: &str) -> ArgusResult<String> {
        let output = Command::new("git")
            .arg("commit")
            .arg("-m")
            .arg(message)
            .current_dir(&self.repo_path)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ArgusError::Git(format!("git commit failed: {}", stderr)));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Stage all changes for commit.
    pub fn stage_all(&self) -> ArgusResult<()> {
        let status = Command::new("git")
            .arg("add")
            .arg(".")
            .current_dir(&self.repo_path)
            .status()?;

        if status.success() {
            Ok(())
        } else {
            Err(ArgusError::Git("git add failed".to_string()))
        }
    }

    /// Whether anything is currently staged for commit (`git diff --cached
    /// --quiet`: exit 0 = nothing staged, exit 1 = staged changes exist).
    /// Used to refuse an empty checkpoint rather than reach for
    /// `--allow-empty`.
    pub fn has_staged_changes(&self) -> ArgusResult<bool> {
        let status = Command::new("git")
            .args(["diff", "--cached", "--quiet"])
            .current_dir(&self.repo_path)
            .status()?;
        match status.code() {
            Some(0) => Ok(false),
            Some(1) => Ok(true),
            _ => Err(ArgusError::Git(format!(
                "git diff --cached --quiet exited abnormally: {status:?}"
            ))),
        }
    }

    /// Commit whatever is staged with an explicit author/committer identity
    /// (`-c user.name=... -c user.email=...`, scoped to this one command —
    /// never the ambient global/repo Git config) and return the resulting
    /// commit SHA.
    pub fn commit_with_identity(
        &self,
        author_name: &str,
        author_email: &str,
        message: &str,
    ) -> ArgusResult<String> {
        let output = Command::new("git")
            .args([
                "-c",
                &format!("user.name={author_name}"),
                "-c",
                &format!("user.email={author_email}"),
                "commit",
                "-m",
                message,
            ])
            .current_dir(&self.repo_path)
            .output()?;

        if !output.status.success() {
            return Err(ArgusError::Git(format!(
                "git commit failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }

        self.get_head_revision()
    }

    /// Most recent commit (walking back from HEAD) whose message contains
    /// `needle` as a literal substring (`git log --fixed-strings --grep`),
    /// or `None` if none matches. Used to find a prior checkpoint commit by
    /// its `Execution-ID:` trailer.
    pub fn find_commit_by_message_substring(&self, needle: &str) -> ArgusResult<Option<String>> {
        let output = Command::new("git")
            .args([
                "log",
                "--format=%H",
                "--fixed-strings",
                "--grep",
                needle,
                "-n",
                "1",
            ])
            .current_dir(&self.repo_path)
            .output()?;

        if !output.status.success() {
            // No commits yet (empty history) is not an error condition here.
            return Ok(None);
        }
        let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok((!sha.is_empty()).then_some(sha))
    }

    /// Files that differ between two commit-ish revisions
    /// (`git diff --name-only -z base head`) — the authoritative content of a
    /// range, as opposed to [`Self::changed_files_at`] which reports the
    /// live working tree against `HEAD`.
    pub fn diff_name_only(&self, base: &str, head: &str) -> ArgusResult<Vec<String>> {
        let output = Command::new("git")
            .args([
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--name-only",
                "-z",
                base,
                head,
                "--",
            ])
            .current_dir(&self.repo_path)
            .output()?;

        if !output.status.success() {
            return Err(ArgusError::Git(format!(
                "git diff --name-only failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }

        Ok(output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(|path| String::from_utf8_lossy(path).into_owned())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_repo_init_and_branch() {
        let dir = tempfile::tempdir().unwrap();
        let repo = RepoManager::new(dir.path());
        repo.init().unwrap();

        let branch = repo.current_branch().unwrap();
        // Branch name can be None until first commit
        let _ = branch;
    }
}
