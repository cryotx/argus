//! Health checking system

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Health check status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthStatus {
    pub healthy: bool,
    pub message: Option<String>,
    pub duration_ms: Option<u64>,
}

impl HealthStatus {
    pub fn healthy() -> Self {
        Self {
            healthy: true,
            message: None,
            duration_ms: None,
        }
    }

    pub fn unhealthy(message: impl Into<String>) -> Self {
        Self {
            healthy: false,
            message: Some(message.into()),
            duration_ms: None,
        }
    }

    pub fn with_duration(mut self, duration: Duration) -> Self {
        self.duration_ms = Some(duration.as_millis() as u64);
        self
    }

    pub fn is_healthy(&self) -> bool {
        self.healthy
    }
}

/// Health checker trait
#[async_trait]
pub trait HealthChecker: Send + Sync {
    /// Name of the health checker
    fn name(&self) -> &str;

    /// Perform health check
    async fn check(&self) -> HealthStatus;
}

/// Database health checker
pub struct DatabaseHealthChecker {
    name: String,
}

impl DatabaseHealthChecker {
    pub fn new() -> Self {
        Self {
            name: "database".to_string(),
        }
    }
}

impl Default for DatabaseHealthChecker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HealthChecker for DatabaseHealthChecker {
    fn name(&self) -> &str {
        &self.name
    }

    async fn check(&self) -> HealthStatus {
        // In real implementation, would check database connectivity
        // For now, return healthy as placeholder
        HealthStatus::healthy()
    }
}

/// Memory health checker
pub struct MemoryHealthChecker {
    threshold_percent: f64,
}

impl MemoryHealthChecker {
    pub fn new(threshold_percent: f64) -> Self {
        Self { threshold_percent }
    }
}

impl Default for MemoryHealthChecker {
    fn default() -> Self {
        Self::new(90.0)
    }
}

#[async_trait]
impl HealthChecker for MemoryHealthChecker {
    fn name(&self) -> &str {
        "memory"
    }

    async fn check(&self) -> HealthStatus {
        match crate::metrics::SystemMetrics::collect().await {
            Ok(metrics) => {
                if metrics.memory.usage_percent > self.threshold_percent {
                    HealthStatus::unhealthy(format!(
                        "Memory usage at {:.1}%",
                        metrics.memory.usage_percent
                    ))
                } else {
                    HealthStatus::healthy()
                }
            }
            Err(e) => HealthStatus::unhealthy(format!("Failed to check memory: {}", e)),
        }
    }
}

/// Disk health checker
pub struct DiskHealthChecker {
    threshold_percent: f64,
}

impl DiskHealthChecker {
    pub fn new(threshold_percent: f64) -> Self {
        Self { threshold_percent }
    }
}

impl Default for DiskHealthChecker {
    fn default() -> Self {
        Self::new(95.0)
    }
}

#[async_trait]
impl HealthChecker for DiskHealthChecker {
    fn name(&self) -> &str {
        "disk"
    }

    async fn check(&self) -> HealthStatus {
        match crate::metrics::SystemMetrics::collect().await {
            Ok(metrics) => {
                if metrics.disk.usage_percent > self.threshold_percent {
                    HealthStatus::unhealthy(format!(
                        "Disk usage at {:.1}%",
                        metrics.disk.usage_percent
                    ))
                } else {
                    HealthStatus::healthy()
                }
            }
            Err(e) => HealthStatus::unhealthy(format!("Failed to check disk: {}", e)),
        }
    }
}

/// Composite health checker that runs multiple checkers
pub struct CompositeHealthChecker {
    checkers: Vec<Box<dyn HealthChecker>>,
}

impl CompositeHealthChecker {
    pub fn new() -> Self {
        Self {
            checkers: Vec::new(),
        }
    }

    pub fn add_checker(mut self, checker: Box<dyn HealthChecker>) -> Self {
        self.checkers.push(checker);
        self
    }

    pub async fn check_all(&self) -> Vec<(String, HealthStatus)> {
        let mut results = Vec::new();
        for checker in &self.checkers {
            let name = checker.name().to_string();
            let status = checker.check().await;
            results.push((name, status));
        }
        results
    }

    pub fn overall_status(&self, results: &[(String, HealthStatus)]) -> HealthStatus {
        let all_healthy = results.iter().all(|(_, status)| status.healthy);
        if all_healthy {
            HealthStatus::healthy()
        } else {
            let unhealthy: Vec<String> = results
                .iter()
                .filter(|(_, s)| !s.healthy)
                .map(|(name, _)| name.clone())
                .collect();
            HealthStatus::unhealthy(format!("Unhealthy components: {}", unhealthy.join(", ")))
        }
    }
}

impl Default for CompositeHealthChecker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_health_status() {
        let healthy = HealthStatus::healthy();
        assert!(healthy.is_healthy());

        let unhealthy = HealthStatus::unhealthy("Test error");
        assert!(!unhealthy.is_healthy());
    }

    #[tokio::test]
    async fn test_memory_health_checker() {
        let checker = MemoryHealthChecker::new(100.0);
        let status = checker.check().await;
        assert!(status.is_healthy());
    }
}
