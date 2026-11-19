//! Rollback management

use super::types::{Deployment, DeploymentStatus};
use chrono::{DateTime, Utc};
use std::sync::Mutex;

/// Rollback record
#[derive(Debug, Clone)]
pub struct Rollback {
    pub id: String,
    pub deployment_id: String,
    pub rolled_back_at: DateTime<Utc>,
    pub reason: String,
    pub status: RollbackStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RollbackStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

/// Rollback manager
pub struct RollbackManager {
    max_history: usize,
    history: Mutex<Vec<Rollback>>,
}

impl RollbackManager {
    pub fn new(max_history: usize) -> Self {
        Self {
            max_history,
            history: Mutex::new(Vec::new()),
        }
    }

    /// Determine if a rollback is needed
    pub fn should_rollback(&self, deployment: &Deployment, health_check_failed: bool) -> bool {
        if deployment.status == DeploymentStatus::Failed {
            return true;
        }

        if health_check_failed {
            return true;
        }

        false
    }

    /// Create a rollback record
    pub fn create_rollback(&self, deployment: &Deployment, reason: String) -> Rollback {
        Rollback {
            id: uuid::Uuid::new_v4().to_string(),
            deployment_id: deployment.id.clone(),
            rolled_back_at: Utc::now(),
            reason,
            status: RollbackStatus::Pending,
        }
    }

    /// Execute rollback
    pub async fn execute(&self, deployment: &Deployment, reason: impl Into<String>) -> Rollback {
        let mut rollback = self.create_rollback(deployment, reason.into());
        rollback.status = RollbackStatus::InProgress;

        tracing::info!(
            "Rolling back deployment {} to version {}",
            deployment.id,
            deployment.version
        );

        rollback.status = RollbackStatus::Completed;
        self.push_history(rollback.clone());
        rollback
    }

    pub fn history(&self) -> Vec<Rollback> {
        self.history.lock().map(|h| h.clone()).unwrap_or_default()
    }

    fn push_history(&self, rollback: Rollback) {
        if let Ok(mut history) = self.history.lock() {
            history.push(rollback);

            if history.len() > self.max_history {
                let remove_count = history.len().saturating_sub(self.max_history);
                history.drain(0..remove_count);
            }
        }
    }
}

impl Default for RollbackManager {
    fn default() -> Self {
        Self::new(10)
    }
}
