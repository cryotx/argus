use async_trait::async_trait;
use reqwest::Client;
use serde::Serialize;

use argus_core::types::{ArgusError, ArgusResult};

use crate::adapter::PlatformAdapter;
use crate::types::{Platform, SocialMessage};

#[derive(Clone, Debug)]
pub struct GoogleChatAdapter {
    pub webhook_url: String,
    client: Client,
}

impl GoogleChatAdapter {
    pub fn new(webhook_url: impl Into<String>) -> Self {
        Self {
            webhook_url: webhook_url.into(),
            client: Client::new(),
        }
    }
}

#[derive(Serialize)]
struct GoogleChatSendRequest<'a> {
    text: &'a str,
}

#[async_trait]
impl PlatformAdapter for GoogleChatAdapter {
    fn platform(&self) -> Platform {
        Platform::GoogleChat
    }

    async fn send_message(&self, _channel_id: &str, text: &str) -> ArgusResult<()> {
        if self.webhook_url.trim().is_empty() {
            return Err(ArgusError::Social(
                "google chat webhook url missing".to_string(),
            ));
        }

        let payload = GoogleChatSendRequest { text };

        let response = self
            .client
            .post(&self.webhook_url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| ArgusError::Social(format!("google chat request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ArgusError::Social(format!(
                "google chat send failed ({status}): {body}"
            )));
        }

        Ok(())
    }

    async fn health_check(&self) -> ArgusResult<bool> {
        Ok(!self.webhook_url.trim().is_empty())
    }

    async fn normalize_incoming(&self, payload: serde_json::Value) -> ArgusResult<SocialMessage> {
        let space = payload
            .get("space")
            .and_then(|v| v.get("name"))
            .and_then(|v| v.as_str())
            .or_else(|| payload.get("space_name").and_then(|v| v.as_str()))
            .ok_or_else(|| ArgusError::Social("google chat payload missing space".to_string()))?;

        let user = payload
            .get("user")
            .and_then(|v| v.get("name"))
            .and_then(|v| v.as_str())
            .or_else(|| payload.get("user_id").and_then(|v| v.as_str()))
            .ok_or_else(|| ArgusError::Social("google chat payload missing user".to_string()))?;

        let text = payload
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ArgusError::Social("google chat payload missing text".to_string()))?;

        Ok(SocialMessage::incoming(
            Platform::GoogleChat,
            space,
            user,
            text,
        ))
    }
}
