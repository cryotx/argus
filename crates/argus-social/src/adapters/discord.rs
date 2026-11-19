use async_trait::async_trait;
use reqwest::Client;
use serde::Serialize;

use argus_core::types::{ArgusError, ArgusResult};

use crate::adapter::PlatformAdapter;
use crate::types::{Platform, SocialMessage};

#[derive(Clone, Debug)]
pub struct DiscordAdapter {
    pub bot_token: String,
    pub api_base_url: String,
    client: Client,
}

impl DiscordAdapter {
    pub fn new(bot_token: impl Into<String>) -> Self {
        Self::new_with_base_url(bot_token, "https://discord.com/api/v10")
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

    fn channel_url(&self, channel_id: &str) -> String {
        let base = self.api_base_url.trim_end_matches('/');
        format!("{base}/channels/{channel_id}/messages")
    }
}

#[derive(Serialize)]
struct DiscordMessage<'a> {
    content: &'a str,
}

#[async_trait]
impl PlatformAdapter for DiscordAdapter {
    fn platform(&self) -> Platform {
        Platform::Discord
    }

    async fn send_message(&self, channel_id: &str, text: &str) -> ArgusResult<()> {
        if self.bot_token.trim().is_empty() {
            return Err(ArgusError::Social("discord token missing".to_string()));
        }

        let payload = DiscordMessage { content: text };

        let response = self
            .client
            .post(self.channel_url(channel_id))
            .header("Authorization", format!("Bot {}", self.bot_token))
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|e| ArgusError::Social(format!("discord request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ArgusError::Social(format!(
                "discord send failed ({status}): {body}"
            )));
        }

        Ok(())
    }

    async fn health_check(&self) -> ArgusResult<bool> {
        if self.bot_token.trim().is_empty() {
            return Ok(false);
        }

        let response = self
            .client
            .get(format!(
                "{}/users/@me",
                self.api_base_url.trim_end_matches('/')
            ))
            .header("Authorization", format!("Bot {}", self.bot_token))
            .send()
            .await
            .map_err(|e| ArgusError::Social(format!("discord health check failed: {e}")))?;

        Ok(response.status().is_success())
    }

    async fn normalize_incoming(&self, payload: serde_json::Value) -> ArgusResult<SocialMessage> {
        let channel_id = payload
            .get("channel_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        let user_id = payload
            .get("author")
            .and_then(|v| v.get("id"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        let text = payload
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        Ok(SocialMessage::incoming(
            Platform::Discord,
            &channel_id,
            &user_id,
            &text,
        ))
    }
}
