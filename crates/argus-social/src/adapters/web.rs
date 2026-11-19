use async_trait::async_trait;

use argus_core::types::{ArgusError, ArgusResult};

use crate::adapter::PlatformAdapter;
use crate::types::{Platform, SocialMessage};

#[derive(Clone, Debug, Default)]
pub struct WebDashboardAdapter;

impl WebDashboardAdapter {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl PlatformAdapter for WebDashboardAdapter {
    fn platform(&self) -> Platform {
        Platform::WebDashboard
    }

    async fn send_message(&self, _channel_id: &str, _text: &str) -> ArgusResult<()> {
        Ok(())
    }

    async fn health_check(&self) -> ArgusResult<bool> {
        Ok(true)
    }

    async fn normalize_incoming(&self, payload: serde_json::Value) -> ArgusResult<SocialMessage> {
        let session = payload
            .get("session_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ArgusError::Social("web payload missing session_id".to_string()))?;
        let user_id = payload
            .get("user_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ArgusError::Social("web payload missing user_id".to_string()))?;
        let text = payload
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ArgusError::Social("web payload missing text".to_string()))?;

        Ok(SocialMessage::incoming(
            Platform::WebDashboard,
            session,
            user_id,
            text,
        ))
    }
}
