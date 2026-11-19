//! Shared execution runtime helpers for worker fallback and error classification.

use std::path::Path;
use std::time::Duration;

use tokio::process::Command;

use crate::task_types::TaskErrorType;
use crate::worker_types::WorkerType;

pub const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(10 * 60);

const WORKER_FALLBACK_ORDER: [WorkerType; 4] = [
    WorkerType::OpenCode,
    WorkerType::ClaudeCode,
    WorkerType::Aider,
    WorkerType::GenericCli,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellExecutor {
    Bash,
    Sh,
}

impl ShellExecutor {
    pub fn program(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::Sh => "sh",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Bash => "generic_cli:bash",
            Self::Sh => "generic_cli:sh",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ExecutionResult {
    pub stdout: String,
    pub executor: ShellExecutor,
    pub fallbacks: Vec<String>,
}

#[derive(Clone, Copy, Debug)]
pub enum ExecutionErrorCategory {
    WorkerUnavailable,
    PermissionDenied,
    Timeout,
    CommandFailed,
    Unknown,
}

#[derive(Clone, Debug)]
pub struct ExecutionError {
    pub category: ExecutionErrorCategory,
    pub detail: String,
    pub fallbacks: Vec<String>,
}

impl ExecutionError {
    pub fn to_task_error_type(&self) -> TaskErrorType {
        match self.category {
            ExecutionErrorCategory::WorkerUnavailable => TaskErrorType::WorkerCrashed,
            ExecutionErrorCategory::PermissionDenied => TaskErrorType::SecurityViolation,
            ExecutionErrorCategory::Timeout => TaskErrorType::Timeout,
            ExecutionErrorCategory::CommandFailed => TaskErrorType::TestsFailed,
            ExecutionErrorCategory::Unknown => TaskErrorType::Unknown,
        }
    }
}

pub fn ordered_worker_candidates(preferred: Option<WorkerType>) -> Vec<WorkerType> {
    let mut order = Vec::with_capacity(WORKER_FALLBACK_ORDER.len());
    if let Some(preferred) = preferred {
        order.push(preferred);
    }

    for candidate in WORKER_FALLBACK_ORDER {
        if !order.contains(&candidate) {
            order.push(candidate);
        }
    }

    order
}

pub fn binary_in_path(binary: &str) -> bool {
    if binary.contains('/') {
        return is_executable(Path::new(binary));
    }

    std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| dir.join(binary))
                .any(|candidate| is_executable(&candidate))
        })
        .unwrap_or(false)
}

pub fn first_available_shell() -> Option<ShellExecutor> {
    [ShellExecutor::Bash, ShellExecutor::Sh]
        .into_iter()
        .find(|executor| binary_in_path(executor.program()))
}

pub async fn run_shell_with_fallback(
    command: &str,
    workspace_dir: &Path,
    timeout: Duration,
) -> Result<ExecutionResult, ExecutionError> {
    let mut fallbacks = Vec::new();
    for executor in [ShellExecutor::Bash, ShellExecutor::Sh] {
        match run_with_executor(executor, command, workspace_dir, timeout).await {
            Ok(stdout) => {
                return Ok(ExecutionResult {
                    stdout,
                    executor,
                    fallbacks,
                });
            }
            Err(err) => {
                if matches!(err.category, ExecutionErrorCategory::WorkerUnavailable)
                    && matches!(executor, ShellExecutor::Bash)
                {
                    fallbacks.push(format!("{} unavailable ({})", executor.label(), err.detail));
                    continue;
                }

                return Err(ExecutionError {
                    category: err.category,
                    detail: err.detail,
                    fallbacks,
                });
            }
        }
    }

    Err(ExecutionError {
        category: ExecutionErrorCategory::WorkerUnavailable,
        detail: "No shell executor available".to_string(),
        fallbacks,
    })
}

async fn run_with_executor(
    executor: ShellExecutor,
    command: &str,
    workspace_dir: &Path,
    timeout: Duration,
) -> Result<String, ExecutionError> {
    let mut child = Command::new(executor.program());
    child
        .arg("-c")
        .arg(command)
        .current_dir(workspace_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let child = child.spawn().map_err(|e| ExecutionError {
        category: classify_spawn_error(e.kind()),
        detail: format!("failed to spawn {}: {}", executor.program(), e),
        fallbacks: Vec::new(),
    })?;

    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| ExecutionError {
            category: ExecutionErrorCategory::Timeout,
            detail: format!(
                "{} exceeded timeout of {}s",
                executor.label(),
                timeout.as_secs()
            ),
            fallbacks: Vec::new(),
        })
        .and_then(|result| {
            result.map_err(|e| ExecutionError {
                category: ExecutionErrorCategory::Unknown,
                detail: format!("{} wait failed: {}", executor.label(), e),
                fallbacks: Vec::new(),
            })
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    if output.status.success() {
        Ok(stdout)
    } else {
        Err(ExecutionError {
            category: ExecutionErrorCategory::CommandFailed,
            detail: format!(
                "{} exited with status {}{}",
                executor.label(),
                output.status,
                if stderr.is_empty() {
                    String::new()
                } else {
                    format!(", stderr: {}", stderr)
                }
            ),
            fallbacks: Vec::new(),
        })
    }
}

fn classify_spawn_error(kind: std::io::ErrorKind) -> ExecutionErrorCategory {
    match kind {
        std::io::ErrorKind::NotFound => ExecutionErrorCategory::WorkerUnavailable,
        std::io::ErrorKind::PermissionDenied => ExecutionErrorCategory::PermissionDenied,
        _ => ExecutionErrorCategory::Unknown,
    }
}

fn is_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = std::fs::metadata(path) {
            return (metadata.permissions().mode() & 0o111) != 0;
        }
    }

    #[cfg(not(unix))]
    {
        if let Ok(metadata) = std::fs::metadata(path) {
            return !metadata.permissions().readonly();
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ordered_worker_candidates_prioritizes_hint() {
        let order = ordered_worker_candidates(Some(WorkerType::Aider));
        assert_eq!(order[0], WorkerType::Aider);
        assert!(order.contains(&WorkerType::OpenCode));
        assert!(order.contains(&WorkerType::GenericCli));
    }

    #[test]
    fn test_execution_error_mapping() {
        let timeout = ExecutionError {
            category: ExecutionErrorCategory::Timeout,
            detail: "timed out".to_string(),
            fallbacks: vec![],
        };
        assert_eq!(timeout.to_task_error_type(), TaskErrorType::Timeout);

        let unavailable = ExecutionError {
            category: ExecutionErrorCategory::WorkerUnavailable,
            detail: "missing binary".to_string(),
            fallbacks: vec![],
        };
        assert_eq!(
            unavailable.to_task_error_type(),
            TaskErrorType::WorkerCrashed
        );
    }

    #[test]
    fn test_binary_in_path_detects_absolute_missing() {
        let missing = std::path::PathBuf::from("/tmp/argus-not-a-real-binary");
        assert!(!binary_in_path(missing.to_string_lossy().as_ref()));
    }
}
