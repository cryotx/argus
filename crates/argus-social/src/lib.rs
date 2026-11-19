//! argus-social: Social gateway, platform adapters, notification engine

pub mod adapter;
pub mod adapters;
pub mod conversation;
pub mod gateway;
pub mod notification;
pub mod types;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::adapters::telegram::TelegramAdapter;
    use crate::adapters::web::WebDashboardAdapter;
    use crate::conversation::ConversationManager;
    use crate::gateway::SocialGateway;
    use crate::notification::NotificationEngine;
    use crate::types::{NotificationLevel, Platform, SocialMessage};

    #[tokio::test]
    async fn test_gateway_send() {
        let gateway = SocialGateway::new();
        gateway
            .register_adapter(Arc::new(WebDashboardAdapter::new()))
            .unwrap();

        gateway
            .send(Platform::WebDashboard, "session-1", "hello")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_telegram_normalize() {
        let gateway = SocialGateway::new();
        gateway
            .register_adapter(Arc::new(TelegramAdapter::new("token")))
            .unwrap();

        let msg = gateway
            .normalize(
                Platform::Telegram,
                serde_json::json!({
                    "chat_id": "c1",
                    "user_id": "u1",
                    "text": "nihao"
                }),
            )
            .await
            .unwrap();
        assert_eq!(msg.channel_id, "c1");
    }

    #[test]
    fn test_notification_engine() {
        let engine = NotificationEngine::new();
        let n = engine.make(NotificationLevel::Critical, "title", "body");
        assert!(n.platforms.contains(&Platform::Telegram));
    }

    #[test]
    fn test_conversation_append() {
        let manager = ConversationManager::new();
        let msg = SocialMessage::incoming(Platform::WebDashboard, "s1", "u1", "hello");
        let sid = manager.append(msg).unwrap();
        let s = manager.get("s1", "u1").unwrap().unwrap();
        assert_eq!(s.id, sid);
        assert_eq!(s.messages.len(), 1);
    }
}
