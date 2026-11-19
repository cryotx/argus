use async_trait::async_trait;
use reqwest::Client;
use serde::Serialize;

use argus_core::types::{ArgusError, ArgusResult};

use crate::adapter::PlatformAdapter;
use crate::types::{Platform, SocialMessage};

#[derive(Clone, Debug)]
pub struct TelegramAdapter {
    pub bot_token: String,
    pub api_base_url: String,
    client: Client,
}

impl TelegramAdapter {
    pub fn new(bot_token: impl Into<String>) -> Self {
        Self::new_with_base_url(bot_token, "https://api.telegram.org")
    }

    pub fn new_with_base_url(
        bot_token: impl Into<String>,
        api_base_url: impl Into<String>,
    ) -> Self {
        Self {
            bot_token: bot_token.into(),
            api_base_url: api_base_url.into(),
            client: Client::new(),
        }
    }

    fn send_message_url(&self) -> String {
        let base = self.api_base_url.trim_end_matches('/');
        format!("{base}/bot{}/sendMessage", self.bot_token)
    }
}

#[derive(Serialize)]
struct TelegramSendRequest<'a> {
    chat_id: &'a str,
    text: &'a str,
}

#[async_trait]
impl PlatformAdapter for TelegramAdapter {
    fn platform(&self) -> Platform {
        Platform::Telegram
    }

    async fn send_message(&self, channel_id: &str, text: &str) -> ArgusResult<()> {
        if self.bot_token.trim().is_empty() {
            return Err(ArgusError::Social("telegram token missing".to_string()));
        }

        let payload = TelegramSendRequest {
            chat_id: channel_id,
            text,
        };

        let response = self
            .client
            .post(self.send_message_url())
            .json(&payload)
            .send()
            .await
            .map_err(|e| ArgusError::Social(format!("telegram request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ArgusError::Social(format!(
                "telegram send failed ({status}): {body}"
            )));
        }

        Ok(())
    }

    async fn health_check(&self) -> ArgusResult<bool> {
        Ok(!self.bot_token.trim().is_empty())
    }

    async fn normalize_incoming(&self, payload: serde_json::Value) -> ArgusResult<SocialMessage> {
        let chat_id = payload
            .get("chat_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ArgusError::Social("telegram payload missing chat_id".to_string()))?;
        let user_id = payload
            .get("user_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ArgusError::Social("telegram payload missing user_id".to_string()))?;
        let text = payload
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ArgusError::Social("telegram payload missing text".to_string()))?;

        Ok(SocialMessage::incoming(
            Platform::Telegram,
            chat_id,
            user_id,
            text,
        ))
    }
}
