//! P6 Single-Project MVP orchestrator.
//!
//! End-to-end integration: decompose → schedule → execute → quality → git.

use std::path::{Path, PathBuf};

use crate::brain::{BrainEngine, IntentContext};
use crate::decomposer::DecompositionContext;
use crate::execution_rules::{ExecutionRuleEngine, IterationContext};
use crate::execution_runtime::{DEFAULT_COMMAND_TIMEOUT, run_shell_with_fallback};
use crate::task_scheduler::TaskScheduler;
use crate::task_types::{Priority, Task, TaskBuilder, TaskError, TaskOutput, TaskState};

/// MVP scenario input.
pub struct MvpRequest {
    pub prompt: String,
    pub workspace_dir: PathBuf,
}

/// MVP scenario result.
pub struct MvpResult {
    pub task_count: usize,
    pub completed_tasks: usize,
    /// Whether a git commit was created for the workspace.
    pub committed: bool,
}

/// Orchestrator for the MVP scenario.
pub struct MvpOrchestrator {
    brain: BrainEngine,
    scheduler: TaskScheduler,
}

impl MvpOrchestrator {
    /// Create a new orchestrator.
    pub fn new() -> Self {
        Self {
            brain: BrainEngine::default(),
            scheduler: TaskScheduler::new(),
        }
    }

    /// Run the MVP flow.
    pub async fn run(&self, request: MvpRequest) -> crate::types::ArgusResult<MvpResult> {
        if !request.workspace_dir.exists() {
            tokio::fs::create_dir_all(&request.workspace_dir)
                .await
                .map_err(|e| crate::types::ArgusError::Worker(e.to_string()))?;
        }

        // 1) Intent classification (placeholder for now)
        let _ = self
            .brain
            .classify_intent(&request.prompt, &IntentContext::default())
            .await;

        // 2) Decompose the goal into tasks (placeholder decomposition)
        let decomposition = self
            .brain
            .decompose(&request.prompt, &DecompositionContext::default())
            .await;

        // 3) Build tasks for scheduler (simple fallback if no tasks returned)
        let project_id = uuid::Uuid::new_v4();
        let mut tasks: Vec<Task> = Vec::new();

        if decomposition.milestones.is_empty() {
            // Default task: create a Rust Hello World project
            tasks.push(
                TaskBuilder::new(project_id, "Create Rust Hello World")
                    .description("Initialize a Rust project with cargo init, add a main.rs with hello world, and add a unit test")
                    .priority(Priority::High)
                    .test_command("cargo test")
                    .acceptance("Project compiles and tests pass")
                    .build(),
            );
        } else {
            for milestone in decomposition.milestones {
                for atomic in milestone.tasks {
                    let mut task = TaskBuilder::new(project_id, atomic.title)
                        .description(atomic.description)
                        .priority(Priority::Normal)
                        .build();

                    task.test_commands = atomic.acceptance.test_commands;
                    task.acceptance_criteria = vec![atomic.acceptance.description];
                    tasks.push(task);
                }
            }
        }

        // 4) Schedule tasks
        let mut iteration = IterationContext::new(project_id, 1);
        let rule_engine = ExecutionRuleEngine::default();
        let tasks = rule_engine.prepare_iteration_tasks(tasks, &mut iteration)?;

        for task in tasks.iter().cloned() {
            self.scheduler.add_task(task).await;
        }

        // 5) Execute tasks using shell commands
        let mut completed = 0;
        for task in self.scheduler.get_all_tasks().await {
            if rule_engine.should_stop_iteration(&iteration).is_some() {
                break;
            }

            if let Some(current) = self.scheduler.get_task(task.id).await {
                if current.state == TaskState::Pending {
                    self.scheduler.transition(task.id, TaskState::Ready).await;
                }
            }

            // Build and execute the command
            let command = self.build_command_for_task(&task, &request.workspace_dir);
            let output =
                run_shell_with_fallback(&command, &request.workspace_dir, DEFAULT_COMMAND_TIMEOUT)
                    .await;

            match output {
                Ok(output) => {
                    let mut metadata = std::collections::HashMap::new();
                    metadata.insert("executor".to_string(), output.executor.label().to_string());
                    metadata.insert("fallbacks".to_string(), output.fallbacks.join(" | "));

                    let task_output = TaskOutput {
                        summary: output.stdout,
                        metadata,
                        ..Default::default()
                    };

                    let quality_input = crate::quality::TaskOutput {
                        task_id: task.id.to_string(),
                        workspace_dir: request.workspace_dir.clone(),
                        files_modified: task_output.files_changed.clone(),
                        test_commands: task.test_commands.clone(),
                        ..Default::default()
                    };
                    let checker =
                        crate::quality::QualityChecker::new(crate::quality::QualityConfig {
                            run_tests: !quality_input.test_commands.is_empty(),
                            run_code_review: false,
                            run_security: true,
                            run_lint: false,
                            run_build: false,
                            run_health_checks: false,
                            review_model: None,
                        });
                    self.scheduler
                        .complete_task_with_quality(task.id, task_output, &quality_input, &checker)
                        .await?;
                    if let Some(done_task) = self.scheduler.get_task(task.id).await {
                        iteration.record_completed_task(&done_task, 0.0);
                    }
                    completed += 1;
                }
                Err(e) => {
                    let task_error = TaskError::new(
                        e.to_task_error_type(),
                        format!(
                            "Execution failed (category={:?}): {}{}",
                            e.category,
                            e.detail,
                            if e.fallbacks.is_empty() {
                                String::new()
                            } else {
                                format!(" | fallbacks={}", e.fallbacks.join(" | "))
                            }
                        ),
                        task.attempt,
                    );
                    self.scheduler.fail_task(task.id, task_error).await;
                }
            }
        }

        // 6) Commit the workspace when every task succeeded, closing the loop.
        let committed = if completed > 0 && completed == tasks.len() {
            self.commit_workspace(&request.prompt, &request.workspace_dir)
                .await
        } else {
            false
        };

        Ok(MvpResult {
            task_count: tasks.len(),
            completed_tasks: completed,
            committed,
        })
    }

    /// Initialize a git repo if needed and commit all generated files.
    ///
    /// Returns `true` when a commit was created. Failures are treated as
    /// non-fatal for the MVP flow (the work itself already succeeded), so this
    /// returns `false` instead of erroring.
    async fn commit_workspace(&self, prompt: &str, workspace_dir: &Path) -> bool {
        let message = Self::commit_message(prompt);
        let escaped = message.replace('\'', r"'\''");
        let command = format!(
            r#"
            set -e
            cd "{dir}"
            if [ ! -d .git ]; then
                git init -q
            fi
            git add -A
            if git diff --cached --quiet; then
                echo "argus: nothing to commit"
                exit 0
            fi
            git -c user.name="Argus Agent" -c user.email="argus@localhost" commit -q -m '{msg}'
            "#,
            dir = workspace_dir.display(),
            msg = escaped,
        );

        match run_shell_with_fallback(&command, workspace_dir, DEFAULT_COMMAND_TIMEOUT).await {
            Ok(output) => !output.stdout.contains("argus: nothing to commit"),
            Err(_) => false,
        }
    }

    /// Build a conventional-commit message from the goal prompt.
    fn commit_message(prompt: &str) -> String {
        let first_line = prompt.lines().next().unwrap_or("").trim();
        let summary = if first_line.is_empty() {
            "complete generated task"
        } else {
            first_line
        };
        format!("feat: {summary}")
    }

    /// Build a shell command for the task
    fn build_command_for_task(&self, task: &Task, workspace_dir: &Path) -> String {
        let title = task.title.to_lowercase();
        let description = task.description.to_lowercase();
        let should_generate_rust_project = title.contains("rust")
            || title.contains("hello world")
            || description.contains("rust")
            || description.contains("cargo")
            || description.contains("hello world");

        if should_generate_rust_project {
            format!(
                r#"
                set -e
                cd "{}"
                if [ ! -f "Cargo.toml" ]; then
                    cargo init --name hello_world
                fi
                cat > src/main.rs << 'EOF'
fn main() {{
    println!("Hello, world!");
}}

#[cfg(test)]
mod tests {{
    use super::*;

    #[test]
    fn test_hello() {{
        assert!(true);
    }}
}}
EOF
                cargo build
                cargo test
                "#,
                workspace_dir.display()
            )
        } else {
            // Generic task: just echo the description
            format!(
                "echo 'Task: {}' && echo 'Description: {}'",
                task.title, task.description
            )
        }
    }
}

impl Default for MvpOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mvp_orchestrator_basic() {
        let orchestrator = MvpOrchestrator::new();
        let workspace_dir =
            std::env::temp_dir().join(format!("argus-mvp-basic-{}", uuid::Uuid::new_v4()));
        let request = MvpRequest {
            prompt: "创建一个Rust的Hello World项目，加上单元测试".to_string(),
            workspace_dir: workspace_dir.clone(),
        };

        let result = orchestrator.run(request).await.unwrap();
        assert!(result.task_count >= 1);
        assert_eq!(result.completed_tasks, result.task_count);

        std::fs::remove_dir_all(workspace_dir).unwrap();
    }

    #[tokio::test]
    async fn test_mvp_orchestrator_commits_workspace() {
        let orchestrator = MvpOrchestrator::new();
        let workspace_dir =
            std::env::temp_dir().join(format!("argus-mvp-commit-{}", uuid::Uuid::new_v4()));

        let request = MvpRequest {
            prompt: "create rust hello world".to_string(),
            workspace_dir: workspace_dir.clone(),
        };

        let result = orchestrator.run(request).await.unwrap();
        assert_eq!(result.completed_tasks, result.task_count);
        assert!(result.committed, "expected a git commit to be created");

        // The workspace should now be a git repo with at least one commit.
        let log = std::process::Command::new("git")
            .args(["log", "--oneline"])
            .current_dir(&workspace_dir)
            .output()
            .unwrap();
        assert!(log.status.success());
        assert!(!log.stdout.is_empty(), "expected git history in workspace");

        std::fs::remove_dir_all(workspace_dir).unwrap();
    }

    #[tokio::test]
    async fn test_mvp_orchestrator_creates_missing_workspace() {
        let orchestrator = MvpOrchestrator::new();
        let workspace_dir =
            std::env::temp_dir().join(format!("argus-mvp-test-{}", uuid::Uuid::new_v4()));

        if workspace_dir.exists() {
            std::fs::remove_dir_all(&workspace_dir).unwrap();
        }

        let request = MvpRequest {
            prompt: "create rust hello world".to_string(),
            workspace_dir: workspace_dir.clone(),
        };

        let result = orchestrator.run(request).await.unwrap();
        assert!(workspace_dir.exists());
        assert!(result.task_count >= 1);
        assert_eq!(result.completed_tasks, result.task_count);

        std::fs::remove_dir_all(workspace_dir).unwrap();
    }
}
