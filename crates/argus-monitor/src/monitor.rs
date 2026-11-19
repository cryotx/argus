//! System monitor - orchestrates metrics collection and monitoring

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::time::interval;
use tracing::{debug, error, info, warn};

use crate::alert::{Alert, AlertLevel, AlertManager};
use crate::health::{HealthChecker, HealthStatus};
use crate::metrics::{AppMetrics, SystemMetrics};

/// Configuration for system monitoring
#[derive(Debug, Clone)]
pub struct MonitorConfig {
    pub poll_interval: Duration,
    pub cpu_threshold: f64,
    pub memory_threshold: f64,
    pub disk_threshold: f64,
    pub enable_alerts: bool,
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(10),
            cpu_threshold: 80.0,
            memory_threshold: 85.0,
            disk_threshold: 90.0,
            enable_alerts: true,
        }
    }
}

/// System monitor - collects and monitors system metrics
pub struct SystemMonitor {
    config: MonitorConfig,
    alert_manager: Arc<AlertManager>,
    last_metrics: RwLock<Option<SystemMetrics>>,
    app_metrics: RwLock<AppMetrics>,
    health_checkers: RwLock<Vec<Box<dyn HealthChecker>>>,
}

impl SystemMonitor {
    /// Create a new system monitor
    pub fn new(config: MonitorConfig) -> Self {
        Self {
            config,
            alert_manager: Arc::new(AlertManager::new(100)),
            last_metrics: RwLock::new(None),
            app_metrics: RwLock::new(AppMetrics::new()),
            health_checkers: RwLock::new(Vec::new()),
        }
    }

    /// Create with default config
    pub fn with_defaults() -> Self {
        Self::new(MonitorConfig::default())
    }

    /// Get alert manager reference
    pub fn alert_manager(&self) -> Arc<AlertManager> {
        Arc::clone(&self.alert_manager)
    }

    /// Register a health checker
    pub async fn register_health_checker(&self, checker: Box<dyn HealthChecker>) {
        let mut checkers = self.health_checkers.write().await;
        checkers.push(checker);
    }

    /// Update application metrics
    pub async fn update_app_metrics(&self, metrics: AppMetrics) {
        let mut app = self.app_metrics.write().await;
        *app = metrics;
    }

    /// Collect current system metrics
    pub async fn collect_metrics(&self) -> anyhow::Result<SystemMetrics> {
        let metrics = SystemMetrics::collect().await?;

        // Store for comparison
        {
            let mut last = self.last_metrics.write().await;
            *last = Some(metrics.clone());
        }

        // Check thresholds and generate alerts
        if self.config.enable_alerts {
            self.check_thresholds(&metrics).await;
        }

        Ok(metrics)
    }

    /// Check metrics against thresholds and generate alerts
    async fn check_thresholds(&self, metrics: &SystemMetrics) {
        // CPU alert
        if metrics.cpu.usage_percent > self.config.cpu_threshold {
            let alert = Alert::new(
                AlertLevel::Warning,
                "High CPU usage".to_string(),
                format!("CPU usage is at {:.1}%", metrics.cpu.usage_percent),
            );
            self.alert_manager.send(alert).await;
        }

        // Memory alert
        if metrics.memory.usage_percent > self.config.memory_threshold {
            let alert = Alert::new(
                AlertLevel::Warning,
                "High memory usage".to_string(),
                format!("Memory usage is at {:.1}%", metrics.memory.usage_percent),
            );
            self.alert_manager.send(alert).await;
        }

        // Disk alert
        if metrics.disk.usage_percent > self.config.disk_threshold {
            let alert = Alert::new(
                AlertLevel::Critical,
                "High disk usage".to_string(),
                format!("Disk usage is at {:.1}%", metrics.disk.usage_percent),
            );
            self.alert_manager.send(alert).await;
        }
    }

    /// Get latest metrics
    pub async fn get_last_metrics(&self) -> Option<SystemMetrics> {
        let last = self.last_metrics.read().await;
        last.clone()
    }

    /// Get current app metrics
    pub async fn get_app_metrics(&self) -> AppMetrics {
        let app = self.app_metrics.read().await;
        app.clone()
    }

    /// Run health checks
    pub async fn run_health_checks(&self) -> Vec<(String, HealthStatus)> {
        let checkers = self.health_checkers.read().await;
        let mut results = Vec::new();

        for checker in checkers.iter() {
            let name = checker.name().to_string();
            let status = checker.check().await;
            results.push((name, status));
        }

        results
    }

    /// Start the monitoring loop
    pub async fn start_monitoring(&self) {
        let mut ticker = interval(self.config.poll_interval);
        let poll_interval = self.config.poll_interval;

        info!("Starting system monitor with interval {:?}", poll_interval);

        loop {
            ticker.tick().await;

            // Collect metrics
            match self.collect_metrics().await {
                Ok(metrics) => {
                    debug!(
                        cpu = metrics.cpu.usage_percent,
                        memory = metrics.memory.usage_percent,
                        disk = metrics.disk.usage_percent,
                        "Collected system metrics"
                    );
                }
                Err(e) => {
                    error!(error = %e, "Failed to collect metrics");
                }
            }

            // Run health checks
            let health_results = self.run_health_checks().await;
            for (name, status) in &health_results {
                if !status.is_healthy() {
                    warn!(checker = %name, status = ?status, "Health check failed");
                    if self.config.enable_alerts {
                        let alert = Alert::new(
                            AlertLevel::Warning,
                            format!("Health check failed: {}", name),
                            status.message.clone().unwrap_or_default(),
                        );
                        self.alert_manager.send(alert).await;
                    }
                }
            }
        }
    }
}

impl Default for SystemMonitor {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_system_monitor_creation() {
        let monitor = SystemMonitor::with_defaults();
        assert!(monitor.get_last_metrics().await.is_none());
    }

    #[tokio::test]
    async fn test_collect_metrics() {
        let monitor = SystemMonitor::with_defaults();
        let metrics = monitor.collect_metrics().await.unwrap();

        assert!(metrics.cpu.usage_percent >= 0.0);
        assert!(metrics.memory.total_bytes > 0);
    }

    #[tokio::test]
    async fn test_app_metrics_update() {
        let monitor = SystemMonitor::with_defaults();

        let app_metrics = AppMetrics::with_worker_stats(2, 3, 10, 1);
        monitor.update_app_metrics(app_metrics).await;

        let retrieved = monitor.get_app_metrics().await;
        assert_eq!(retrieved.workers_active, 2);
        assert_eq!(retrieved.workers_idle, 3);
    }
}
