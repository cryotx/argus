//! Deployer implementations

pub mod docker;
pub mod kubernetes;
pub mod server;
pub mod vps;

pub use docker::DockerDeployer;
pub use kubernetes::KubernetesDeployer;
pub use server::ServerDeployer;
pub use vps::{SshVpsDeployer, VpsDeployRequest, VpsDeployResult, VpsDeployer};

use crate::types::{DeployConfig, Deployment};
use async_trait::async_trait;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum DeployError {
    #[error("Build failed: {0}")]
    BuildFailed(String),
    #[error("Push failed: {0}")]
    PushFailed(String),
    #[error("Deploy failed: {0}")]
    DeployFailed(String),
    #[error("Authentication failed: {0}")]
    AuthFailed(String),
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),
}

/// Result type for deployer operations
pub type DeployResult<T> = Result<T, DeployError>;

/// Trait for deployment backends
#[async_trait]
pub trait Deployer: Send + Sync {
    /// Get the deployer name
    fn name(&self) -> &str;

    /// Build the deployment artifact
    async fn build(&self, deployment: &mut Deployment) -> DeployResult<()>;

    /// Push the artifact to registry
    async fn push(&self, deployment: &mut Deployment) -> DeployResult<()>;

    /// Deploy to target
    async fn deploy(&self, config: &DeployConfig, deployment: &mut Deployment) -> DeployResult<()>;

    /// Check deployment health
    async fn health_check(&self, deployment: &Deployment) -> DeployResult<bool>;

    /// Rollback to previous version
    async fn rollback(&self, deployment: &mut Deployment) -> DeployResult<()>;
}
