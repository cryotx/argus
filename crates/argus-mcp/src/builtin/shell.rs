use tokio::process::Command;

use argus_core::types::{ArgusError, ArgusResult};
use argus_security::command_guard::CommandSafetyGuard;

#[derive(Clone, Debug)]
pub struct ShellServer {
    guard: CommandSafetyGuard,
}

impl ShellServer {
    pub fn new() -> Self {
        Self {
            guard: CommandSafetyGuard::new(),
        }
    }

    pub fn with_blocked_commands<I, S>(blocked_commands: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            guard: CommandSafetyGuard::with_additional_denied_tokens(blocked_commands),
        }
    }

    pub async fn run(&self, command: &str) -> ArgusResult<String> {
        self.guard
            .enforce(command)
            .map_err(|e| ArgusError::Security(format!("shell command blocked: {e}")))?;

        let out = Command::new("bash").arg("-c").arg(command).output().await?;
        if !out.status.success() {
            return Err(ArgusError::Mcp(format!(
                "shell command failed: {}",
                String::from_utf8_lossy(&out.stderr)
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }
}

impl Default for ShellServer {
    fn default() -> Self {
        Self::new()
    }
}
