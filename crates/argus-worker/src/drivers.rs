//! Worker drivers

use async_trait::async_trait;
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::types::{
    ResourceUsage, TaskPayload, WorkerConfig, WorkerHandle, WorkerOutput, WorkerType,
};

/// Worker driver trait
#[async_trait]
pub trait WorkerDriver: Send + Sync {
    fn worker_type(&self) -> WorkerType;
    async fn is_available(&self) -> bool;
    async fn spawn(&self, config: &WorkerConfig) -> anyhow::Result<WorkerHandle>;
    async fn send_task(&self, handle: &mut WorkerHandle, task: &TaskPayload) -> anyhow::Result<()>;
    async fn read_output(&self, handle: &mut WorkerHandle) -> anyhow::Result<WorkerOutput>;
    async fn is_alive(&self, handle: &WorkerHandle) -> bool;
    async fn stop(&self, handle: &mut WorkerHandle) -> anyhow::Result<()>;
    async fn kill(&self, handle: &mut WorkerHandle) -> anyhow::Result<()>;
    async fn resource_usage(&self, handle: &WorkerHandle) -> anyhow::Result<ResourceUsage>;
}

/// Check if process is alive
fn is_process_alive(pid: u32) -> bool {
    let stat_path = format!("/proc/{}/stat", pid);
    let stat = match std::fs::read_to_string(&stat_path) {
        Ok(stat) => stat,
        Err(_) => return false,
    };
    let close_paren = match stat.rfind(") ") {
        Some(idx) => idx,
        None => return false,
    };
    let state_char = stat[close_paren + 2..].chars().next().unwrap_or(' ');
    state_char != 'Z'
}

/// OpenCode driver
pub struct OpenCodeDriver {
    binary_path: PathBuf,
    default_model: String,
}

impl OpenCodeDriver {
    pub fn new(binary_path: impl Into<PathBuf>) -> Self {
        Self {
            binary_path: binary_path.into(),
            default_model: "anthropic/claude-sonnet-4-20250514".to_string(),
        }
    }
}

#[async_trait]
impl WorkerDriver for OpenCodeDriver {
    fn worker_type(&self) -> WorkerType {
        WorkerType::OpenCode
    }

    async fn is_available(&self) -> bool {
        tokio::fs::metadata(&self.binary_path).await.is_ok()
    }

    async fn spawn(&self, config: &WorkerConfig) -> anyhow::Result<WorkerHandle> {
        let model = config
            .model_override
            .clone()
            .unwrap_or_else(|| self.default_model.clone());
        let mut cmd = Command::new(&self.binary_path);
        cmd.current_dir(&config.workspace_dir)
            .env("OPENCODE_MODEL", model)
            .env("OPENCODE_AUTO_APPROVE", "true")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        for (key, value) in &config.environment {
            cmd.env(key, value);
        }

        let child = cmd.spawn()?;
        Ok(WorkerHandle {
            pid: child.id().unwrap_or(0),
            process: child,
            started_at: chrono::Utc::now(),
            worker_type: WorkerType::OpenCode,
        })
    }

    async fn send_task(&self, handle: &mut WorkerHandle, task: &TaskPayload) -> anyhow::Result<()> {
        let prompt = format!(
            "{}\n\n## Acceptance Criteria\n{}\n\n## Test Commands\n{}",
            task.description,
            task.acceptance_criteria.join("\n"),
            task.test_commands.join("\n")
        );
        if let Some(stdin) = handle.process.stdin.as_mut() {
            stdin.write_all(prompt.as_bytes()).await?;
            stdin.write_all(b"\n").await?;
            stdin.flush().await?;
        }
        Ok(())
    }

    async fn read_output(&self, handle: &mut WorkerHandle) -> anyhow::Result<WorkerOutput> {
        let mut output = String::new();
        if let Some(stdout) = handle.process.stdout.as_mut() {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                match reader.read_line(&mut line).await {
                    Ok(0) => break,
                    Ok(_) => {
                        output.push_str(&line);
                        if line.contains("TASK_COMPLETE") || line.contains("TASK_FAILED") {
                            break;
                        }
                        line.clear();
                    }
                    Err(_) => break,
                }
            }
        }
        let exit_code = handle.process.try_wait()?.map(|s| s.code().unwrap_or(-1));
        Ok(WorkerOutput {
            raw_output: output,
            exit_code,
        })
    }

    async fn is_alive(&self, handle: &WorkerHandle) -> bool {
        is_process_alive(handle.pid)
    }

    async fn stop(&self, handle: &mut WorkerHandle) -> anyhow::Result<()> {
        if let Some(stdin) = handle.process.stdin.as_mut() {
            let _ = stdin.write_all(b"exit\n").await;
            let _ = stdin.flush().await;
        }
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        let _ = handle.process.kill().await;
        Ok(())
    }

    async fn kill(&self, handle: &mut WorkerHandle) -> anyhow::Result<()> {
        handle.process.kill().await?;
        Ok(())
    }

    async fn resource_usage(&self, _handle: &WorkerHandle) -> anyhow::Result<ResourceUsage> {
        Ok(ResourceUsage::default())
    }
}

/// Claude Code driver
pub struct ClaudeCodeDriver {
    binary_path: PathBuf,
}

impl ClaudeCodeDriver {
    pub fn new(binary_path: impl Into<PathBuf>) -> Self {
        Self {
            binary_path: binary_path.into(),
        }
    }
}

#[async_trait]
impl WorkerDriver for ClaudeCodeDriver {
    fn worker_type(&self) -> WorkerType {
        WorkerType::ClaudeCode
    }

    async fn is_available(&self) -> bool {
        tokio::fs::metadata(&self.binary_path).await.is_ok()
    }

    async fn spawn(&self, config: &WorkerConfig) -> anyhow::Result<WorkerHandle> {
        let mut cmd = Command::new(&self.binary_path);
        cmd.arg("--print")
            .arg("--dangerously-skip-permissions")
            .current_dir(&config.workspace_dir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        for (key, value) in &config.environment {
            cmd.env(key, value);
        }

        let child = cmd.spawn()?;
        Ok(WorkerHandle {
            pid: child.id().unwrap_or(0),
            process: child,
            started_at: chrono::Utc::now(),
            worker_type: WorkerType::ClaudeCode,
        })
    }

    async fn send_task(&self, handle: &mut WorkerHandle, task: &TaskPayload) -> anyhow::Result<()> {
        let prompt = format!(
            "{}\n\n## Acceptance Criteria\n{}\n\n## Test Commands\n{}",
            task.description,
            task.acceptance_criteria.join("\n"),
            task.test_commands.join("\n")
        );
        if let Some(stdin) = handle.process.stdin.as_mut() {
            stdin.write_all(prompt.as_bytes()).await?;
            stdin.write_all(b"\n").await?;
            stdin.flush().await?;
        }
        Ok(())
    }

    async fn read_output(&self, handle: &mut WorkerHandle) -> anyhow::Result<WorkerOutput> {
        let mut output = String::new();
        if let Some(stdout) = handle.process.stdout.as_mut() {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            while let Ok(n) = reader.read_line(&mut line).await {
                if n == 0 {
                    break;
                }
                output.push_str(&line);
                line.clear();
            }
        }
        let exit_code = handle.process.try_wait()?.map(|s| s.code().unwrap_or(-1));
        Ok(WorkerOutput {
            raw_output: output,
            exit_code,
        })
    }

    async fn is_alive(&self, handle: &WorkerHandle) -> bool {
        is_process_alive(handle.pid)
    }

    async fn stop(&self, handle: &mut WorkerHandle) -> anyhow::Result<()> {
        handle.process.kill().await?;
        Ok(())
    }

    async fn kill(&self, handle: &mut WorkerHandle) -> anyhow::Result<()> {
        handle.process.kill().await?;
        Ok(())
    }

    async fn resource_usage(&self, _handle: &WorkerHandle) -> anyhow::Result<ResourceUsage> {
        Ok(ResourceUsage::default())
    }
}

/// Aider driver
pub struct AiderDriver {
    binary_path: PathBuf,
    default_model: String,
}

impl AiderDriver {
    pub fn new(binary_path: impl Into<PathBuf>) -> Self {
        Self {
            binary_path: binary_path.into(),
            default_model: "deepseek/deepseek-chat".to_string(),
        }
    }
}

#[async_trait]
impl WorkerDriver for AiderDriver {
    fn worker_type(&self) -> WorkerType {
        WorkerType::Aider
    }

    async fn is_available(&self) -> bool {
        tokio::fs::metadata(&self.binary_path).await.is_ok()
    }

    async fn spawn(&self, config: &WorkerConfig) -> anyhow::Result<WorkerHandle> {
        let model = config
            .model_override
            .clone()
            .unwrap_or_else(|| self.default_model.clone());
        let mut cmd = Command::new(&self.binary_path);
        cmd.arg("--yes-always")
            .arg("--no-auto-commits")
            .arg("--model")
            .arg(model)
            .current_dir(&config.workspace_dir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        for (key, value) in &config.environment {
            cmd.env(key, value);
        }

        let child = cmd.spawn()?;
        Ok(WorkerHandle {
            pid: child.id().unwrap_or(0),
            process: child,
            started_at: chrono::Utc::now(),
            worker_type: WorkerType::Aider,
        })
    }

    async fn send_task(&self, handle: &mut WorkerHandle, task: &TaskPayload) -> anyhow::Result<()> {
        let prompt = format!(
            "{}\n\n## Acceptance Criteria\n{}\n\n## Test Commands\n{}",
            task.description,
            task.acceptance_criteria.join("\n"),
            task.test_commands.join("\n")
        );
        if let Some(stdin) = handle.process.stdin.as_mut() {
            stdin.write_all(prompt.as_bytes()).await?;
            stdin.write_all(b"\n").await?;
            stdin.flush().await?;
        }
        Ok(())
    }

    async fn read_output(&self, handle: &mut WorkerHandle) -> anyhow::Result<WorkerOutput> {
        let mut output = String::new();
        if let Some(stdout) = handle.process.stdout.as_mut() {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            while let Ok(n) = reader.read_line(&mut line).await {
                if n == 0 {
                    break;
                }
                output.push_str(&line);
                line.clear();
            }
        }
        let exit_code = handle.process.try_wait()?.map(|s| s.code().unwrap_or(-1));
        Ok(WorkerOutput {
            raw_output: output,
            exit_code,
        })
    }

    async fn is_alive(&self, handle: &WorkerHandle) -> bool {
        is_process_alive(handle.pid)
    }

    async fn stop(&self, handle: &mut WorkerHandle) -> anyhow::Result<()> {
        handle.process.kill().await?;
        Ok(())
    }

    async fn kill(&self, handle: &mut WorkerHandle) -> anyhow::Result<()> {
        handle.process.kill().await?;
        Ok(())
    }

    async fn resource_usage(&self, _handle: &WorkerHandle) -> anyhow::Result<ResourceUsage> {
        Ok(ResourceUsage::default())
    }
}

/// Generic CLI driver
pub struct GenericCliDriver;

impl GenericCliDriver {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl WorkerDriver for GenericCliDriver {
    fn worker_type(&self) -> WorkerType {
        WorkerType::GenericCli
    }

    async fn is_available(&self) -> bool {
        true
    }

    async fn spawn(&self, config: &WorkerConfig) -> anyhow::Result<WorkerHandle> {
        let command = config
            .command
            .clone()
            .unwrap_or_else(|| "echo 'No command'".to_string());
        let child = Command::new("bash")
            .arg("-c")
            .arg(command)
            .current_dir(&config.workspace_dir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;

        Ok(WorkerHandle {
            pid: child.id().unwrap_or(0),
            process: child,
            started_at: chrono::Utc::now(),
            worker_type: WorkerType::GenericCli,
        })
    }

    async fn send_task(&self, handle: &mut WorkerHandle, task: &TaskPayload) -> anyhow::Result<()> {
        if let Some(stdin) = handle.process.stdin.as_mut() {
            stdin.write_all(task.description.as_bytes()).await?;
            stdin.write_all(b"\n").await?;
            stdin.flush().await?;
        }
        Ok(())
    }

    async fn read_output(&self, handle: &mut WorkerHandle) -> anyhow::Result<WorkerOutput> {
        let mut output = String::new();
        if let Some(stdout) = handle.process.stdout.as_mut() {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            while let Ok(n) = reader.read_line(&mut line).await {
                if n == 0 {
                    break;
                }
                output.push_str(&line);
                line.clear();
            }
        }
        let exit_code = handle.process.try_wait()?.map(|s| s.code().unwrap_or(-1));
        Ok(WorkerOutput {
            raw_output: output,
            exit_code,
        })
    }

    async fn is_alive(&self, handle: &WorkerHandle) -> bool {
        is_process_alive(handle.pid)
    }

    async fn stop(&self, handle: &mut WorkerHandle) -> anyhow::Result<()> {
        handle.process.kill().await?;
        Ok(())
    }

    async fn kill(&self, handle: &mut WorkerHandle) -> anyhow::Result<()> {
        handle.process.kill().await?;
        Ok(())
    }

    async fn resource_usage(&self, _handle: &WorkerHandle) -> anyhow::Result<ResourceUsage> {
        Ok(ResourceUsage::default())
    }
}

impl Default for GenericCliDriver {
    fn default() -> Self {
        Self::new()
    }
}

/// Get a driver by worker type
pub fn get_driver(worker_type: WorkerType) -> Box<dyn WorkerDriver> {
    match worker_type {
        WorkerType::OpenCode => Box::new(OpenCodeDriver::new("opencode")),
        WorkerType::ClaudeCode => Box::new(ClaudeCodeDriver::new("claude")),
        WorkerType::Aider => Box::new(AiderDriver::new("aider")),
        WorkerType::GenericCli => Box::new(GenericCliDriver::new()),
    }
}
