//! LLM Client trait for Brain Engine.
//!
//! This module defines a trait for LLM clients so that argus-core
//! doesn't need to directly depend on argus-llm (avoiding circular dependencies).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Message for LLM chat.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ChatRole {
    System,
    User,
    Assistant,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: ChatRole::System,
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: ChatRole::User,
            content: content.into(),
        }
    }
}

/// Request to LLM.
#[derive(Clone, Debug)]
pub struct LlmRequest {
    pub messages: Vec<ChatMessage>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub purpose: String,
}

impl LlmRequest {
    pub fn new(messages: Vec<ChatMessage>, purpose: impl Into<String>) -> Self {
        Self {
            messages,
            max_tokens: None,
            temperature: None,
            purpose: purpose.into(),
        }
    }

    pub fn with_max_tokens(mut self, tokens: u32) -> Self {
        self.max_tokens = Some(tokens);
        self
    }

    pub fn with_temperature(mut self, temp: f32) -> Self {
        self.temperature = Some(temp);
        self
    }
}

/// Response from LLM.
#[derive(Clone, Debug)]
pub struct LlmResponse {
    pub content: String,
    pub provider: String,
    pub model: String,
}

/// Error from LLM client.
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("Request failed: {0}")]
    RequestFailed(String),
    #[error("Parse error: {0}")]
    ParseError(String),
    #[error("Rate limited")]
    RateLimited,
    #[error("No provider available")]
    NoProvider,
}

/// Trait for LLM clients.
///
/// Implement this trait to provide LLM capabilities to the Brain Engine.
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Send a chat request to the LLM.
    async fn chat(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError>;
}
