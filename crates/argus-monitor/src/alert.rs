//! Alert system

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

type AlertSubscriber = Arc<dyn Fn(Alert) + Send + Sync>;

/// Alert severity level
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlertLevel {
    Info,
    Warning,
    Critical,
}

/// An alert message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    pub id: String,
    pub level: AlertLevel,
    pub title: String,
    pub message: String,
    pub timestamp: DateTime<Utc>,
    pub resolved: bool,
}

impl Alert {
    pub fn new(level: AlertLevel, title: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            level,
            title: title.into(),
            message: message.into(),
            timestamp: Utc::now(),
            resolved: false,
        }
    }

    pub fn resolve(&mut self) {
        self.resolved = true;
    }

    pub fn is_critical(&self) -> bool {
        self.level == AlertLevel::Critical
    }

    pub fn is_warning(&self) -> bool {
        self.level == AlertLevel::Warning
    }
}

/// Alert manager - handles alert storage and notification
pub struct AlertManager {
    alerts: RwLock<VecDeque<Alert>>,
    max_alerts: usize,
    subscribers: RwLock<Vec<AlertSubscriber>>,
}

impl AlertManager {
    /// Create a new alert manager
    pub fn new(max_alerts: usize) -> Self {
        Self {
            alerts: RwLock::new(VecDeque::with_capacity(max_alerts)),
            max_alerts,
            subscribers: RwLock::new(Vec::new()),
        }
    }

    /// Create with default capacity
    pub fn with_defaults() -> Self {
        Self::new(100)
    }

    /// Send an alert
    pub async fn send(&self, alert: Alert) {
        info!(
            level = ?alert.level,
            title = %alert.title,
            "Alert triggered"
        );

        // Store alert
        {
            let mut alerts = self.alerts.write().await;
            if alerts.len() >= self.max_alerts {
                alerts.pop_front();
            }
            alerts.push_back(alert.clone());
        }

        // Notify subscribers
        self.notify_subscribers(alert).await;
    }

    /// Get all alerts
    pub async fn get_alerts(&self, include_resolved: bool) -> Vec<Alert> {
        let alerts = self.alerts.read().await;
        if include_resolved {
            alerts.iter().cloned().collect()
        } else {
            alerts.iter().filter(|a| !a.resolved).cloned().collect()
        }
    }

    /// Get unresolved alerts
    pub async fn get_unresolved(&self) -> Vec<Alert> {
        self.get_alerts(false).await
    }

    /// Get alerts by level
    pub async fn get_by_level(&self, level: AlertLevel) -> Vec<Alert> {
        let alerts = self.alerts.read().await;
        alerts
            .iter()
            .filter(|a| a.level == level && !a.resolved)
            .cloned()
            .collect()
    }

    /// Get critical alerts
    pub async fn get_critical(&self) -> Vec<Alert> {
        self.get_by_level(AlertLevel::Critical).await
    }

    /// Get warning alerts
    pub async fn get_warnings(&self) -> Vec<Alert> {
        self.get_by_level(AlertLevel::Warning).await
    }

    /// Resolve an alert
    pub async fn resolve(&self, alert_id: &str) -> bool {
        let mut alerts = self.alerts.write().await;
        if let Some(alert) = alerts.iter_mut().find(|a| a.id == alert_id) {
            alert.resolve();
            info!(alert_id = %alert_id, "Alert resolved");
            return true;
        }
        false
    }

    /// Clear all alerts
    pub async fn clear(&self) {
        let mut alerts = self.alerts.write().await;
        alerts.clear();
    }

    /// Subscribe to alerts
    pub async fn subscribe<F>(&self, callback: Arc<F>)
    where
        F: Fn(Alert) + Send + Sync + 'static,
    {
        let mut subs = self.subscribers.write().await;
        subs.push(callback);
    }

    async fn notify_subscribers(&self, alert: Alert) {
        let subs = self.subscribers.read().await;
        for callback in subs.iter() {
            callback(alert.clone());
        }
    }

    /// Get alert count
    pub async fn count(&self) -> usize {
        let alerts = self.alerts.read().await;
        alerts.len()
    }

    /// Get unresolved count
    pub async fn unresolved_count(&self) -> usize {
        let alerts = self.alerts.read().await;
        alerts.iter().filter(|a| !a.resolved).count()
    }
}

impl Default for AlertManager {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_alert_creation() {
        let alert = Alert::new(AlertLevel::Warning, "Test", "Test message");
        assert_eq!(alert.level, AlertLevel::Warning);
        assert!(!alert.resolved);
    }

    #[tokio::test]
    async fn test_alert_manager_send() {
        let manager = AlertManager::with_defaults();
        let alert = Alert::new(AlertLevel::Info, "Test", "Test message");

        manager.send(alert).await;
        assert_eq!(manager.count().await, 1);
    }

    #[tokio::test]
    async fn test_alert_resolution() {
        let manager = AlertManager::with_defaults();
        let alert = Alert::new(AlertLevel::Info, "Test", "Test message");
        let id = alert.id.clone();

        manager.send(alert).await;
        assert!(manager.resolve(&id).await);
    }
}
