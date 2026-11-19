use async_trait::async_trait;
use reqwest::Client;
use serde::Serialize;

use argus_core::types::{ArgusError, ArgusResult};

use crate::adapter::PlatformAdapter;
use crate::types::{Platform, SocialMessage};

#[derive(Clone, Debug)]
pub struct FeishuAdapter {
    pub webhook_url: String,
    client: Client,
}

impl FeishuAdapter {
    pub fn new(webhook_url: impl Into<String>) -> Self {
        Self {
            webhook_url: webhook_url.into(),
            client: Client::new(),
        }
    }
}

#[derive(Serialize)]
struct FeishuTextContent<'a> {
    text: &'a str,
}

#[derive(Serialize)]
struct FeishuSendRequest<'a> {
    msg_type: &'static str,
    content: FeishuTextContent<'a>,
}

#[async_trait]
impl PlatformAdapter for FeishuAdapter {
    fn platform(&self) -> Platform {
        Platform::Feishu
    }

    async fn send_message(&self, _channel_id: &str, text: &str) -> ArgusResult<()> {
        if self.webhook_url.trim().is_empty() {
            return Err(ArgusError::Social("feishu webhook url missing".to_string()));
        }

        let payload = FeishuSendRequest {
            msg_type: "text",
            content: FeishuTextContent { text },
        };

        let response = self
            .client
            .post(&self.webhook_url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| ArgusError::Social(format!("feishu request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ArgusError::Social(format!(
                "feishu send failed ({status}): {body}"
            )));
        }

        Ok(())
    }

    async fn health_check(&self) -> ArgusResult<bool> {
        Ok(!self.webhook_url.trim().is_empty())
    }

    async fn normalize_incoming(&self, payload: serde_json::Value) -> ArgusResult<SocialMessage> {
        let chat_id = payload
            .get("chat_id")
            .and_then(|v| v.as_str())
            .or_else(|| payload.get("open_chat_id").and_then(|v| v.as_str()))
            .ok_or_else(|| ArgusError::Social("feishu payload missing chat_id".to_string()))?;

        let user_id = payload
            .get("user_id")
            .and_then(|v| v.as_str())
            .or_else(|| payload.get("open_id").and_then(|v| v.as_str()))
            .ok_or_else(|| ArgusError::Social("feishu payload missing user_id".to_string()))?;

        let text = payload
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ArgusError::Social("feishu payload missing text".to_string()))?;

        Ok(SocialMessage::incoming(
            Platform::Feishu,
            chat_id,
            user_id,
            text,
        ))
    }
}
