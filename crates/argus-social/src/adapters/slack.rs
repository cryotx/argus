use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use argus_core::types::{ArgusError, ArgusResult};

use crate::adapter::PlatformAdapter;
use crate::types::{Platform, SocialMessage};

#[derive(Clone, Debug)]
pub struct SlackAdapter {
    pub bot_token: String,
    pub signing_secret: String,
    pub api_base_url: String,
    client: Client,
}

impl SlackAdapter {
    pub fn new(bot_token: impl Into<String>, signing_secret: impl Into<String>) -> Self {
        Self::new_with_base_url(bot_token, signing_secret, "https://slack.com/api")
    }

    pub fn new_with_base_url(
        bot_token: impl Into<String>,
        signing_secret: impl Into<String>,
        api_base_url: impl Into<String>,
    ) -> Self {
        Self {
            bot_token: bot_token.into(),
            signing_secret: signing_secret.into(),
            api_base_url: api_base_url.into(),
            client: Client::new(),
        }
    }

    fn chat_url(&self) -> String {
        format!(
            "{}/chat.postMessage",
            self.api_base_url.trim_end_matches('/')
        )
    }
}

#[derive(Serialize)]
struct SlackMessage<'a> {
    channel: &'a str,
    text: &'a str,
}

#[derive(Deserialize)]
struct SlackResponse {
    ok: bool,
    error: Option<String>,
}

#[async_trait]
impl PlatformAdapter for SlackAdapter {
    fn platform(&self) -> Platform {
        Platform::Slack
    }

    async fn send_message(&self, channel_id: &str, text: &str) -> ArgusResult<()> {
        if self.bot_token.trim().is_empty() {
            return Err(ArgusError::Social("slack token missing".to_string()));
        }

        let payload = SlackMessage {
            channel: channel_id,
            text,
        };

        let response = self
            .client
            .post(self.chat_url())
            .header("Authorization", format!("Bearer {}", self.bot_token))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&payload)
            .send()
            .await
            .map_err(|e| ArgusError::Social(format!("slack request failed: {e}")))?;

        let slack_resp: SlackResponse = response
            .json()
            .await
            .map_err(|e| ArgusError::Social(format!("slack response parse failed: {e}")))?;

        if !slack_resp.ok {
            return Err(ArgusError::Social(format!(
                "slack send failed: {}",
                slack_resp.error.unwrap_or_default()
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
                "{}/auth.test",
                self.api_base_url.trim_end_matches('/')
            ))
            .header("Authorization", format!("Bearer {}", self.bot_token))
            .send()
            .await
            .map_err(|e| ArgusError::Social(format!("slack health check failed: {e}")))?;

        let slack_resp: SlackResponse = response
            .json()
            .await
            .map_err(|e| ArgusError::Social(format!("slack response parse failed: {e}")))?;

        Ok(slack_resp.ok)
    }

    async fn normalize_incoming(&self, payload: serde_json::Value) -> ArgusResult<SocialMessage> {
        let event = payload
            .get("event")
            .ok_or_else(|| ArgusError::Social("slack payload missing event".to_string()))?;

        let channel_id = event
            .get("channel")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        let user_id = event
            .get("user")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        let text = event
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        Ok(SocialMessage::incoming(
            Platform::Slack,
            &channel_id,
            &user_id,
            &text,
        ))
    }
}
