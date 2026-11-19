//! Server deployer (SSH-based)

use super::{DeployConfig, DeployError, DeployResult, Deployer};
use crate::types::{Deployment, DeploymentStatus};
use async_trait::async_trait;
use std::process::Stdio;
use tokio::process::Command;

pub struct ServerDeployer {
    host: String,
    user: String,
    key_path: Option<String>,
    deploy_path: String,
}

impl ServerDeployer {
    pub fn new(host: String, user: String, key_path: Option<String>, deploy_path: String) -> Self {
        Self {
            host,
            user,
            key_path,
            deploy_path,
        }
    }

    async fn run_ssh(&self, command: &str, deployment: &mut Deployment) -> DeployResult<String> {
        deployment.logs.push(format!("SSH: {}", command));

        let mut cmd = Command::new("ssh");

        if let Some(ref key) = self.key_path {
            cmd.arg("-i").arg(key);
        }

        let target = format!("{}@{}", self.user, self.host);
        cmd.arg("-o").arg("StrictHostKeyChecking=no");
        cmd.arg(&target);
        cmd.arg(command);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let output = cmd
            .output()
            .await
            .map_err(|e| DeployError::ConnectionFailed(e.to_string()))?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            Ok(stdout.to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            deployment.logs.push(format!("Error: {}", stderr));
            Err(DeployError::DeployFailed(stderr.to_string()))
        }
    }

    async fn scp(&self, source: &str, dest: &str, deployment: &mut Deployment) -> DeployResult<()> {
        deployment.logs.push(format!("SCP: {} -> {}", source, dest));

        let mut cmd = Command::new("scp");

        if let Some(ref key) = self.key_path {
            cmd.arg("-i").arg(key);
        }

        cmd.arg("-o").arg("StrictHostKeyChecking=no");
        cmd.arg(source);

        let target = format!("{}@{}:{}", self.user, self.host, dest);
        cmd.arg(&target);

        let output = cmd
            .output()
            .await
            .map_err(|e| DeployError::ConnectionFailed(e.to_string()))?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(DeployError::DeployFailed(stderr.to_string()))
        }
    }
}

#[async_trait]
impl Deployer for ServerDeployer {
    fn name(&self) -> &str {
        "server"
    }

    async fn build(&self, deployment: &mut Deployment) -> DeployResult<()> {
        deployment.status = DeploymentStatus::Building;

        // Build locally first
        deployment.logs.push("Building locally...".to_string());

        let output = Command::new("npm")
            .args(["run", "build"])
            .output()
            .await
            .map_err(|e| DeployError::BuildFailed(e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DeployError::BuildFailed(stderr.to_string()));
        }

        deployment.logs.push("Build completed".to_string());
        Ok(())
    }

    async fn push(&self, deployment: &mut Deployment) -> DeployResult<()> {
        deployment.logs.push("Uploading files...".to_string());

        // Create tarball of dist folder
        let tarball = format!(
            "/tmp/{}-{}.tar.gz",
            deployment.project_id, deployment.version
        );

        let output = Command::new("tar")
            .args(["-czf", &tarball, "-C", "dist", "."])
            .output()
            .await
            .map_err(|e| DeployError::PushFailed(e.to_string()))?;

        if !output.status.success() {
            return Err(DeployError::PushFailed(
                "Failed to create tarball".to_string(),
            ));
        }

        let remote_path = format!("{}/{}.tar.gz", self.deploy_path, deployment.version);
        self.scp(&tarball, &remote_path, deployment).await?;

        deployment.logs.push("Files uploaded".to_string());
        Ok(())
    }

    async fn deploy(
        &self,
        _config: &DeployConfig,
        deployment: &mut Deployment,
    ) -> DeployResult<()> {
        deployment.status = DeploymentStatus::Deploying;

        let version = &deployment.version;
        let deploy_dir = &self.deploy_path;

        // Extract and link
        let commands = vec![
            format!("cd {} && tar -xzf {}.tar.gz", deploy_dir, version),
            format!("ln -sfn {}/{} {}/current", deploy_dir, version, deploy_dir),
            format!("rm -f {}.tar.gz", version),
        ];

        for cmd in commands {
            self.run_ssh(&cmd, deployment).await?;
        }

        deployment.logs.push("Deployment completed".to_string());
        Ok(())
    }

    async fn health_check(&self, _deployment: &Deployment) -> DeployResult<bool> {
        let url = format!("http://{}/health", self.host);

        let output = Command::new("curl").args(["-sf", &url]).output().await;

        Ok(output.map(|o| o.status.success()).unwrap_or(false))
    }

    async fn rollback(&self, deployment: &mut Deployment) -> DeployResult<()> {
        let deploy_dir = &self.deploy_path;

        // Get previous version from symlink
        let _current_output = self
            .run_ssh(
                &format!(
                    "ls -la {} | grep 'current ->' | awk '{{print $NF}}'",
                    deploy_dir
                ),
                deployment,
            )
            .await?;

        // List available versions
        let output = self
            .run_ssh(
                &format!("ls -1 {} | grep -v current | tail -2 | head -1", deploy_dir),
                deployment,
            )
            .await?;

        let previous_version = output.trim();

        if !previous_version.is_empty() {
            let cmd = format!("cd {} && ln -sfn {} current", deploy_dir, previous_version);
            self.run_ssh(&cmd, deployment).await?;
            deployment
                .logs
                .push(format!("Rolled back to {}", previous_version));
        }

        Ok(())
    }
}
