//! Worker types for the Worker Manager.
//!
//! Defines worker types, states, capabilities, and configuration.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

/// Type of AI coding worker.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "worker_type", rename_all = "snake_case")]
pub enum WorkerType {
    /// OpenCode coding assistant
    OpenCode,
    /// Claude Code coding assistant
    ClaudeCode,
    /// Aider coding assistant
    Aider,
    /// Generic CLI worker
    #[default]
    GenericCli,
}

impl WorkerType {
    /// Get the default sandbox type for this worker.
    pub fn default_sandbox(&self) -> SandboxType {
        match self {
            WorkerType::OpenCode | WorkerType::ClaudeCode | WorkerType::Aider => {
                SandboxType::Directory
            }
            WorkerType::GenericCli => SandboxType::None,
        }
    }

    /// Get the capability tier of this worker type.
    pub fn capability(&self) -> WorkerCapability {
        match self {
            WorkerType::OpenCode | WorkerType::ClaudeCode => WorkerCapability::CollaborationCapable,
            WorkerType::Aider | WorkerType::GenericCli => WorkerCapability::SoloOnly,
        }
    }

    /// Get the default binary name for this worker type.
    pub fn default_binary(&self) -> &'static str {
        match self {
            WorkerType::OpenCode => "opencode",
            WorkerType::ClaudeCode => "claude",
            WorkerType::Aider => "aider",
            WorkerType::GenericCli => "bash",
        }
    }
}

impl std::fmt::Display for WorkerType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Self::OpenCode => "opencode",
            Self::ClaudeCode => "claude_code",
            Self::Aider => "aider",
            Self::GenericCli => "generic_cli",
        };
        f.write_str(value)
    }
}

impl std::str::FromStr for WorkerType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "opencode" => Ok(WorkerType::OpenCode),
            "claude_code" | "claude" | "claudecode" => Ok(WorkerType::ClaudeCode),
            "aider" => Ok(WorkerType::Aider),
            "generic_cli" | "generic" => Ok(WorkerType::GenericCli),
            "cli" | "ai_cli" | "aicli" => Ok(WorkerType::GenericCli),
            _ => Err(format!("Unknown worker type: {}", s)),
        }
    }
}

/// Collaboration capability tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerCapability {
    /// Solo-only: no structured JSON output or system prompt control.
    SoloOnly,
    /// Supports role-based collaboration.
    CollaborationCapable,
}

impl WorkerCapability {
    /// Check if this worker can participate in collaboration.
    pub fn can_collaborate(self) -> bool {
        matches!(self, WorkerCapability::CollaborationCapable)
    }
}

/// Worker execution state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "worker_state", rename_all = "snake_case")]
pub enum WorkerState {
    /// Worker is being spawned
    Starting,
    /// Worker is ready to accept tasks
    Idle,
    /// Worker is busy executing a task
    Busy,
    /// Worker is being stopped
    ShuttingDown,
    /// Worker has crashed
    Crashed,
}

/// Type of sandbox isolation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(rename_all = "snake_case")]
pub enum SandboxType {
    /// Directory-level isolation (default)
    #[default]
    Directory,
    /// Docker container isolation
    Docker,
    /// No isolation
    None,
}

/// Resource limits for a worker.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResourceLimits {
    /// Maximum memory in bytes (default: 2 GB)
    #[serde(default = "default_memory")]
    pub max_memory_bytes: u64,
    /// Maximum CPU cores (default: 2.0)
    #[serde(default = "default_cpu")]
    pub max_cpu_cores: f64,
    /// Maximum disk space in bytes (default: 10 GB)
    #[serde(default = "default_disk")]
    pub max_disk_bytes: u64,
    /// Maximum execution time (default: 30 min)
    #[serde(with = "serde_duration", default = "default_runtime")]
    pub max_runtime: Duration,
    /// Maximum output size in bytes (default: 50 MB)
    #[serde(default = "default_output")]
    pub max_output_bytes: u64,
    /// Network access allowed (default: true)
    #[serde(default = "default_network")]
    pub network_enabled: bool,
}

fn default_memory() -> u64 {
    2 * 1024 * 1024 * 1024
}
fn default_cpu() -> f64 {
    2.0
}
fn default_disk() -> u64 {
    10 * 1024 * 1024 * 1024
}
fn default_runtime() -> Duration {
    Duration::from_secs(30 * 60)
}
fn default_output() -> u64 {
    50 * 1024 * 1024
}
fn default_network() -> bool {
    true
}

/// Serde helpers for Duration
mod serde_duration {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u64(duration.as_secs())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let secs = u64::deserialize(deserializer)?;
        Ok(Duration::from_secs(secs))
    }
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_memory_bytes: default_memory(),
            max_cpu_cores: default_cpu(),
            max_disk_bytes: default_disk(),
            max_runtime: default_runtime(),
            max_output_bytes: default_output(),
            network_enabled: default_network(),
        }
    }
}

/// Configuration for a worker.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkerConfig {
    /// Workspace directory for the worker
    pub workspace_dir: PathBuf,
    /// Model override (optional)
    pub model_override: Option<String>,
    /// Command to run (for GenericCli)
    pub command: Option<String>,
    /// Execution timeout
    #[serde(with = "serde_duration", default = "default_runtime")]
    pub timeout: Duration,
    /// Resource limits
    #[serde(default)]
    pub resource_limits: ResourceLimits,
    /// Sandbox type
    #[serde(default)]
    pub sandbox_type: SandboxType,
    /// Environment variables
    #[serde(default)]
    pub environment: HashMap<String, String>,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            workspace_dir: PathBuf::from("/tmp"),
            model_override: None,
            command: None,
            timeout: default_runtime(),
            resource_limits: ResourceLimits::default(),
            sandbox_type: SandboxType::Directory,
            environment: HashMap::new(),
        }
    }
}

/// Handle to a running worker process.
#[derive(Debug)]
pub struct WorkerHandle {
    /// Process ID
    pub pid: u32,
    /// Process handle
    pub process: tokio::process::Child,
    /// When the worker was started
    pub started_at: DateTime<Utc>,
    /// Worker type
    pub worker_type: WorkerType,
}

/// Payload sent to a worker for task execution.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TaskPayload {
    /// Task description / prompt
    pub description: String,
    /// Acceptance criteria
    pub acceptance_criteria: Vec<String>,
    /// Test commands to verify success
    pub test_commands: Vec<String>,
    /// Additional context
    pub context: String,
    /// Target files to modify (hints)
    pub target_files: Vec<String>,
}

/// Output from a worker.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WorkerOutput {
    /// Raw stdout output
    pub raw_output: String,
    /// Process exit code (if exited)
    pub exit_code: Option<i32>,
}

/// Resource usage statistics.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ResourceUsage {
    /// Memory usage in bytes
    pub memory_bytes: u64,
    /// CPU usage percentage (0-100)
    pub cpu_percent: f64,
    /// Disk usage in bytes
    pub disk_bytes: u64,
}

/// Worker statistics.
#[derive(Clone, Debug, Default)]
pub struct WorkerStats {
    /// Total number of workers
    pub total_workers: usize,
    /// Number of idle workers
    pub idle_workers: usize,
    /// Number of busy workers
    pub busy_workers: usize,
    /// Number of crashed workers
    pub crashed_workers: usize,
    /// Total tasks completed across all workers
    pub total_tasks_completed: u64,
    /// Total tasks failed across all workers
    pub total_tasks_failed: u64,
    /// Success rate (0.0 - 1.0)
    pub success_rate: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_worker_type_from_str() {
        assert_eq!(
            "opencode".parse::<WorkerType>().unwrap(),
            WorkerType::OpenCode
        );
        assert_eq!(
            "claude_code".parse::<WorkerType>().unwrap(),
            WorkerType::ClaudeCode
        );
        assert_eq!(
            "claude".parse::<WorkerType>().unwrap(),
            WorkerType::ClaudeCode
        );
        assert!("unknown".parse::<WorkerType>().is_err());
    }

    #[test]
    fn test_worker_capability() {
        assert!(WorkerType::OpenCode.capability().can_collaborate());
        assert!(WorkerType::ClaudeCode.capability().can_collaborate());
        assert!(!WorkerType::Aider.capability().can_collaborate());
        assert!(!WorkerType::GenericCli.capability().can_collaborate());
    }

    #[test]
    fn test_default_sandbox() {
        assert_eq!(
            WorkerType::OpenCode.default_sandbox(),
            SandboxType::Directory
        );
        assert_eq!(WorkerType::GenericCli.default_sandbox(), SandboxType::None);
    }

    #[test]
    fn test_resource_limits_default() {
        let limits = ResourceLimits::default();
        assert_eq!(limits.max_memory_bytes, 2 * 1024 * 1024 * 1024);
        assert_eq!(limits.max_cpu_cores, 2.0);
        assert_eq!(limits.max_disk_bytes, 10 * 1024 * 1024 * 1024);
        assert_eq!(limits.max_runtime, Duration::from_secs(30 * 60));
        assert_eq!(limits.max_output_bytes, 50 * 1024 * 1024);
        assert!(limits.network_enabled);
    }

    #[test]
    fn test_worker_config_default() {
        let config = WorkerConfig::default();
        assert_eq!(config.workspace_dir, PathBuf::from("/tmp"));
        assert!(config.model_override.is_none());
        assert!(config.command.is_none());
    }

    #[test]
    fn test_task_payload_default() {
        let payload = TaskPayload::default();
        assert!(payload.description.is_empty());
        assert!(payload.acceptance_criteria.is_empty());
        assert!(payload.test_commands.is_empty());
    }
}
