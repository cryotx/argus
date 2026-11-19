//! Deployment types

use serde::{Deserialize, Serialize};

/// Deployment status
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentStatus {
    #[default]
    Pending,
    Building,
    Deploying,
    RollingBack,
    Completed,
    Failed,
}

/// Deployment target type
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeployTarget {
    #[default]
    Docker,
    Kubernetes,
    Server,
    S3,
    Vercel,
    Netlify,
}

/// A deployment record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Deployment {
    pub id: String,
    pub project_id: String,
    pub version: String,
    pub target: DeployTarget,
    pub status: DeploymentStatus,
    pub environment: String,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub logs: Vec<String>,
    pub error_message: Option<String>,
}

impl Deployment {
    pub fn new(
        project_id: String,
        version: String,
        target: DeployTarget,
        environment: String,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            project_id,
            version,
            target,
            status: DeploymentStatus::Pending,
            environment,
            started_at: None,
            completed_at: None,
            logs: Vec::new(),
            error_message: None,
        }
    }
}

/// Deployment configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployConfig {
    pub target: DeployTarget,
    pub environment: String,
    pub image: Option<String>,
    pub tag: Option<String>,
    pub replicas: Option<u32>,
    pub resources: Option<ResourceLimits>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    pub cpu_limit: Option<String>,
    pub memory_limit: Option<String>,
}
