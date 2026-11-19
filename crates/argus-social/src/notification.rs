use crate::types::{Notification, NotificationLevel, Platform};

pub struct NotificationEngine;

impl NotificationEngine {
    pub fn new() -> Self {
        Self
    }

    pub fn make(
        &self,
        level: NotificationLevel,
        title: impl Into<String>,
        body: impl Into<String>,
    ) -> Notification {
        let platforms = match level {
            NotificationLevel::Info => vec![Platform::WebDashboard],
            NotificationLevel::Warning => vec![Platform::WebDashboard, Platform::Telegram],
            NotificationLevel::Error | NotificationLevel::Critical => {
                vec![Platform::WebDashboard, Platform::Telegram, Platform::Slack]
            }
        };

        Notification::new(level, title, body, platforms)
    }
}

impl Default for NotificationEngine {
    fn default() -> Self {
        Self::new()
    }
}
