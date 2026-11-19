use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use argus_core::types::{ArgusError, ArgusResult};

use crate::adapter::PlatformAdapter;
use crate::types::{Platform, SocialMessage};

#[derive(Clone, Debug)]
pub struct WechatAdapter {
    pub corp_id: String,
    pub agent_id: String,
    pub secret: String,
    pub access_token: Option<String>,
    pub api_base_url: String,
    client: Client,
}

impl WechatAdapter {
    pub fn new(
        corp_id: impl Into<String>,
        agent_id: impl Into<String>,
        secret: impl Into<String>,
    ) -> Self {
        Self::new_with_base_url(corp_id, agent_id, secret, "https://qyapi.weixin.qq.com")
    }

    pub fn new_with_base_url(
        corp_id: impl Into<String>,
        agent_id: impl Into<String>,
        secret: impl Into<String>,
        api_base_url: impl Into<String>,
    ) -> Self {
        Self {
            corp_id: corp_id.into(),
            agent_id: agent_id.into(),
            secret: secret.into(),
            access_token: None,
            api_base_url: api_base_url.into(),
            client: Client::new(),
        }
    }

    fn token_url(&self) -> String {
        format!(
            "{}/cgi-bin/gettoken?corpid={}&corpsecret={}",
            self.api_base_url.trim_end_matches('/'),
            self.corp_id,
            self.secret
        )
    }

    pub async fn refresh_token(&mut self) -> ArgusResult<()> {
        let response = self
            .client
            .get(self.token_url())
            .send()
            .await
            .map_err(|e| ArgusError::Social(format!("wechat token request failed: {e}")))?;

        #[derive(Deserialize)]
        struct TokenResponse {
            errcode: i32,
            errmsg: String,
            access_token: Option<String>,
            _expires_in: Option<i32>,
        }

        let token_resp: TokenResponse = response
            .json()
            .await
            .map_err(|e| ArgusError::Social(format!("wechat token response parse failed: {e}")))?;

        if token_resp.errcode != 0 {
            return Err(ArgusError::Social(format!(
                "wechat token failed: {} - {}",
                token_resp.errcode, token_resp.errmsg
            )));
        }

        self.access_token = token_resp.access_token;
        Ok(())
    }
}

#[derive(Serialize)]
struct WechatMessage<'a> {
    touser: &'a str,
    msgtype: &'a str,
    agentid: &'a str,
    text: &'a str,
}

#[derive(Serialize)]
struct WechatTextContent<'a> {
    content: &'a str,
}

#[derive(Deserialize)]
struct WechatResponse {
    errcode: i32,
    errmsg: String,
}

#[async_trait]
impl PlatformAdapter for WechatAdapter {
    fn platform(&self) -> Platform {
        Platform::Wechat
    }

    async fn send_message(&self, user_id: &str, text: &str) -> ArgusResult<()> {
        if self.corp_id.trim().is_empty() || self.secret.trim().is_empty() {
            return Err(ArgusError::Social("wechat credentials missing".to_string()));
        }

        // Note: In production, implement token caching
        let token = self
            .access_token
            .as_deref()
            .ok_or_else(|| ArgusError::Social("wechat access token not available".to_string()))?;

        let _content = WechatTextContent { content: text };
        let payload = WechatMessage {
            touser: user_id,
            msgtype: "text",
            agentid: &self.agent_id,
            text,
        };

        let url = format!(
            "{}/cgi-bin/message/send?access_token={}",
            self.api_base_url.trim_end_matches('/'),
            token
        );

        let response = self
            .client
            .post(&url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| ArgusError::Social(format!("wechat request failed: {e}")))?;

        let wechat_resp: WechatResponse = response
            .json()
            .await
            .map_err(|e| ArgusError::Social(format!("wechat response parse failed: {e}")))?;

        if wechat_resp.errcode != 0 {
            return Err(ArgusError::Social(format!(
                "wechat send failed: {} - {}",
                wechat_resp.errcode, wechat_resp.errmsg
            )));
        }

        Ok(())
    }

    async fn health_check(&self) -> ArgusResult<bool> {
        if self.corp_id.trim().is_empty() || self.secret.trim().is_empty() {
            return Ok(false);
        }

        let response = self
            .client
            .get(self.token_url())
            .send()
            .await
            .map_err(|e| ArgusError::Social(format!("wechat health check failed: {e}")))?;

        #[derive(Deserialize)]
        struct TokenResponse {
            errcode: i32,
        }

        let token_resp: TokenResponse = response
            .json()
            .await
            .map_err(|e| ArgusError::Social(format!("wechat response parse failed: {e}")))?;

        Ok(token_resp.errcode == 0)
    }

    async fn normalize_incoming(&self, payload: serde_json::Value) -> ArgusResult<SocialMessage> {
        let msg_type = payload
            .get("msgtype")
            .and_then(|v| v.as_str())
            .unwrap_or("text");

        let user_id = payload
            .get("fromUserName")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        let text = if msg_type == "text" {
            payload
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string()
        } else {
            format!("[{} message]", msg_type)
        };

        // WeChat uses user_id as channel_id for direct messages
        Ok(SocialMessage::incoming(
            Platform::Wechat,
            &user_id,
            &user_id,
            &text,
        ))
    }
}
