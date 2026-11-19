//! Kubernetes deployer

use super::{DeployConfig, DeployError, DeployResult, Deployer};
use crate::types::{Deployment, DeploymentStatus};
use async_trait::async_trait;
use std::process::Stdio;
use tokio::process::Command;

pub struct KubernetesDeployer {
    namespace: String,
    kubeconfig: Option<String>,
}

impl KubernetesDeployer {
    pub fn new(namespace: String, kubeconfig: Option<String>) -> Self {
        Self {
            namespace,
            kubeconfig,
        }
    }

    async fn run_kubectl(&self, args: &[&str], deployment: &mut Deployment) -> DeployResult<()> {
        deployment.logs.push(format!("kubectl {}", args.join(" ")));

        let mut cmd = Command::new("kubectl");

        if let Some(ref kubeconfig) = self.kubeconfig {
            cmd.arg("--kubeconfig").arg(kubeconfig);
        }

        cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());

        let output = cmd
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
impl Deployer for KubernetesDeployer {
    fn name(&self) -> &str {
        "kubernetes"
    }

    async fn build(&self, deployment: &mut Deployment) -> DeployResult<()> {
        deployment.status = DeploymentStatus::Building;

        // Kubernetes deployer assumes image is already built
        deployment
            .logs
            .push("Build step skipped - using pre-built image".to_string());
        Ok(())
    }

    async fn push(&self, deployment: &mut Deployment) -> DeployResult<()> {
        deployment
            .logs
            .push("Push step skipped - image should be in registry".to_string());
        Ok(())
    }

    async fn deploy(&self, config: &DeployConfig, deployment: &mut Deployment) -> DeployResult<()> {
        deployment.status = DeploymentStatus::Deploying;

        let image = config
            .image
            .as_ref()
            .ok_or_else(|| DeployError::DeployFailed("Image not specified".to_string()))?;

        // Apply Kubernetes deployment
        let replicas = config.replicas.unwrap_or(1);

        let yaml = format!(
            r#"
apiVersion: apps/v1
kind: Deployment
metadata:
  name: {project}-{env}
  namespace: {namespace}
spec:
  replicas: {replicas}
  selector:
    matchLabels:
      app: {project}
      env: {env}
  template:
    metadata:
      labels:
        app: {project}
        env: {env}
    spec:
      containers:
      - name: app
        image: {image}
        ports:
        - containerPort: 8080
"#,
            project = deployment.project_id,
            env = deployment.environment,
            namespace = self.namespace,
            replicas = replicas,
            image = image
        );

        // Write temporary manifest
        let manifest_path = format!("/tmp/{}-{}.yaml", deployment.project_id, deployment.id);
        std::fs::write(&manifest_path, &yaml)
            .map_err(|e| DeployError::DeployFailed(e.to_string()))?;

        self.run_kubectl(&["apply", "-f", &manifest_path], deployment)
            .await?;

        // Wait for rollout
        let dep_name = format!("{}-{}", deployment.project_id, deployment.environment);
        self.run_kubectl(
            &[
                "rollout",
                "status",
                "deployment",
                &dep_name,
                "--timeout=300s",
            ],
            deployment,
        )
        .await?;

        deployment.logs.push("Deployment completed".to_string());
        Ok(())
    }

    async fn health_check(&self, deployment: &Deployment) -> DeployResult<bool> {
        let name = format!("{}-{}", deployment.project_id, deployment.environment);

        let output = Command::new("kubectl")
            .args([
                "--namespace",
                &self.namespace,
                "get",
                "deployment",
                &name,
                "-o",
                "jsonpath={.status.readyReplicas}",
            ])
            .output()
            .await
            .map_err(|e| DeployError::ConnectionFailed(e.to_string()))?;

        if output.status.success() {
            let replicas = String::from_utf8_lossy(&output.stdout);
            Ok(!replicas.trim().is_empty() && replicas.trim() != "0")
        } else {
            Ok(false)
        }
    }

    async fn rollback(&self, deployment: &mut Deployment) -> DeployResult<()> {
        let name = format!("{}-{}", deployment.project_id, deployment.environment);

        self.run_kubectl(
            &["rollout", "undo", "deployment", &name, "--to-revision=1"],
            deployment,
        )
        .await?;

        deployment.logs.push("Rollback completed".to_string());
        Ok(())
    }
}
