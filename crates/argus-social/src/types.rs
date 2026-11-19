use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Telegram,
    Discord,
    Slack,
    Wechat,
    Feishu,
    GoogleChat,
    Whatsapp,
    Qq,
    WebDashboard,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageDirection {
    Incoming,
    Outgoing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationLevel {
    Info,
    Warning,
    Error,
    Critical,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SocialMessage {
    pub id: Uuid,
    pub platform: Platform,
    pub channel_id: String,
    pub user_id: String,
    pub text: String,
    pub direction: MessageDirection,
    pub timestamp: DateTime<Utc>,
}

impl SocialMessage {
    pub fn incoming(
        platform: Platform,
        channel_id: impl Into<String>,
        user_id: impl Into<String>,
        text: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            platform,
            channel_id: channel_id.into(),
            user_id: user_id.into(),
            text: text.into(),
            direction: MessageDirection::Incoming,
            timestamp: Utc::now(),
        }
    }

    pub fn outgoing(
        platform: Platform,
        channel_id: impl Into<String>,
        user_id: impl Into<String>,
        text: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            platform,
            channel_id: channel_id.into(),
            user_id: user_id.into(),
            text: text.into(),
            direction: MessageDirection::Outgoing,
            timestamp: Utc::now(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Notification {
    pub id: Uuid,
    pub level: NotificationLevel,
    pub title: String,
    pub body: String,
    pub platforms: Vec<Platform>,
    pub created_at: DateTime<Utc>,
}

impl Notification {
    pub fn new(
        level: NotificationLevel,
        title: impl Into<String>,
        body: impl Into<String>,
        platforms: Vec<Platform>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            level,
            title: title.into(),
            body: body.into(),
            platforms,
            created_at: Utc::now(),
        }
    }
}
