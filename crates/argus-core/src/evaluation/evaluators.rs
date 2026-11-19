use std::time::Duration;

use async_trait::async_trait;
use tokio::process::Command;

use super::{EvaluationCheck, EvaluationContext, EvaluationOutcome};

#[async_trait]
pub trait Evaluator: Send + Sync {
    fn name(&self) -> &'static str;
    async fn evaluate(&self, context: &EvaluationContext) -> EvaluationCheck;
}

fn check(
    name: &str,
    outcome: EvaluationOutcome,
    summary: &str,
    context: &EvaluationContext,
) -> EvaluationCheck {
    EvaluationCheck {
        evaluator: name.into(),
        outcome,
        summary: summary.into(),
        artifact_ids: context.tasks.iter().filter_map(|t| t.artifact_id).collect(),
    }
}

/// Validates provenance and an independently recorded quality gate. A note or
/// a claimed completion without linked, completed tasks is insufficient.
pub struct ArtifactEvaluator;

#[async_trait]
impl Evaluator for ArtifactEvaluator {
    fn name(&self) -> &'static str {
        "task_artifacts"
    }

    async fn evaluate(&self, context: &EvaluationContext) -> EvaluationCheck {
        let result = if context.tasks.is_empty() {
            (
                EvaluationOutcome::Indeterminate,
                "No linked execution tasks",
            )
        } else if context
            .work_item
            .definition_of_done
            .as_deref()
            .is_none_or(|s| s.trim().is_empty())
        {
            (
                EvaluationOutcome::Indeterminate,
                "A definition of done is required",
            )
        } else if context.tasks.iter().any(|t| {
            t.project_id != context.work_item.project_id
                || t.artifact_project_id.is_some_and(|id| id != t.project_id)
                || t.workspace_project_id.is_some_and(|id| id != t.project_id)
                || t.workspace_task_id.is_some_and(|id| id != t.task_id)
                || t.artifact_execution_id != t.execution_id
                || t.workspace_execution_id != t.execution_id
        }) {
            (
                EvaluationOutcome::Failed,
                "Task, artifact, or workspace provenance mismatch",
            )
        } else if context
            .tasks
            .iter()
            .any(|t| t.state != "completed" || t.artifact_id.is_none())
        {
            (
                EvaluationOutcome::Indeterminate,
                "Every linked task needs a completed execution and checkpoint artifact",
            )
        } else if context.tasks.iter().any(|t| {
            t.execution_id.is_none()
                || !matches!(
                    t.quality_status.as_deref(),
                    Some("passed" | "passed_with_warnings")
                )
                || t.quality_summary
                    .as_deref()
                    .is_none_or(|s| s.trim().is_empty())
        }) {
            (
                EvaluationOutcome::Failed,
                "Missing or failing persisted quality gate",
            )
        } else if context.tasks.iter().any(|t| {
            t.task_contract
                .get("test_commands")
                .and_then(|v| v.as_array())
                .is_none_or(|commands| {
                    commands.is_empty()
                        || commands
                            .iter()
                            .any(|c| c.as_str().is_none_or(|s| s.trim().is_empty()))
                })
                || !t
                    .quality_summary
                    .as_deref()
                    .is_some_and(|s| s.contains("Test=Passed"))
        }) {
            (
                EvaluationOutcome::Indeterminate,
                "Every task requires independently run test commands and passing test evidence",
            )
        } else {
            (
                EvaluationOutcome::Passed,
                "All linked tasks have scoped checkpoint and passing test evidence",
            )
        };
        check(self.name(), result.0, result.1, context)
    }
}

/// Checks Git objects and the exact changed-file set without executing user
/// commands or editing the repository. Missing repositories fail closed.
pub struct GitCheckpointEvaluator;

async fn git(path: &str, args: &[&str]) -> Option<Vec<u8>> {
    let mut command = Command::new("git");
    command.args(args).current_dir(path).kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(10), command.output())
        .await
        .ok()?
        .ok()?;
    output.status.success().then_some(output.stdout)
}

fn valid_sha(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|b| b.is_ascii_hexdigit())
}

#[async_trait]
impl Evaluator for GitCheckpointEvaluator {
    fn name(&self) -> &'static str {
        "git_checkpoints"
    }

    async fn evaluate(&self, context: &EvaluationContext) -> EvaluationCheck {
        if context.tasks.is_empty() {
            return check(
                self.name(),
                EvaluationOutcome::Indeterminate,
                "No checkpoints to inspect",
                context,
            );
        }
        for task in &context.tasks {
            let (Some(path), Some(base), Some(commit), Some(files)) = (
                &task.worktree_path,
                &task.base_revision,
                &task.commit_sha,
                &task.files_changed,
            ) else {
                return check(
                    self.name(),
                    EvaluationOutcome::Indeterminate,
                    "Checkpoint identity is incomplete",
                    context,
                );
            };
            if !valid_sha(base) || !valid_sha(commit) {
                return check(
                    self.name(),
                    EvaluationOutcome::Failed,
                    "Checkpoint revisions must be full Git object IDs",
                    context,
                );
            }
            let object = format!("{commit}^{{commit}}");
            let Some(resolved) = git(path, &["rev-parse", "--verify", &object]).await else {
                return check(
                    self.name(),
                    EvaluationOutcome::Indeterminate,
                    "Checkpoint Git object is unavailable",
                    context,
                );
            };
            if String::from_utf8_lossy(&resolved).trim() != commit {
                return check(
                    self.name(),
                    EvaluationOutcome::Failed,
                    "Checkpoint object differs from the recorded commit",
                    context,
                );
            }
            if git(path, &["merge-base", "--is-ancestor", base, commit])
                .await
                .is_none()
            {
                return check(
                    self.name(),
                    EvaluationOutcome::Failed,
                    "Checkpoint is not descended from its recorded base",
                    context,
                );
            }
            let Some(message) = git(path, &["show", "--no-patch", "--format=%B", commit]).await
            else {
                return check(
                    self.name(),
                    EvaluationOutcome::Indeterminate,
                    "Checkpoint provenance could not be read",
                    context,
                );
            };
            let message = String::from_utf8_lossy(&message);
            let (Some(execution), Some(workspace)) = (task.execution_id, task.workspace_id) else {
                return check(
                    self.name(),
                    EvaluationOutcome::Failed,
                    "Missing checkpoint provenance",
                    context,
                );
            };
            if [
                format!("Task-ID: {}", task.task_id),
                format!("Execution-ID: {execution}"),
                format!("Workspace-ID: {workspace}"),
            ]
            .iter()
            .any(|trailer| !message.lines().any(|line| line == trailer))
            {
                return check(
                    self.name(),
                    EvaluationOutcome::Failed,
                    "Checkpoint trailers do not match the task execution",
                    context,
                );
            }
            let Some(diff) = git(
                path,
                &[
                    "diff",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--name-only",
                    "-z",
                    base,
                    commit,
                    "--",
                ],
            )
            .await
            else {
                return check(
                    self.name(),
                    EvaluationOutcome::Indeterminate,
                    "Checkpoint diff could not be inspected",
                    context,
                );
            };
            let mut actual: Vec<String> = diff
                .split(|b| *b == 0)
                .filter(|p| !p.is_empty())
                .map(|p| String::from_utf8_lossy(p).into_owned())
                .collect();
            let Some(recorded) = files.as_array() else {
                return check(
                    self.name(),
                    EvaluationOutcome::Failed,
                    "Invalid artifact changed-file list",
                    context,
                );
            };
            let Some(mut expected) = recorded
                .iter()
                .map(|v| v.as_str().map(String::from))
                .collect::<Option<Vec<_>>>()
            else {
                return check(
                    self.name(),
                    EvaluationOutcome::Failed,
                    "Invalid artifact path",
                    context,
                );
            };
            actual.sort();
            expected.sort();
            if actual.is_empty() || actual != expected {
                return check(
                    self.name(),
                    EvaluationOutcome::Failed,
                    "Checkpoint diff does not match the recorded artifact",
                    context,
                );
            }
        }
        check(
            self.name(),
            EvaluationOutcome::Passed,
            "All Git commits and changed-file sets independently verified",
            context,
        )
    }
}
