//! `AgentTaskExecutor` — plugs a real coding agent into the `ExecutionLoop`.
//!
//! This closes the chain the shell executor could only stand in for:
//!
//! ```text
//! ARGUS Task -> ExecutionLoop -> AgentDriver -> Claude Code / Codex / Kimi
//!            -> real repository edits -> git diff -> quality gate (tests)
//!            -> result back into the task record
//! ```
//!
//! # Repository model
//!
//! Each project gets one persistent git repository at
//! `<workspace_root>/<project_id>`, created and `git init`-ed on first use.
//! Successive tasks for a project therefore build on each other's work, which
//! is what "the agent works on my project" has to mean.
//!
//! # How "did it actually change anything?" is decided
//!
//! Not by parsing the agent's prose. The executor snapshots the git work tree
//! before and after the run and diffs it. An agent that exits 0 while changing
//! nothing is reported as a failure, because a task that produced no change
//! did not do its job.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use tokio::process::Command;
use tracing::{info, warn};

use argus_core::execution_loop::{ExecutionRequest, TaskExecutionResult, TaskExecutor};
use argus_core::task_types::Task;
use argus_core::workspace_manager::WorkspaceContext;

use crate::agent::{AgentDriver, AgentTask};

/// Executes tasks by handing them to a coding agent.
///
/// # V0.38 Phase 3 — workspace isolation
///
/// When `ExecutionLoop` has workspace integration attached, every
/// [`ExecutionRequest`] carries a verified [`WorkspaceContext`] and the
/// Agent is run with `worktree_path` as its cwd — never the project's
/// canonical repository (`workspace_root/<project_id>`). Without workspace
/// integration (legacy/tests only), this falls back to the pre-Phase-3
/// behavior of running directly in the canonical repository, so existing
/// non-isolated callers keep working.
pub struct AgentTaskExecutor {
    driver: Arc<dyn AgentDriver>,
    workspace_root: PathBuf,
    model: Option<String>,
    name: String,
}

impl AgentTaskExecutor {
    pub fn new(driver: Arc<dyn AgentDriver>, workspace_root: impl Into<PathBuf>) -> Self {
        let name = format!("agent:{}", driver.executor_type());
        Self {
            driver,
            workspace_root: workspace_root.into(),
            model: None,
            name,
        }
    }

    /// Set the model the agent should use — this is the lever for running
    /// cheap sub-agents under an expensive planner.
    pub fn with_model(mut self, model: Option<String>) -> Self {
        self.model = model;
        self
    }

    /// One persistent repository per project — used only as a fallback when
    /// no `WorkspaceContext` is attached (see struct docs).
    fn repo_for(&self, task: &Task) -> PathBuf {
        self.workspace_root.join(task.project_id.to_string())
    }

    /// Create the repo if needed and make sure it is a git work tree.
    ///
    /// Only used on the no-workspace fallback path: a worktree handed in via
    /// `WorkspaceContext` has already been created and verified by
    /// `ExecutionLoop` and must not be re-initialized here.
    async fn ensure_repo(&self, repo: &Path) -> Result<(), String> {
        tokio::fs::create_dir_all(repo)
            .await
            .map_err(|e| format!("failed to create repo dir {}: {e}", repo.display()))?;

        if repo.join(".git").exists() {
            return Ok(());
        }

        let out = Command::new("git")
            .args(["init", "-q"])
            .current_dir(repo)
            .output()
            .await
            .map_err(|e| format!("failed to run git init: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "git init failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }

        // An identity is required for the repo to be commit-able later.
        for (k, v) in [
            ("user.name", "Argus Agent"),
            ("user.email", "argus@localhost"),
        ] {
            let _ = Command::new("git")
                .args(["config", k, v])
                .current_dir(repo)
                .output()
                .await;
        }
        info!(repo = %repo.display(), "Initialized project repository");
        Ok(())
    }

    /// Snapshot of the work tree: every path git considers changed or
    /// untracked, plus the hash of every tracked file's content.
    ///
    /// Comparing two snapshots catches edits to already-dirty files, which a
    /// bare `git status` comparison would miss.
    async fn snapshot(&self, repo: &Path) -> HashMap<String, String> {
        let mut snap = HashMap::new();

        // Tracked + untracked paths with their current content hash.
        let out = Command::new("git")
            .args(["status", "--porcelain", "--untracked-files=all"])
            .current_dir(repo)
            .output()
            .await;
        if let Ok(out) = out {
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                if let Some(path) = line.get(3..) {
                    snap.insert(path.trim().to_string(), String::new());
                }
            }
        }

        // Hash every file git knows about or can see, so content-only edits
        // to an already-listed path still register.
        let out = Command::new("git")
            .args([
                "-c",
                "core.quotepath=false",
                "ls-files",
                "--cached",
                "--others",
                "--exclude-standard",
            ])
            .current_dir(repo)
            .output()
            .await;
        if let Ok(out) = out {
            for path in String::from_utf8_lossy(&out.stdout).lines() {
                let path = path.trim();
                if path.is_empty() {
                    continue;
                }
                let hash = Command::new("git")
                    .args(["hash-object", "--", path])
                    .current_dir(repo)
                    .output()
                    .await
                    .ok()
                    .filter(|o| o.status.success())
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                    .unwrap_or_default();
                snap.insert(path.to_string(), hash);
            }
        }

        snap
    }

    /// Paths whose presence or content differs between two snapshots.
    fn diff_snapshots(
        before: &HashMap<String, String>,
        after: &HashMap<String, String>,
    ) -> Vec<String> {
        let mut changed: HashSet<String> = HashSet::new();
        for (path, hash) in after {
            match before.get(path) {
                Some(prev) if prev == hash => {}
                _ => {
                    changed.insert(path.clone());
                }
            }
        }
        for path in before.keys() {
            if !after.contains_key(path) {
                changed.insert(path.clone());
            }
        }
        let mut out: Vec<String> = changed.into_iter().collect();
        out.sort();
        out
    }

    /// Turn a task record into an instruction the agent can act on.
    fn build_prompt(task: &Task) -> String {
        let mut p = String::new();
        p.push_str("You are working inside a git repository as an autonomous coding agent.\n");
        p.push_str("Complete the task below by editing files directly in this repository.\n\n");
        p.push_str(&format!("# Task\n{}\n", task.title));

        if !task.description.trim().is_empty() && task.description.trim() != task.title.trim() {
            p.push_str(&format!("\n## Details\n{}\n", task.description));
        }
        if !task.acceptance_criteria.is_empty() {
            p.push_str("\n## Acceptance criteria\n");
            for c in &task.acceptance_criteria {
                p.push_str(&format!("- {c}\n"));
            }
        }
        if !task.test_commands.is_empty() {
            p.push_str("\n## Verification\n");
            p.push_str("These commands will be run against your work and must pass:\n");
            for c in &task.test_commands {
                p.push_str(&format!("- `{c}`\n"));
            }
        }
        p.push_str(
            "\n## Rules\n\
             - Make the changes on disk; do not just describe them.\n\
             - Do not commit; Argus handles version control.\n\
             - Keep changes scoped to this task.\n",
        );
        p
    }
}

#[async_trait]
impl TaskExecutor for AgentTaskExecutor {
    fn name(&self) -> &str {
        &self.name
    }

    async fn execute(&self, request: &ExecutionRequest) -> TaskExecutionResult {
        let task = &request.task;
        let started_at = Utc::now();

        // V0.38 Phase 3: run in the verified worktree when one is attached.
        // The canonical repository (`repo_for`) is only ever used as a
        // fallback for callers that don't attach workspace isolation.
        let workspace: Option<&WorkspaceContext> = request.workspace.as_ref();
        let repo = workspace
            .map(|w| w.worktree_path.clone())
            .unwrap_or_else(|| self.repo_for(task));

        if !self.driver.is_available().await {
            return TaskExecutionResult::failed(
                task.id,
                started_at,
                format!("agent binary `{}` not found on PATH", self.driver.binary()),
            );
        }

        if let Some(ws) = workspace {
            // The worktree was already created and verified by ExecutionLoop
            // (rules #4/#5/#15) — this is only a last-resort sanity check
            // that we are about to run in exactly that directory.
            if let Err(e) = ws.validate_executor_cwd(&repo) {
                return TaskExecutionResult::failed(task.id, started_at, e.to_string());
            }
        } else if let Err(e) = self.ensure_repo(&repo).await {
            return TaskExecutionResult::failed(task.id, started_at, e);
        }

        let before = self.snapshot(&repo).await;

        let agent_task = AgentTask {
            prompt: Self::build_prompt(task),
            workspace: repo.clone(),
            timeout: task.timeout,
            model: self.model.clone(),
        };

        info!(
            task_id = %task.id,
            project_id = %task.project_id,
            agent = %self.driver.worker_type(),
            model = ?self.model,
            repo = %repo.display(),
            timeout_s = task.timeout.as_secs(),
            "Dispatching task to coding agent"
        );

        let run = self.driver.run(&agent_task).await;

        let after = self.snapshot(&repo).await;
        let files_changed = Self::diff_snapshots(&before, &after);

        let mut metadata = HashMap::new();
        metadata.insert("agent".to_string(), self.driver.executor_type().to_string());
        metadata.insert("repo".to_string(), repo.display().to_string());
        metadata.insert("files_changed".to_string(), files_changed.len().to_string());
        if let Some(m) = run.model.as_ref().or(self.model.as_ref()) {
            metadata.insert("model".to_string(), m.clone());
        }
        if let Some(c) = run.cost_usd {
            metadata.insert("cost_usd".to_string(), format!("{c:.6}"));
        }
        if let Some(t) = run.tokens_input {
            metadata.insert("agent_tokens_input".to_string(), t.to_string());
        }
        if let Some(t) = run.tokens_output {
            metadata.insert("agent_tokens_output".to_string(), t.to_string());
        }
        if let Some(t) = run.num_turns {
            metadata.insert("agent_turns".to_string(), t.to_string());
        }
        if let Some(s) = &run.session_id {
            metadata.insert("agent_session_id".to_string(), s.clone());
        }

        // An agent that "succeeded" without touching the repository has not
        // done the task. Treat that as a failure rather than a silent pass.
        let (success, failure_reason) = if !run.success {
            (
                false,
                run.failure_reason
                    .clone()
                    .or_else(|| Some("agent reported failure".to_string())),
            )
        } else if files_changed.is_empty() {
            warn!(
                task_id = %task.id,
                "Agent exited successfully but changed no files"
            );
            (
                false,
                Some("agent completed without modifying any file in the repository".to_string()),
            )
        } else {
            (true, None)
        };

        info!(
            task_id = %task.id,
            success,
            files_changed = files_changed.len(),
            cost_usd = ?run.cost_usd,
            turns = ?run.num_turns,
            "Coding agent finished"
        );

        // The agent's own narration is the useful summary; raw stdout is
        // mostly progress chatter and goes to stderr_summary for debugging.
        let summary = if run.final_message.is_empty() {
            run.stdout.clone()
        } else {
            run.final_message.clone()
        };

        TaskExecutionResult {
            task_id: task.id,
            success,
            exit_code: run.exit_code,
            stdout_summary: summary,
            stderr_summary: run.stderr.clone(),
            started_at,
            finished_at: Utc::now(),
            failure_reason,
            files_changed,
            workspace: Some(repo.clone()),
            metadata,
            // Set directly from the verified WorkspaceContext ExecutionLoop
            // attached to this request. ExecutionLoop also backfills these
            // defensively, but the executor is the source of truth for what
            // it actually ran in.
            workspace_id: workspace.map(|w| w.workspace_id),
            branch_name: workspace.map(|w| w.branch_name.clone()),
            base_revision: workspace.map(|w| w.base_revision.clone()),
            worktree_path: workspace.map(|w| w.worktree_path.clone()).or(Some(repo)),
        }
    }
}

/// Convenience: build an executor for a worker type.
pub fn agent_executor_for(
    worker_type: argus_core::worker_types::WorkerType,
    workspace_root: impl Into<PathBuf>,
    model: Option<String>,
) -> AgentTaskExecutor {
    AgentTaskExecutor::new(
        Arc::from(crate::agent::driver_for(worker_type, None)),
        workspace_root,
    )
    .with_model(model)
}

/// Default per-task timeout when a task does not carry one.
pub const DEFAULT_AGENT_TIMEOUT: Duration = Duration::from_secs(30 * 60);

#[cfg(test)]
mod tests {
    use super::*;
    use argus_core::task_types::TaskBuilder;
    use uuid::Uuid;

    struct MetadataDriver(argus_core::worker_types::WorkerType);
    #[async_trait]
    impl AgentDriver for MetadataDriver {
        fn worker_type(&self) -> argus_core::worker_types::WorkerType {
            self.0
        }
        fn binary(&self) -> &str {
            "sh"
        }
        async fn run(&self, task: &AgentTask) -> crate::agent::AgentRun {
            std::fs::write(task.workspace.join("edited.txt"), "stub edit").unwrap();
            crate::agent::AgentRun {
                success: true,
                exit_code: Some(0),
                ..Default::default()
            }
        }
    }

    #[tokio::test]
    async fn test_executor_metadata_identifies_codex_and_claude() {
        for (worker, expected) in [
            (argus_core::worker_types::WorkerType::GenericCli, "codex"),
            (
                argus_core::worker_types::WorkerType::ClaudeCode,
                "claude_code",
            ),
        ] {
            let root = std::env::temp_dir().join(format!("argus-metadata-{}", Uuid::new_v4()));
            let executor = AgentTaskExecutor::new(Arc::new(MetadataDriver(worker)), &root);
            let request = ExecutionRequest {
                task: TaskBuilder::new(Uuid::new_v4(), "metadata").build(),
                workspace: None,
            };
            let result = executor.execute(&request).await;
            assert!(result.success);
            assert_eq!(
                result.metadata.get("agent").map(String::as_str),
                Some(expected)
            );
            assert_eq!(executor.name(), format!("agent:{expected}"));
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn test_prompt_includes_task_contract() {
        let task = TaskBuilder::new(Uuid::new_v4(), "Add a greet function")
            .description("Add greet(name) to src/lib.rs returning a greeting")
            .acceptance("greet(\"a\") returns \"Hello, a!\"")
            .test_command("cargo test")
            .build();

        let p = AgentTaskExecutor::build_prompt(&task);
        assert!(p.contains("Add a greet function"));
        assert!(p.contains("src/lib.rs"));
        assert!(p.contains("Hello, a!"));
        assert!(p.contains("cargo test"));
        assert!(p.contains("do not just describe them"));
        assert!(p.contains("Do not commit"));
    }

    #[test]
    fn test_prompt_omits_duplicate_description() {
        // TaskBuilder defaults description to the title.
        let task = TaskBuilder::new(Uuid::new_v4(), "Same text").build();
        let p = AgentTaskExecutor::build_prompt(&task);
        assert_eq!(p.matches("Same text").count(), 1, "prompt was: {p}");
    }

    #[test]
    fn test_diff_snapshots_detects_add_modify_delete() {
        let before = HashMap::from([
            ("keep.txt".to_string(), "h1".to_string()),
            ("edit.txt".to_string(), "h2".to_string()),
            ("gone.txt".to_string(), "h3".to_string()),
        ]);
        let after = HashMap::from([
            ("keep.txt".to_string(), "h1".to_string()),
            ("edit.txt".to_string(), "CHANGED".to_string()),
            ("new.txt".to_string(), "h4".to_string()),
        ]);

        let changed = AgentTaskExecutor::diff_snapshots(&before, &after);
        assert_eq!(changed, vec!["edit.txt", "gone.txt", "new.txt"]);
        assert!(!changed.contains(&"keep.txt".to_string()));
    }

    #[test]
    fn test_diff_snapshots_empty_when_unchanged() {
        let snap = HashMap::from([("a".to_string(), "h".to_string())]);
        assert!(AgentTaskExecutor::diff_snapshots(&snap, &snap).is_empty());
    }
}
