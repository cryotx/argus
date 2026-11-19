//! argus-monitor: System monitoring, watchdog, alerting
//!
//! Provides system resource monitoring, worker health checks, and alerting.

pub mod alert;
pub mod health;
pub mod metrics;
pub mod monitor;

pub use alert::{Alert, AlertLevel, AlertManager};
pub use health::{HealthChecker, HealthStatus};
pub use metrics::SystemMetrics;
pub use monitor::SystemMonitor;
