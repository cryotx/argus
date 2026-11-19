use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use argus_core::types::{ArgusError, ArgusResult};

use crate::adapter::PlatformAdapter;
use crate::types::{Platform, SocialMessage};

#[derive(Clone, Debug)]
pub struct QqAdapter {
    app_id: String,
    app_secret: String,
    api_base_url: String,
    token_url: String,
    client: Client,
    token_cache: Arc<RwLock<Option<CachedToken>>>,
    reply_contexts: Arc<RwLock<HashMap<String, ReplyContext>>>,
}

#[derive(Clone, Debug)]
struct CachedToken {
    value: String,
    expires_at: Instant,
}

#[derive(Clone, Debug)]
struct ReplyContext {
    msg_id: Option<String>,
    event_id: Option<String>,
    next_msg_seq: u32,
    received_at: Instant,
}

#[derive(Debug, Serialize)]
struct AccessTokenRequest<'a> {
    #[serde(rename = "appId")]
    app_id: &'a str,
    #[serde(rename = "clientSecret")]
    client_secret: &'a str,
}

#[derive(Debug, Deserialize)]
struct AccessTokenResponse {
    access_token: String,
    expires_in: u64,
}

#[derive(Debug, Serialize)]
struct SendMessageRequest<'a> {
    content: &'a str,
    msg_type: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    msg_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    event_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    msg_seq: Option<u32>,
}

const PASSIVE_REPLY_WINDOW: Duration = Duration::from_secs(55 * 60);

impl QqAdapter {
    pub fn new(
        app_id: impl Into<String>,
        app_secret: impl Into<String>,
        api_base_url: impl Into<String>,
        token_url: impl Into<String>,
    ) -> Self {
        Self {
            app_id: app_id.into(),
            app_secret: app_secret.into(),
            api_base_url: api_base_url.into(),
            token_url: token_url.into(),
            client: Client::new(),
            token_cache: Arc::new(RwLock::new(None)),
            reply_contexts: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    async fn access_token(&self) -> ArgusResult<String> {
        {
            let cache = self.token_cache.read().await;
            if let Some(cached) = cache.as_ref() {
                if cached.expires_at > Instant::now() + Duration::from_secs(60) {
                    return Ok(cached.value.clone());
                }
            }
        }

        if self.app_id.trim().is_empty() || self.app_secret.trim().is_empty() {
            return Err(ArgusError::Social(
                "qq credentials missing: ARGUS_QQ_APP_ID/ARGUS_QQ_APP_SECRET".to_string(),
            ));
        }

        let request = AccessTokenRequest {
            app_id: &self.app_id,
            client_secret: &self.app_secret,
        };

        let response = self
            .client
            .post(&self.token_url)
            .json(&request)
            .send()
            .await
            .map_err(|e| ArgusError::Social(format!("qq token request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ArgusError::Social(format!(
                "qq token request failed ({status}): {body}"
            )));
        }

        let payload: AccessTokenResponse = response
            .json()
            .await
            .map_err(|e| ArgusError::Social(format!("qq token parse failed: {e}")))?;

        let expires_in = payload.expires_in.max(120);
        *self.token_cache.write().await = Some(CachedToken {
            value: payload.access_token.clone(),
            expires_at: Instant::now() + Duration::from_secs(expires_in),
        });

        Ok(payload.access_token)
    }

    fn c2c_send_message_url(&self, user_openid: &str) -> String {
        let base = self.api_base_url.trim_end_matches('/');
        format!("{base}/v2/users/{user_openid}/messages")
    }

    async fn load_reply_context(
        &self,
        user_openid: &str,
    ) -> (Option<String>, Option<String>, Option<u32>) {
        let mut contexts = self.reply_contexts.write().await;
        let Some(context) = contexts.get_mut(user_openid) else {
            return (None, None, None);
        };

        if Instant::now().duration_since(context.received_at) > PASSIVE_REPLY_WINDOW {
            contexts.remove(user_openid);
            return (None, None, None);
        }

        let msg_seq = if context.msg_id.is_some() {
            let seq = context.next_msg_seq;
            context.next_msg_seq = context.next_msg_seq.saturating_add(1);
            Some(seq)
        } else {
            None
        };

        (context.msg_id.clone(), context.event_id.clone(), msg_seq)
    }

    async fn update_reply_context(
        &self,
        user_openid: &str,
        msg_id: Option<String>,
        event_id: Option<String>,
    ) {
        let mut contexts = self.reply_contexts.write().await;
        contexts.insert(
            user_openid.to_string(),
            ReplyContext {
                msg_id,
                event_id,
                next_msg_seq: 1,
                received_at: Instant::now(),
            },
        );
    }
}

#[async_trait]
impl PlatformAdapter for QqAdapter {
    fn platform(&self) -> Platform {
        Platform::Qq
    }

    async fn send_message(&self, channel_id: &str, text: &str) -> ArgusResult<()> {
        let access_token = self.access_token().await?;
        let (msg_id, event_id, msg_seq) = self.load_reply_context(channel_id).await;
        let request = SendMessageRequest {
            content: text,
            msg_type: 0,
            msg_id: msg_id.as_deref(),
            event_id: event_id.as_deref(),
            msg_seq,
        };

        let response = self
            .client
            .post(self.c2c_send_message_url(channel_id))
            .header("Authorization", format!("QQBot {access_token}"))
            .json(&request)
            .send()
            .await
            .map_err(|e| ArgusError::Social(format!("qq send request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ArgusError::Social(format!(
                "qq send failed ({status}): {body}"
            )));
        }

        Ok(())
    }

    async fn health_check(&self) -> ArgusResult<bool> {
        Ok(!self.app_id.trim().is_empty() && !self.app_secret.trim().is_empty())
    }

    async fn normalize_incoming(&self, payload: serde_json::Value) -> ArgusResult<SocialMessage> {
        let data = payload.get("d").unwrap_or(&payload);
        let event_id = payload
            .get("id")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let msg_id = data.get("id").and_then(|v| v.as_str()).map(str::to_string);

        let user_openid = data
            .get("author")
            .and_then(|v| v.get("user_openid"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ArgusError::Social("qq payload missing author.user_openid".to_string())
            })?;

        let text = data
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ArgusError::Social("qq payload missing content".to_string()))?;

        self.update_reply_context(user_openid, msg_id, event_id)
            .await;

        Ok(SocialMessage::incoming(
            Platform::Qq,
            user_openid,
            user_openid,
            text,
        ))
    }
}
