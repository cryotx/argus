//! Health check for deployments

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Health check result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckResult {
    pub healthy: bool,
    pub message: String,
    pub latency_ms: u64,
    pub checked_at: chrono::DateTime<chrono::Utc>,
}

/// Health check configuration
#[derive(Debug, Clone)]
pub struct HealthCheckConfig {
    pub path: String,
    pub interval: Duration,
    pub timeout: Duration,
    pub retries: u32,
    pub healthy_threshold: u32,
    pub unhealthy_threshold: u32,
}

impl Default for HealthCheckConfig {
    fn default() -> Self {
        Self {
            path: "/health".to_string(),
            interval: Duration::from_secs(30),
            timeout: Duration::from_secs(10),
            retries: 3,
            healthy_threshold: 2,
            unhealthy_threshold: 3,
        }
    }
}

/// Health checker trait
#[async_trait::async_trait]
pub trait HealthChecker: Send + Sync {
    /// Check if the deployment is healthy
    async fn check(&self) -> HealthCheckResult;
}

/// HTTP health checker
pub struct HttpHealthChecker {
    url: String,
    client: reqwest::Client,
}

impl HttpHealthChecker {
    pub fn new(url: String) -> Self {
        Self {
            url,
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait::async_trait]
impl HealthChecker for HttpHealthChecker {
    async fn check(&self) -> HealthCheckResult {
        let start = std::time::Instant::now();

        match self.client.get(&self.url).send().await {
            Ok(response) => {
                let latency = start.elapsed().as_millis() as u64;
                let healthy = response.status().is_success();

                HealthCheckResult {
                    healthy,
                    message: if healthy {
                        "OK".to_string()
                    } else {
                        format!("Status: {}", response.status())
                    },
                    latency_ms: latency,
                    checked_at: chrono::Utc::now(),
                }
            }
            Err(e) => HealthCheckResult {
                healthy: false,
                message: e.to_string(),
                latency_ms: start.elapsed().as_millis() as u64,
                checked_at: chrono::Utc::now(),
            },
        }
    }
}

/// TCP health checker
pub struct TcpHealthChecker {
    host: String,
    port: u16,
}

impl TcpHealthChecker {
    pub fn new(host: String, port: u16) -> Self {
        Self { host, port }
    }
}

#[async_trait::async_trait]
impl HealthChecker for TcpHealthChecker {
    async fn check(&self) -> HealthCheckResult {
        let addr = format!("{}:{}", self.host, self.port);
        let start = std::time::Instant::now();

        let result = tokio::net::TcpStream::connect(&addr).await;

        let latency = start.elapsed().as_millis() as u64;

        match result {
            Ok(_) => HealthCheckResult {
                healthy: true,
                message: "TCP connection successful".to_string(),
                latency_ms: latency,
                checked_at: chrono::Utc::now(),
            },
            Err(e) => HealthCheckResult {
                healthy: false,
                message: e.to_string(),
                latency_ms: latency,
                checked_at: chrono::Utc::now(),
            },
        }
    }
}
