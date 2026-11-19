//! Pull Request management via gh CLI

use std::process::Command;

use argus_core::types::{ArgusError, ArgusResult};
use serde::Deserialize;

use crate::repo::RepoManager;

/// PR manager using GitHub CLI (gh)
pub struct PrManager {
    repo: RepoManager,
}

impl PrManager {
    pub fn new(repo: RepoManager) -> Self {
        Self { repo }
    }

    /// Create a pull request
    pub fn create_pr(
        &self,
        title: &str,
        body: &str,
        base: &str,
        head: &str,
    ) -> ArgusResult<String> {
        let output = self.run_gh(&[
            "pr", "create", "--title", title, "--body", body, "--base", base, "--head", head,
        ])?;

        let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if url.is_empty() {
            return Err(ArgusError::Git(
                "gh pr create succeeded but returned empty URL".to_string(),
            ));
        }
        Ok(url)
    }

    /// List pull requests
    pub fn list_prs(&self, state: &str) -> ArgusResult<Vec<PrInfo>> {
        let output = self.run_gh(&[
            "pr",
            "list",
            "--state",
            state,
            "--json",
            "number,title,url,state",
        ])?;

        let prs: Vec<GhPrListItem> = serde_json::from_slice(&output.stdout)
            .map_err(|e| ArgusError::Git(format!("Failed to parse PR list: {e}")))?;

        let result: Vec<PrInfo> = prs
            .into_iter()
            .map(|pr| PrInfo {
                number: pr.number,
                title: pr.title,
                url: pr.url,
                state: pr.state,
            })
            .collect();

        Ok(result)
    }

    /// Get PR status
    pub fn get_pr_status(&self, pr_number: u32) -> ArgusResult<PrStatus> {
        let pr_number_s = pr_number.to_string();
        let output = self.run_gh(&[
            "pr",
            "view",
            &pr_number_s,
            "--json",
            "state,mergeable,additions,deletions,changedFiles",
        ])?;

        let json: GhPrStatus = serde_json::from_slice(&output.stdout)
            .map_err(|e| ArgusError::Git(format!("Failed to parse PR: {e}")))?;

        let mergeable_state = json.mergeable.clone();
        let mergeable = matches!(mergeable_state.as_str(), "MERGEABLE" | "UNKNOWN");

        Ok(PrStatus {
            state: json.state,
            mergeable,
            mergeable_state,
            additions: json.additions,
            deletions: json.deletions,
            changed_files: json.changed_files,
        })
    }

    /// Merge a pull request
    pub fn merge_pr(&self, pr_number: u32, method: &str) -> ArgusResult<()> {
        let mode_flag = merge_mode_flag(method)?;
        let pr_number_s = pr_number.to_string();

        self.run_gh(&["pr", "merge", &pr_number_s, mode_flag, "--delete-branch"])?;

        Ok(())
    }

    /// List review comments for a pull request.
    pub fn list_comments(&self, pr_number: u32) -> ArgusResult<Vec<PrComment>> {
        let endpoint = format!("repos/{{owner}}/{{repo}}/pulls/{pr_number}/comments");
        let output = self.run_gh(&["api", &endpoint])?;

        let comments: Vec<GhPrComment> = serde_json::from_slice(&output.stdout)
            .map_err(|e| ArgusError::Git(format!("Failed to parse PR comments: {e}")))?;

        Ok(comments
            .into_iter()
            .map(|c| PrComment {
                id: c.id,
                author: c.user.login,
                body: c.body,
                path: c.path,
                line: c.line,
                created_at: c.created_at,
            })
            .collect())
    }

    /// Add a top-level comment to a pull request.
    pub fn add_comment(&self, pr_number: u32, body: &str) -> ArgusResult<()> {
        let pr_number_s = pr_number.to_string();
        self.run_gh(&["pr", "comment", &pr_number_s, "--body", body])?;

        Ok(())
    }

    fn run_gh(&self, args: &[&str]) -> ArgusResult<std::process::Output> {
        let output = Command::new("gh")
            .args(args)
            .current_dir(&self.repo.repo_path)
            .output()?;

        if output.status.success() {
            return Ok(output);
        }

        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = if !stderr.trim().is_empty() {
            stderr.trim().to_string()
        } else {
            stdout.trim().to_string()
        };

        Err(ArgusError::Git(format!(
            "gh {} failed: {}",
            args.join(" "),
            detail
        )))
    }
}

fn merge_mode_flag(method: &str) -> ArgusResult<&'static str> {
    match method.trim().to_ascii_lowercase().as_str() {
        "merge" => Ok("--merge"),
        "squash" => Ok("--squash"),
        "rebase" => Ok("--rebase"),
        other => Err(ArgusError::Git(format!(
            "unsupported merge method: {other}; expected merge|squash|rebase"
        ))),
    }
}

#[derive(Debug, Deserialize)]
struct GhPrListItem {
    number: u32,
    title: String,
    url: String,
    state: String,
}

#[derive(Debug, Deserialize)]
struct GhPrStatus {
    state: String,
    mergeable: String,
    additions: u32,
    deletions: u32,
    #[serde(rename = "changedFiles")]
    changed_files: u32,
}

#[derive(Debug, Deserialize)]
struct GhPrComment {
    id: u64,
    body: String,
    path: Option<String>,
    line: Option<u32>,
    created_at: String,
    user: GhUser,
}

#[derive(Debug, Deserialize)]
struct GhUser {
    login: String,
}

/// PR information
#[derive(Debug, Clone)]
pub struct PrInfo {
    pub number: u32,
    pub title: String,
    pub url: String,
    pub state: String,
}

/// PR status details
#[derive(Debug, Clone)]
pub struct PrStatus {
    pub state: String,
    pub mergeable: bool,
    pub mergeable_state: String,
    pub additions: u32,
    pub deletions: u32,
    pub changed_files: u32,
}

/// PR review comment
#[derive(Debug, Clone)]
pub struct PrComment {
    pub id: u64,
    pub author: String,
    pub body: String,
    pub path: Option<String>,
    pub line: Option<u32>,
    pub created_at: String,
}

#[cfg(test)]
mod tests {
    use super::merge_mode_flag;

    #[test]
    fn test_merge_mode_flag_variants() {
        assert_eq!(merge_mode_flag("merge").unwrap(), "--merge");
        assert_eq!(merge_mode_flag("squash").unwrap(), "--squash");
        assert_eq!(merge_mode_flag("ReBaSe").unwrap(), "--rebase");
        assert!(merge_mode_flag("invalid").is_err());
    }
}
