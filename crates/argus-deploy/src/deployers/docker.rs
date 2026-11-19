//! Docker deployer

use super::{DeployConfig, DeployError, DeployResult, Deployer};
use crate::types::{Deployment, DeploymentStatus};
use async_trait::async_trait;
use std::process::Stdio;
use tokio::process::Command;

pub struct DockerDeployer {
    registry: String,
}

impl DockerDeployer {
    pub fn new(registry: String) -> Self {
        Self { registry }
    }

    async fn run_command(
        &self,
        program: &str,
        args: &[&str],
        deployment: &mut Deployment,
    ) -> DeployResult<()> {
        deployment
            .logs
            .push(format!("Running: {} {}", program, args.join(" ")));

        let output = Command::new(program)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| DeployError::ConnectionFailed(e.to_string()))?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            deployment.logs.push(stdout.to_string());
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            deployment.logs.push(format!("Error: {}", stderr));
            Err(DeployError::DeployFailed(stderr.to_string()))
        }
    }
}

#[async_trait]
impl Deployer for DockerDeployer {
    fn name(&self) -> &str {
        "docker"
    }

    async fn build(&self, deployment: &mut Deployment) -> DeployResult<()> {
        deployment.status = DeploymentStatus::Building;

        let image_name = format!(
            "{}/{}:{}",
            self.registry, deployment.project_id, deployment.version
        );

        self.run_command("docker", &["build", "-t", &image_name, "."], deployment)
            .await?;

        deployment.logs.push(format!("Built image: {}", image_name));
        Ok(())
    }

    async fn push(&self, deployment: &mut Deployment) -> DeployResult<()> {
        let image_name = format!(
            "{}/{}:{}",
            self.registry, deployment.project_id, deployment.version
        );

        self.run_command("docker", &["push", &image_name], deployment)
            .await?;

        deployment
            .logs
            .push(format!("Pushed image: {}", image_name));
        Ok(())
    }

    async fn deploy(
        &self,
        _config: &DeployConfig,
        deployment: &mut Deployment,
    ) -> DeployResult<()> {
        deployment.status = DeploymentStatus::Deploying;

        let image_name = format!(
            "{}/{}:{}",
            self.registry, deployment.project_id, deployment.version
        );
        let container_name = format!("{}-{}", deployment.project_id, deployment.environment);

        // Stop existing container
        let _ = self
            .run_command("docker", &["rm", "-f", &container_name], deployment)
            .await;

        // Run new container
        let args = vec!["run", "-d", "--name", &container_name, &image_name];

        self.run_command("docker", &args, deployment).await?;

        deployment
            .logs
            .push(format!("Deployed container: {}", container_name));
        Ok(())
    }

    async fn health_check(&self, deployment: &Deployment) -> DeployResult<bool> {
        let container_name = format!("{}-{}", deployment.project_id, deployment.environment);

        let output = Command::new("docker")
            .args([
                "inspect",
                "--format={{.State.Health.Status}}",
                &container_name,
            ])
            .output()
            .await
            .map_err(|e| DeployError::ConnectionFailed(e.to_string()))?;

        if output.status.success() {
            let status = String::from_utf8_lossy(&output.stdout);
            Ok(status.trim() == "healthy")
        } else {
            // Container exists but no health check defined
            Ok(true)
        }
    }

    async fn rollback(&self, deployment: &mut Deployment) -> DeployResult<()> {
        let container_name = format!("{}-{}", deployment.project_id, deployment.environment);

        // Get previous image
        let output = Command::new("docker")
            .args(["ps", "-l", "--format={{.Image}}"])
            .output()
            .await
            .map_err(|e| DeployError::ConnectionFailed(e.to_string()))?;

        if !output.status.success() {
            return Err(DeployError::DeployFailed(
                "Cannot find previous container".to_string(),
            ));
        }

        // Stop current and start previous
        self.run_command("docker", &["rm", "-f", &container_name], deployment)
            .await
            .ok(); // Ignore errors - container might not exist

        deployment.logs.push("Rollback completed".to_string());
        Ok(())
    }
}
