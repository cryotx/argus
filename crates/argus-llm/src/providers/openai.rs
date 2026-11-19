//! OpenAI-compatible provider implementation.
//!
//! Supports any provider that follows the OpenAI chat completions API:
//! OpenAI, DeepSeek, Qwen, Zhipu, Moonshot, and other compatible services.

use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use argus_core::types::{ArgusError, ArgusResult};

use crate::provider::LlmProvider;
use crate::types::{ChatMessage, LlmResponse, ModelInfo, StreamChunk, TokenUsage};

// ============================================================
// OpenAI-compatible provider
// ============================================================

/// An LLM provider that speaks the OpenAI chat completions API.
///
/// Used for: OpenAI, DeepSeek, Qwen, Zhipu, Moonshot, and any other
/// provider that implements the same protocol.
pub struct OpenAiCompatibleProvider {
    /// Provider identifier.
    id: String,
    /// Human-readable name.
    name: String,
    /// Base URL (e.g., "https://api.openai.com/v1").
    base_url: String,
    /// Whether this provider is enabled.
    enabled: bool,
    /// Models offered.
    models: Vec<ModelInfo>,
    /// HTTP client.
    client: Client,
}

impl OpenAiCompatibleProvider {
    /// Create a new OpenAI-compatible provider.
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
        }
    }

    /// Set enabled state.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Build the chat completions URL.
    fn completions_url(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        format!("{base}/chat/completions")
    }
}

// ============================================================
// OpenAI API request/response types
// ============================================================

#[derive(Serialize)]
struct OpenAiRequest {
    model: String,
    messages: Vec<OpenAiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

#[derive(Serialize)]
struct OpenAiMessage {
    role: String,
    content: String,
}

impl From<&ChatMessage> for OpenAiMessage {
    fn from(msg: &ChatMessage) -> Self {
        Self {
            role: msg.role.to_string(),
            content: msg.content.clone(),
        }
    }
}

#[derive(Deserialize)]
struct OpenAiResponse {
    choices: Vec<OpenAiChoice>,
    usage: Option<OpenAiUsage>,
    model: Option<String>,
}

#[derive(Deserialize)]
struct OpenAiChoice {
    message: Option<OpenAiResponseMessage>,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct OpenAiResponseMessage {
    content: Option<String>,
}

#[derive(Deserialize)]
struct OpenAiUsage {
    prompt_tokens: Option<u32>,
    completion_tokens: Option<u32>,
    total_tokens: Option<u32>,
}

/// SSE stream chunk from OpenAI.
#[derive(Deserialize)]
struct OpenAiStreamChunk {
    choices: Vec<OpenAiStreamChoice>,
    usage: Option<OpenAiUsage>,
}

#[derive(Deserialize)]
struct OpenAiStreamChoice {
    delta: Option<OpenAiStreamDelta>,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct OpenAiStreamDelta {
    content: Option<String>,
}

// ============================================================
// LlmProvider implementation
// ============================================================

#[async_trait]
impl LlmProvider for OpenAiCompatibleProvider {
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

        let request_body = OpenAiRequest {
            model: model_id.to_string(),
            messages: messages.iter().map(OpenAiMessage::from).collect(),
            max_tokens,
            temperature,
            stream: None,
        };

        let response = self
            .client
            .post(self.completions_url())
            .header("Authorization", format!("Bearer {api_key}"))
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

        let api_response: OpenAiResponse = response
            .json()
            .await
            .map_err(|e| ArgusError::Llm(format!("[{}] JSON parse error: {e}", self.id)))?;

        let content = api_response
            .choices
            .first()
            .and_then(|c| c.message.as_ref())
            .and_then(|m| m.content.clone())
            .unwrap_or_default();

        let finish_reason = api_response
            .choices
            .first()
            .and_then(|c| c.finish_reason.clone());

        let usage = api_response.usage.as_ref();
        let input_tokens = usage.and_then(|u| u.prompt_tokens).unwrap_or(0);
        let output_tokens = usage.and_then(|u| u.completion_tokens).unwrap_or(0);
        let total_tokens = usage
            .and_then(|u| u.total_tokens)
            .unwrap_or(input_tokens + output_tokens);

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
                total_tokens,
            },
            finish_reason,
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
        let request_body = OpenAiRequest {
            model: model_id.to_string(),
            messages: messages.iter().map(OpenAiMessage::from).collect(),
            max_tokens,
            temperature,
            stream: Some(true),
        };

        let response = self
            .client
            .post(self.completions_url())
            .header("Authorization", format!("Bearer {api_key}"))
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
                    // SSE format: lines starting with "data: "
                    let mut result_chunks = Vec::new();
                    for line in text.lines() {
                        let line = line.trim();
                        if line == "data: [DONE]" {
                            result_chunks.push(Ok(StreamChunk {
                                delta: String::new(),
                                is_final: true,
                                finish_reason: Some("stop".to_string()),
                                usage: None,
                            }));
                            break;
                        }
                        if let Some(data) = line.strip_prefix("data: ") {
                            if let Ok(chunk) = serde_json::from_str::<OpenAiStreamChunk>(data) {
                                let delta = chunk
                                    .choices
                                    .first()
                                    .and_then(|c| c.delta.as_ref())
                                    .and_then(|d| d.content.clone())
                                    .unwrap_or_default();
                                let finish_reason =
                                    chunk.choices.first().and_then(|c| c.finish_reason.clone());
                                let is_final = finish_reason.is_some();
                                let usage = chunk.usage.map(|u| TokenUsage {
                                    input_tokens: u.prompt_tokens.unwrap_or(0),
                                    output_tokens: u.completion_tokens.unwrap_or(0),
                                    total_tokens: u.total_tokens.unwrap_or(0),
                                });

                                result_chunks.push(Ok(StreamChunk {
                                    delta,
                                    is_final,
                                    finish_reason,
                                    usage,
                                }));
                            }
                        }
                    }
                    // Return the first chunk (simplification: SSE lines in one TCP chunk)
                    if let Some(first) = result_chunks.into_iter().next() {
                        first
                    } else {
                        Ok(StreamChunk {
                            delta: String::new(),
                            is_final: false,
                            finish_reason: None,
                            usage: None,
                        })
                    }
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

    fn make_provider() -> OpenAiCompatibleProvider {
        OpenAiCompatibleProvider::new(
            "openai",
            "OpenAI",
            "https://api.openai.com/v1",
            vec![
                ModelInfo {
                    id: "gpt-4o".to_string(),
                    class: ModelClass::Coding,
                    cost_per_1k_input: Some(0.0025),
                    cost_per_1k_output: Some(0.01),
                    max_context_tokens: Some(128000),
                },
                ModelInfo {
                    id: "gpt-4o-mini".to_string(),
                    class: ModelClass::Fast,
                    cost_per_1k_input: Some(0.00015),
                    cost_per_1k_output: Some(0.0006),
                    max_context_tokens: Some(128000),
                },
            ],
        )
    }

    #[test]
    fn test_provider_basics() {
        let provider = make_provider();
        assert_eq!(provider.id(), "openai");
        assert_eq!(provider.name(), "OpenAI");
        assert!(provider.is_enabled());
        assert_eq!(provider.models().len(), 2);
    }

    #[test]
    fn test_has_model_class() {
        let provider = make_provider();
        assert!(provider.has_model_class(&ModelClass::Coding));
        assert!(provider.has_model_class(&ModelClass::Fast));
        assert!(!provider.has_model_class(&ModelClass::Reasoning));
    }

    #[test]
    fn test_best_model_for_class() {
        let provider = make_provider();
        let model = provider.best_model_for_class(&ModelClass::Coding).unwrap();
        assert_eq!(model.id, "gpt-4o");

        let model = provider.best_model_for_class(&ModelClass::Fast).unwrap();
        assert_eq!(model.id, "gpt-4o-mini");

        assert!(
            provider
                .best_model_for_class(&ModelClass::Reasoning)
                .is_none()
        );
    }

    #[test]
    fn test_get_model() {
        let provider = make_provider();
        assert!(provider.get_model("gpt-4o").is_some());
        assert!(provider.get_model("nonexistent").is_none());
    }

    #[test]
    fn test_completions_url() {
        let provider = make_provider();
        assert_eq!(
            provider.completions_url(),
            "https://api.openai.com/v1/chat/completions"
        );

        // Trailing slash should be stripped
        let provider2 =
            OpenAiCompatibleProvider::new("test", "Test", "https://api.example.com/v1/", vec![]);
        assert_eq!(
            provider2.completions_url(),
            "https://api.example.com/v1/chat/completions"
        );
    }

    #[test]
    fn test_set_enabled() {
        let mut provider = make_provider();
        assert!(provider.is_enabled());
        provider.set_enabled(false);
        assert!(!provider.is_enabled());
    }
}
