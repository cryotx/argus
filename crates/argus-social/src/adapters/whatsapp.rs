use async_trait::async_trait;
use reqwest::Client;
use serde::Serialize;

use argus_core::types::{ArgusError, ArgusResult};

use crate::adapter::PlatformAdapter;
use crate::types::{Platform, SocialMessage};

#[derive(Clone, Debug)]
pub struct WhatsAppAdapter {
    pub access_token: String,
    pub phone_number_id: String,
    pub graph_api_base_url: String,
    client: Client,
}

impl WhatsAppAdapter {
    pub fn new(access_token: impl Into<String>, phone_number_id: impl Into<String>) -> Self {
        Self::new_with_base_url(
            access_token,
            phone_number_id,
            "https://graph.facebook.com/v20.0",
        )
    }

    pub fn new_with_base_url(
        access_token: impl Into<String>,
        phone_number_id: impl Into<String>,
        graph_api_base_url: impl Into<String>,
    ) -> Self {
        Self {
            access_token: access_token.into(),
            phone_number_id: phone_number_id.into(),
            graph_api_base_url: graph_api_base_url.into(),
            client: Client::new(),
        }
    }

    fn send_message_url(&self) -> String {
        let base = self.graph_api_base_url.trim_end_matches('/');
        format!("{base}/{}/messages", self.phone_number_id)
    }
}

#[derive(Serialize)]
struct WhatsAppTextBody<'a> {
    body: &'a str,
}

#[derive(Serialize)]
struct WhatsAppSendRequest<'a> {
    messaging_product: &'static str,
    to: &'a str,
    r#type: &'static str,
    text: WhatsAppTextBody<'a>,
}

#[async_trait]
impl PlatformAdapter for WhatsAppAdapter {
    fn platform(&self) -> Platform {
        Platform::Whatsapp
    }

    async fn send_message(&self, channel_id: &str, text: &str) -> ArgusResult<()> {
        if self.access_token.trim().is_empty() {
            return Err(ArgusError::Social(
                "whatsapp access token missing".to_string(),
            ));
        }
        if self.phone_number_id.trim().is_empty() {
            return Err(ArgusError::Social(
                "whatsapp phone_number_id missing".to_string(),
            ));
        }

        let payload = WhatsAppSendRequest {
            messaging_product: "whatsapp",
            to: channel_id,
            r#type: "text",
            text: WhatsAppTextBody { body: text },
        };

        let response = self
            .client
            .post(self.send_message_url())
            .bearer_auth(&self.access_token)
            .json(&payload)
            .send()
            .await
            .map_err(|e| ArgusError::Social(format!("whatsapp request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ArgusError::Social(format!(
                "whatsapp send failed ({status}): {body}"
            )));
        }

        Ok(())
    }

    async fn health_check(&self) -> ArgusResult<bool> {
        Ok(!self.access_token.trim().is_empty() && !self.phone_number_id.trim().is_empty())
    }

    async fn normalize_incoming(&self, payload: serde_json::Value) -> ArgusResult<SocialMessage> {
        let from = payload
            .get("from")
            .and_then(|v| v.as_str())
            .or_else(|| payload.get("phone").and_then(|v| v.as_str()))
            .ok_or_else(|| ArgusError::Social("whatsapp payload missing from".to_string()))?;

        let text = payload
            .get("text")
            .and_then(|v| v.as_str())
            .or_else(|| {
                payload
                    .get("text")
                    .and_then(|v| v.get("body"))
                    .and_then(|v| v.as_str())
            })
            .ok_or_else(|| ArgusError::Social("whatsapp payload missing text".to_string()))?;

        Ok(SocialMessage::incoming(
            Platform::Whatsapp,
            from,
            from,
            text,
        ))
    }
}
