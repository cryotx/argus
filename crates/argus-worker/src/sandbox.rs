//! Worker sandbox management

use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::info;

use crate::types::{SandboxType, WorkerConfig};

/// Sandbox handle for worker isolation
pub struct SandboxHandle {
    pub sandbox_type: SandboxType,
    pub workspace_path: PathBuf,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl SandboxHandle {
    /// Create a new sandbox
    pub async fn create(
        sandbox_type: SandboxType,
        workspace_dir: &Path,
        _config: &WorkerConfig,
    ) -> anyhow::Result<Self> {
        let workspace_path = match sandbox_type {
            SandboxType::Directory => {
                let path = workspace_dir.join(".argus_workspace");
                fs::create_dir_all(&path).await?;
                info!(path = %path.display(), "Created workspace directory sandbox");
                path
            }
            SandboxType::Docker => {
                anyhow::bail!(
                    "Docker sandbox is not implemented; refusing to execute without the requested isolation"
                );
            }
            SandboxType::None => workspace_dir.to_path_buf(),
        };

        Ok(Self {
            sandbox_type,
            workspace_path,
            created_at: chrono::Utc::now(),
        })
    }

    /// Get the workspace path for the sandbox
    pub fn workspace_path(&self) -> &PathBuf {
        &self.workspace_path
    }

    /// Check if sandbox is active
    pub fn is_active(&self) -> bool {
        true
    }

    /// Cleanup sandbox resources
    pub async fn cleanup(&self) -> anyhow::Result<()> {
        if self.sandbox_type == SandboxType::Directory && self.workspace_path.exists() {
            fs::remove_dir_all(&self.workspace_path).await?;
            info!(path = %self.workspace_path.display(), "Cleaned up sandbox");
        }
        Ok(())
    }
}

impl Default for SandboxHandle {
    fn default() -> Self {
        Self {
            sandbox_type: SandboxType::None,
            workspace_path: PathBuf::from("/tmp"),
            created_at: chrono::Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn docker_request_never_silently_falls_back_to_host_directory() {
        let result = SandboxHandle::create(
            SandboxType::Docker,
            Path::new("/tmp"),
            &WorkerConfig::default(),
        )
        .await;
        assert!(result.is_err());
    }
}
