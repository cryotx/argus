use std::path::Path;

use async_trait::async_trait;
use chrono::Utc;
use tokio::process::Command;

use argus_core::types::{ArgusError, ArgusResult};

#[derive(Clone, Debug)]
pub struct VpsDeployRequest {
    pub host: String,
    pub user: String,
    pub artifact_path: String,
    pub service_name: String,
}

#[derive(Clone, Debug)]
pub struct VpsDeployResult {
    pub deployed: bool,
    pub host: String,
}

#[async_trait]
pub trait VpsDeployer {
    async fn deploy(&self, req: &VpsDeployRequest) -> ArgusResult<VpsDeployResult>;
}

#[derive(Clone, Debug)]
pub struct SshVpsDeployer {
    ssh_program: String,
    scp_program: String,
}

impl SshVpsDeployer {
    pub fn new() -> Self {
        Self {
            ssh_program: "ssh".to_string(),
            scp_program: "scp".to_string(),
        }
    }

    pub fn with_programs(ssh_program: impl Into<String>, scp_program: impl Into<String>) -> Self {
        Self {
            ssh_program: ssh_program.into(),
            scp_program: scp_program.into(),
        }
    }

    fn validate_request(req: &VpsDeployRequest) -> ArgusResult<()> {
        if req.host.trim().is_empty() {
            return Err(ArgusError::Deploy("host cannot be empty".to_string()));
        }
        if req.user.trim().is_empty() {
            return Err(ArgusError::Deploy("user cannot be empty".to_string()));
        }
        if req.service_name.trim().is_empty() {
            return Err(ArgusError::Deploy(
                "service_name cannot be empty".to_string(),
            ));
        }
        if req.artifact_path.trim().is_empty() {
            return Err(ArgusError::Deploy(
                "artifact_path cannot be empty".to_string(),
            ));
        }
        if !Path::new(&req.artifact_path).exists() {
            return Err(ArgusError::Deploy(format!(
                "artifact not found: {}",
                req.artifact_path
            )));
        }
        Ok(())
    }

    fn scp_target(req: &VpsDeployRequest, remote_artifact: &str) -> String {
        format!("{}@{}:{}", req.user, req.host, remote_artifact)
    }

    fn build_remote_command(req: &VpsDeployRequest, remote_artifact: &str) -> String {
        let service = req.service_name.trim();
        let install_root = format!("/opt/{service}");

        format!(
            "sudo mkdir -p {install_root} && sudo cp {remote_artifact} {install_root}/artifact && sudo systemctl restart {service} && sudo systemctl is-active --quiet {service}",
        )
    }

    async fn run_command(&self, program: &str, args: &[&str]) -> ArgusResult<()> {
        let output = Command::new(program)
            .args(args)
            .output()
            .await
            .map_err(|e| ArgusError::Deploy(format!("failed to start {program}: {e}")))?;

        if output.status.success() {
            return Ok(());
        }

        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(ArgusError::Deploy(format!(
            "{program} command failed: {}",
            stderr.trim()
        )))
    }
}

impl Default for SshVpsDeployer {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VpsDeployer for SshVpsDeployer {
    async fn deploy(&self, req: &VpsDeployRequest) -> ArgusResult<VpsDeployResult> {
        Self::validate_request(req)?;

        let remote_artifact = format!(
            "/tmp/argus-{}-{}.artifact",
            req.service_name,
            Utc::now().timestamp_millis()
        );
        let scp_target = Self::scp_target(req, &remote_artifact);

        self.run_command(&self.scp_program, &[&req.artifact_path, &scp_target])
            .await?;

        let remote_command = Self::build_remote_command(req, &remote_artifact);
        let remote_host = format!("{}@{}", req.user, req.host);

        self.run_command(&self.ssh_program, &[&remote_host, &remote_command])
            .await?;

        Ok(VpsDeployResult {
            deployed: true,
            host: req.host.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_validate_request_rejects_missing_host() {
        let req = VpsDeployRequest {
            host: " ".to_string(),
            user: "root".to_string(),
            artifact_path: "/tmp/a.tar.gz".to_string(),
            service_name: "argus".to_string(),
        };

        let err = SshVpsDeployer::validate_request(&req).unwrap_err();
        assert!(format!("{err}").contains("host cannot be empty"));
    }

    #[test]
    fn test_validate_request_rejects_missing_artifact() {
        let req = VpsDeployRequest {
            host: "127.0.0.1".to_string(),
            user: "root".to_string(),
            artifact_path: "/tmp/definitely-missing-artifact-argus.tar.gz".to_string(),
            service_name: "argus".to_string(),
        };

        let err = SshVpsDeployer::validate_request(&req).unwrap_err();
        assert!(format!("{err}").contains("artifact not found"));
    }

    #[test]
    fn test_scp_target_and_remote_command() {
        let tmp = std::env::temp_dir().join("argus-vps-test-artifact");
        fs::write(&tmp, "ok").unwrap();

        let req = VpsDeployRequest {
            host: "10.0.0.9".to_string(),
            user: "deploy".to_string(),
            artifact_path: tmp.display().to_string(),
            service_name: "argus-service".to_string(),
        };

        let target = SshVpsDeployer::scp_target(&req, "/tmp/a.artifact");
        assert_eq!(target, "deploy@10.0.0.9:/tmp/a.artifact");

        let cmd = SshVpsDeployer::build_remote_command(&req, "/tmp/a.artifact");
        assert!(cmd.contains("systemctl restart argus-service"));
    }
}
