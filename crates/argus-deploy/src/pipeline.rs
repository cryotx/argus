//! Deployment pipeline orchestrator

use super::deployers::{DeployResult, Deployer};
use super::health::HealthCheckConfig;
use super::rollback::RollbackManager;
use super::types::{DeployConfig, DeployTarget, Deployment, DeploymentStatus};
use crate::deployers::{DockerDeployer, KubernetesDeployer, ServerDeployer};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Deployment pipeline that orchestrates the entire deployment process
pub struct DeploymentPipeline {
    deployer: Arc<dyn Deployer>,
    deployments: RwLock<Vec<Deployment>>,
    health_config: HealthCheckConfig,
    rollback_manager: RollbackManager,
    auto_rollback: bool,
}

impl DeploymentPipeline {
    pub fn new(deployer: Arc<dyn Deployer>) -> Self {
        Self {
            deployer,
            deployments: RwLock::new(Vec::new()),
            health_config: HealthCheckConfig::default(),
            rollback_manager: RollbackManager::default(),
            auto_rollback: true,
        }
    }

    /// Create a pipeline with the appropriate deployer
    pub fn create(target: DeployTarget, config: &DeployConfig) -> Self {
        let deployer: Arc<dyn Deployer> = match target {
            DeployTarget::Docker => Arc::new(DockerDeployer::new(
                config.image.clone().unwrap_or_default(),
            )),
            DeployTarget::Kubernetes => {
                Arc::new(KubernetesDeployer::new(config.environment.clone(), None))
            }
            DeployTarget::Server => Arc::new(ServerDeployer::new(
                "localhost".to_string(), // Would be from config
                "root".to_string(),
                None,
                "/var/www/deploy".to_string(),
            )),
            _ => panic!("Unsupported deploy target: {:?}", target),
        };

        Self {
            deployer,
            deployments: RwLock::new(Vec::new()),
            health_config: HealthCheckConfig::default(),
            rollback_manager: RollbackManager::default(),
            auto_rollback: true,
        }
    }

    pub fn with_health_config(mut self, health_config: HealthCheckConfig) -> Self {
        self.health_config = health_config;
        self
    }

    pub fn with_auto_rollback(mut self, auto_rollback: bool) -> Self {
        self.auto_rollback = auto_rollback;
        self
    }

    /// Run the full deployment pipeline
    pub async fn run(
        &self,
        mut deployment: Deployment,
        config: &DeployConfig,
    ) -> DeployResult<Deployment> {
        deployment.started_at = Some(chrono::Utc::now());

        // Step 1: Build
        if let Err(e) = self.deployer.build(&mut deployment).await {
            self.mark_failed(&mut deployment, e.to_string());
            self.add_deployment(deployment.clone()).await;
            return Err(e);
        }

        // Step 2: Push
        if let Err(e) = self.deployer.push(&mut deployment).await {
            self.mark_failed(&mut deployment, e.to_string());
            self.add_deployment(deployment.clone()).await;
            return Err(e);
        }

        // Step 3: Deploy
        if let Err(e) = self.deployer.deploy(config, &mut deployment).await {
            let error_msg = e.to_string();
            self.mark_failed(&mut deployment, error_msg.clone());
            self.try_auto_rollback(&mut deployment, &error_msg).await;
            self.add_deployment(deployment.clone()).await;
            return Err(e);
        }

        // Step 4: Health check
        let healthy = self.run_health_checks(&mut deployment).await;

        if healthy {
            deployment.status = DeploymentStatus::Completed;
            deployment.logs.push("Health check passed".to_string());
        } else {
            self.mark_failed(&mut deployment, "Health check failed".to_string());
            self.try_auto_rollback(&mut deployment, "Health check failed")
                .await;
        }

        deployment.completed_at = Some(chrono::Utc::now());
        self.add_deployment(deployment.clone()).await;

        Ok(deployment)
    }

    /// Rollback a deployment
    pub async fn rollback(&self, mut deployment: Deployment) -> DeployResult<()> {
        deployment.status = DeploymentStatus::RollingBack;
        self.deployer.rollback(&mut deployment).await
    }

    /// Add a deployment to history
    async fn add_deployment(&self, deployment: Deployment) {
        let mut deployments = self.deployments.write().await;
        deployments.push(deployment);
    }

    /// Get deployment history
    pub async fn get_history(&self) -> Vec<Deployment> {
        let deployments = self.deployments.read().await;
        deployments.clone()
    }

    pub fn rollback_history(&self) -> Vec<crate::rollback::Rollback> {
        self.rollback_manager.history()
    }

    fn mark_failed(&self, deployment: &mut Deployment, message: String) {
        deployment.status = DeploymentStatus::Failed;
        deployment.error_message = Some(message);
        deployment.completed_at = Some(chrono::Utc::now());
    }

    async fn try_auto_rollback(&self, deployment: &mut Deployment, reason: &str) {
        if !self.auto_rollback {
            deployment
                .logs
                .push("Auto-rollback disabled; skipping rollback".to_string());
            return;
        }

        if !self.rollback_manager.should_rollback(deployment, true) {
            return;
        }

        deployment.status = DeploymentStatus::RollingBack;
        deployment
            .logs
            .push(format!("Auto-rollback triggered: {reason}"));

        match self.deployer.rollback(deployment).await {
            Ok(()) => {
                let rollback = self
                    .rollback_manager
                    .execute(deployment, reason.to_string())
                    .await;
                deployment
                    .logs
                    .push(format!("Rollback completed (record id: {})", rollback.id));
            }
            Err(err) => {
                deployment.logs.push(format!("Rollback failed: {err}"));
                deployment.error_message = Some(format!("{reason}; rollback failed: {err}"));
            }
        }

        deployment.status = DeploymentStatus::Failed;
        deployment.completed_at = Some(chrono::Utc::now());
    }

    async fn run_health_checks(&self, deployment: &mut Deployment) -> bool {
        let healthy_threshold = self.health_config.healthy_threshold.max(1);
        let unhealthy_threshold = self.health_config.unhealthy_threshold.max(1);
        let max_attempts = self
            .health_config
            .retries
            .max(healthy_threshold + unhealthy_threshold)
            .max(1);

        let mut healthy_streak = 0u32;
        let mut unhealthy_streak = 0u32;

        for attempt in 1..=max_attempts {
            let result = tokio::time::timeout(
                self.health_config.timeout,
                self.deployer.health_check(deployment),
            )
            .await;

            let is_healthy = match result {
                Ok(Ok(true)) => {
                    deployment
                        .logs
                        .push(format!("Health check attempt {attempt}: healthy"));
                    true
                }
                Ok(Ok(false)) => {
                    deployment
                        .logs
                        .push(format!("Health check attempt {attempt}: unhealthy"));
                    false
                }
                Ok(Err(err)) => {
                    deployment
                        .logs
                        .push(format!("Health check attempt {attempt}: error: {err}"));
                    false
                }
                Err(_) => {
                    deployment.logs.push(format!(
                        "Health check attempt {attempt}: timeout after {:?}",
                        self.health_config.timeout
                    ));
                    false
                }
            };

            if is_healthy {
                healthy_streak += 1;
                unhealthy_streak = 0;
                if healthy_streak >= healthy_threshold {
                    return true;
                }
            } else {
                unhealthy_streak += 1;
                healthy_streak = 0;
                if unhealthy_streak >= unhealthy_threshold {
                    return false;
                }
            }

            if attempt < max_attempts {
                tokio::time::sleep(self.health_config.interval).await;
            }
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deployers::{DeployError, Deployer};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct MockDeployer {
        health_failures_before_success: usize,
        health_calls: AtomicUsize,
        rollback_calls: AtomicUsize,
    }

    impl MockDeployer {
        fn new(health_failures_before_success: usize) -> Self {
            Self {
                health_failures_before_success,
                health_calls: AtomicUsize::new(0),
                rollback_calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl Deployer for MockDeployer {
        fn name(&self) -> &str {
            "mock"
        }

        async fn build(&self, _deployment: &mut Deployment) -> DeployResult<()> {
            Ok(())
        }

        async fn push(&self, _deployment: &mut Deployment) -> DeployResult<()> {
            Ok(())
        }

        async fn deploy(
            &self,
            _config: &DeployConfig,
            _deployment: &mut Deployment,
        ) -> DeployResult<()> {
            Ok(())
        }

        async fn health_check(&self, _deployment: &Deployment) -> DeployResult<bool> {
            let call = self.health_calls.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(call > self.health_failures_before_success)
        }

        async fn rollback(&self, _deployment: &mut Deployment) -> DeployResult<()> {
            self.rollback_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct FailingDeployer;

    #[async_trait]
    impl Deployer for FailingDeployer {
        fn name(&self) -> &str {
            "failing"
        }

        async fn build(&self, _deployment: &mut Deployment) -> DeployResult<()> {
            Ok(())
        }

        async fn push(&self, _deployment: &mut Deployment) -> DeployResult<()> {
            Ok(())
        }

        async fn deploy(
            &self,
            _config: &DeployConfig,
            _deployment: &mut Deployment,
        ) -> DeployResult<()> {
            Ok(())
        }

        async fn health_check(&self, _deployment: &Deployment) -> DeployResult<bool> {
            Ok(false)
        }

        async fn rollback(&self, _deployment: &mut Deployment) -> DeployResult<()> {
            Err(DeployError::DeployFailed("rollback boom".to_string()))
        }
    }

    fn sample_config() -> DeployConfig {
        DeployConfig {
            target: DeployTarget::Docker,
            environment: "staging".to_string(),
            image: Some("registry/app:1".to_string()),
            tag: Some("1".to_string()),
            replicas: Some(1),
            resources: None,
        }
    }

    #[tokio::test]
    async fn test_pipeline_health_retries_then_success() {
        let mock = Arc::new(MockDeployer::new(1));
        let pipeline = DeploymentPipeline::new(mock).with_health_config(HealthCheckConfig {
            interval: std::time::Duration::from_millis(1),
            timeout: std::time::Duration::from_secs(1),
            retries: 3,
            healthy_threshold: 1,
            unhealthy_threshold: 3,
            ..HealthCheckConfig::default()
        });

        let deployment = Deployment::new(
            "proj".to_string(),
            "v1".to_string(),
            DeployTarget::Docker,
            "staging".to_string(),
        );

        let result = pipeline.run(deployment, &sample_config()).await.unwrap();
        assert_eq!(result.status, DeploymentStatus::Completed);
    }

    #[tokio::test]
    async fn test_pipeline_auto_rollback_on_health_failure() {
        let mock = Arc::new(MockDeployer::new(99));
        let pipeline =
            DeploymentPipeline::new(mock.clone()).with_health_config(HealthCheckConfig {
                interval: std::time::Duration::from_millis(1),
                timeout: std::time::Duration::from_secs(1),
                retries: 2,
                healthy_threshold: 1,
                unhealthy_threshold: 2,
                ..HealthCheckConfig::default()
            });

        let deployment = Deployment::new(
            "proj".to_string(),
            "v2".to_string(),
            DeployTarget::Docker,
            "staging".to_string(),
        );

        let result = pipeline.run(deployment, &sample_config()).await.unwrap();
        assert_eq!(result.status, DeploymentStatus::Failed);
        assert!(
            result
                .logs
                .iter()
                .any(|line| line.contains("Auto-rollback triggered"))
        );
        assert_eq!(mock.rollback_calls.load(Ordering::SeqCst), 1);
        assert_eq!(pipeline.rollback_history().len(), 1);
    }

    #[tokio::test]
    async fn test_pipeline_records_rollback_failure() {
        let pipeline = DeploymentPipeline::new(Arc::new(FailingDeployer)).with_health_config(
            HealthCheckConfig {
                interval: std::time::Duration::from_millis(1),
                timeout: std::time::Duration::from_secs(1),
                retries: 1,
                healthy_threshold: 1,
                unhealthy_threshold: 1,
                ..HealthCheckConfig::default()
            },
        );

        let deployment = Deployment::new(
            "proj".to_string(),
            "v3".to_string(),
            DeployTarget::Docker,
            "staging".to_string(),
        );

        let result = pipeline.run(deployment, &sample_config()).await.unwrap();
        assert_eq!(result.status, DeploymentStatus::Failed);
        assert!(
            result
                .error_message
                .unwrap_or_default()
                .contains("rollback failed")
        );
    }
}
