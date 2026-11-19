//! Anthropic Claude provider implementation.
//!
//! Implements the Anthropic Messages API, which has a different protocol
//! from OpenAI (different auth header, message format, response shape).

use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use argus_core::types::{ArgusError, ArgusResult};

use crate::provider::LlmProvider;
use crate::types::{ChatMessage, ChatRole, LlmResponse, ModelInfo, StreamChunk, TokenUsage};

// ============================================================
// Anthropic provider
// ============================================================

/// Anthropic Claude API provider.
///
/// Uses the Anthropic Messages API (not OpenAI-compatible).
/// Key differences:
/// - Auth via `x-api-key` header (not `Authorization: Bearer`)
/// - System message is a separate top-level field
/// - Different response structure
pub struct AnthropicProvider {
    /// Provider identifier.
    id: String,
    /// Human-readable name.
    name: String,
    /// Base URL (default: "https://api.anthropic.com").
    base_url: String,
    /// Whether this provider is enabled.
    enabled: bool,
    /// Models offered.
    models: Vec<ModelInfo>,
    /// HTTP client.
    client: Client,
    /// API version header value.
    api_version: String,
}

impl AnthropicProvider {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        base_url: impl Into<String>,
        models: Vec<ModelInfo>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            base_url: base_url.into(),
            enabled: true,
            models,
            client: Client::new(),
            api_version: "2023-06-01".to_string(),
        }
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    fn messages_url(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        format!("{base}/v1/messages")
    }

    /// Split messages into system prompt and conversation messages.
    /// Anthropic requires the system prompt to be a separate field.
    fn split_system_messages(messages: &[ChatMessage]) -> (Option<String>, Vec<&ChatMessage>) {
        let mut system_parts: Vec<&str> = Vec::new();
        let mut conversation: Vec<&ChatMessage> = Vec::new();

        for msg in messages {
            match msg.role {
                ChatRole::System => system_parts.push(&msg.content),
                _ => conversation.push(msg),
            }
        }

        let system = if system_parts.is_empty() {
            None
        } else {
            Some(system_parts.join("\n\n"))
        };

        (system, conversation)
    }
}

// ============================================================
// Anthropic API types
// ============================================================

#[derive(Serialize)]
struct AnthropicRequest {
    model: String,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

#[derive(Serialize)]
struct AnthropicMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContentBlock>,
    model: Option<String>,
    stop_reason: Option<String>,
    usage: Option<AnthropicUsage>,
}

#[derive(Deserialize)]
struct AnthropicContentBlock {
    #[serde(rename = "type")]
    content_type: String,
    text: Option<String>,
}

#[derive(Deserialize)]
struct AnthropicUsage {
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
}

/// SSE event from Anthropic streaming.
#[derive(Deserialize)]
struct AnthropicStreamEvent {
    #[serde(rename = "type")]
    event_type: String,
    delta: Option<AnthropicStreamDelta>,
    usage: Option<AnthropicUsage>,
}

#[derive(Deserialize)]
struct AnthropicStreamDelta {
    #[serde(rename = "type")]
    _delta_type: Option<String>,
    text: Option<String>,
    stop_reason: Option<String>,
}

// ============================================================
// LlmProvider implementation
// ============================================================

#[async_trait]
impl LlmProvider for AnthropicProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn is_enabled(&self) -> bool {
        self.enabled
    }

    fn models(&self) -> &[ModelInfo] {
        &self.models
    }

    async fn complete(
        &self,
        model_id: &str,
        messages: &[ChatMessage],
        max_tokens: Option<u32>,
        temperature: Option<f32>,
        api_key: &str,
    ) -> ArgusResult<LlmResponse> {
        let start = std::time::Instant::now();
        let (system, conversation) = Self::split_system_messages(messages);

        let anthropic_messages: Vec<AnthropicMessage> = conversation
            .iter()
            .map(|m| AnthropicMessage {
                role: match m.role {
                    ChatRole::User => "user".to_string(),
                    ChatRole::Assistant => "assistant".to_string(),
                    _ => "user".to_string(),
                },
                content: m.content.clone(),
            })
            .collect();

        let request_body = AnthropicRequest {
            model: model_id.to_string(),
            messages: anthropic_messages,
            system,
            max_tokens: max_tokens.unwrap_or(4096),
            temperature,
            stream: None,
        };

        let response = self
            .client
            .post(self.messages_url())
            .header("x-api-key", api_key)
            .header("anthropic-version", &self.api_version)
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
            .map_err(|e| ArgusError::Llm(format!("[{}] HTTP error: {e}", self.id)))?;

        let status = response.status();
        if !status.is_success() {
            let error_body = response.text().await.unwrap_or_default();
            return Err(ArgusError::Llm(format!(
                "[{}] API error {status}: {error_body}",
                self.id
            )));
        }

        let api_response: AnthropicResponse = response
            .json()
            .await
            .map_err(|e| ArgusError::Llm(format!("[{}] JSON parse error: {e}", self.id)))?;

        let content = api_response
            .content
            .iter()
            .filter(|b| b.content_type == "text")
            .filter_map(|b| b.text.as_ref())
            .cloned()
            .collect::<Vec<String>>()
            .join("");

        let usage = api_response.usage.as_ref();
        let input_tokens = usage.and_then(|u| u.input_tokens).unwrap_or(0);
        let output_tokens = usage.and_then(|u| u.output_tokens).unwrap_or(0);

        let model_info = self.get_model(model_id);
        let cost_usd = model_info
            .map(|m| m.estimate_cost(input_tokens, output_tokens))
            .unwrap_or(0.0);

        let latency_ms = start.elapsed().as_millis() as u64;

        Ok(LlmResponse {
            content,
            provider: self.id.clone(),
            model: api_response.model.unwrap_or_else(|| model_id.to_string()),
            usage: TokenUsage {
                input_tokens,
                output_tokens,
                total_tokens: input_tokens + output_tokens,
            },
            finish_reason: api_response.stop_reason,
            latency_ms,
            cost_usd,
        })
    }

    async fn complete_stream(
        &self,
        model_id: &str,
        messages: &[ChatMessage],
        max_tokens: Option<u32>,
        temperature: Option<f32>,
        api_key: &str,
    ) -> ArgusResult<Pin<Box<dyn Stream<Item = ArgusResult<StreamChunk>> + Send>>> {
        let (system, conversation) = Self::split_system_messages(messages);

        let anthropic_messages: Vec<AnthropicMessage> = conversation
            .iter()
            .map(|m| AnthropicMessage {
                role: match m.role {
                    ChatRole::User => "user".to_string(),
                    ChatRole::Assistant => "assistant".to_string(),
                    _ => "user".to_string(),
                },
                content: m.content.clone(),
            })
            .collect();

        let request_body = AnthropicRequest {
            model: model_id.to_string(),
            messages: anthropic_messages,
            system,
            max_tokens: max_tokens.unwrap_or(4096),
            temperature,
            stream: Some(true),
        };

        let response = self
            .client
            .post(self.messages_url())
            .header("x-api-key", api_key)
            .header("anthropic-version", &self.api_version)
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
            .map_err(|e| ArgusError::Llm(format!("[{}] HTTP error: {e}", self.id)))?;

        let status = response.status();
        if !status.is_success() {
            let error_body = response.text().await.unwrap_or_default();
            return Err(ArgusError::Llm(format!(
                "[{}] Stream API error {status}: {error_body}",
                self.id
            )));
        }

        let byte_stream = response.bytes_stream();
        let provider_id = self.id.clone();

        let stream = byte_stream.map(move |chunk_result| {
            match chunk_result {
                Err(e) => Err(ArgusError::Llm(format!(
                    "[{provider_id}] Stream read error: {e}"
                ))),
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes);
                    // Parse Anthropic SSE events
                    for line in text.lines() {
                        let line = line.trim();
                        if let Some(data) = line.strip_prefix("data: ") {
                            if let Ok(event) = serde_json::from_str::<AnthropicStreamEvent>(data) {
                                match event.event_type.as_str() {
                                    "content_block_delta" => {
                                        let text = event
                                            .delta
                                            .as_ref()
                                            .and_then(|d| d.text.clone())
                                            .unwrap_or_default();
                                        return Ok(StreamChunk {
                                            delta: text,
                                            is_final: false,
                                            finish_reason: None,
                                            usage: None,
                                        });
                                    }
                                    "message_delta" => {
                                        let stop_reason = event
                                            .delta
                                            .as_ref()
                                            .and_then(|d| d.stop_reason.clone());
                                        let usage = event.usage.map(|u| TokenUsage {
                                            input_tokens: u.input_tokens.unwrap_or(0),
                                            output_tokens: u.output_tokens.unwrap_or(0),
                                            total_tokens: u.input_tokens.unwrap_or(0)
                                                + u.output_tokens.unwrap_or(0),
                                        });
                                        return Ok(StreamChunk {
                                            delta: String::new(),
                                            is_final: true,
                                            finish_reason: stop_reason,
                                            usage,
                                        });
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    Ok(StreamChunk {
                        delta: String::new(),
                        is_final: false,
                        finish_reason: None,
                        usage: None,
                    })
                }
            }
        });

        Ok(Box::pin(stream))
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ModelClass;

    fn make_provider() -> AnthropicProvider {
        AnthropicProvider::new(
            "anthropic",
            "Anthropic",
            "https://api.anthropic.com",
            vec![
                ModelInfo {
                    id: "claude-sonnet-4-20250514".to_string(),
                    class: ModelClass::Coding,
                    cost_per_1k_input: Some(0.003),
                    cost_per_1k_output: Some(0.015),
                    max_context_tokens: Some(200000),
                },
                ModelInfo {
                    id: "claude-haiku-3-5".to_string(),
                    class: ModelClass::Fast,
                    cost_per_1k_input: Some(0.0008),
                    cost_per_1k_output: Some(0.004),
                    max_context_tokens: Some(200000),
                },
            ],
        )
    }

    #[test]
    fn test_provider_basics() {
        let provider = make_provider();
        assert_eq!(provider.id(), "anthropic");
        assert_eq!(provider.name(), "Anthropic");
        assert!(provider.is_enabled());
        assert_eq!(provider.models().len(), 2);
    }

    #[test]
    fn test_messages_url() {
        let provider = make_provider();
        assert_eq!(
            provider.messages_url(),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn test_split_system_messages() {
        let messages = vec![
            ChatMessage::system("You are a coding assistant"),
            ChatMessage::system("Be concise"),
            ChatMessage::user("Hello"),
            ChatMessage::assistant("Hi there"),
            ChatMessage::user("Help me"),
        ];

        let (system, conversation) = AnthropicProvider::split_system_messages(&messages);
        assert_eq!(system.unwrap(), "You are a coding assistant\n\nBe concise");
        assert_eq!(conversation.len(), 3);
        assert_eq!(conversation[0].role, ChatRole::User);
        assert_eq!(conversation[0].content, "Hello");
    }

    #[test]
    fn test_split_no_system_messages() {
        let messages = vec![ChatMessage::user("Hello"), ChatMessage::assistant("Hi")];

        let (system, conversation) = AnthropicProvider::split_system_messages(&messages);
        assert!(system.is_none());
        assert_eq!(conversation.len(), 2);
    }
}
