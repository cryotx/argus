//! Ollama local model provider implementation.
//!
//! Ollama runs local LLM models and provides an API compatible with
//! the OpenAI chat completions format (with some differences).
//! Zero cost, no API key required.

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
// Ollama provider
// ============================================================

/// Local LLM provider via Ollama.
///
/// Ollama provides a REST API at `http://localhost:11434` by default.
/// Uses the `/api/chat` endpoint for completions.
/// No API key needed — it's a local service.
pub struct OllamaProvider {
    id: String,
    name: String,
    base_url: String,
    enabled: bool,
    models: Vec<ModelInfo>,
    client: Client,
}

impl OllamaProvider {
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

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    fn chat_url(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        format!("{base}/api/chat")
    }

    fn tags_url(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        format!("{base}/api/tags")
    }
}

// ============================================================
// Ollama API types
// ============================================================

#[derive(Serialize)]
struct OllamaRequest {
    model: String,
    messages: Vec<OllamaMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<OllamaOptions>,
}

#[derive(Serialize)]
struct OllamaMessage {
    role: String,
    content: String,
}

#[derive(Serialize)]
struct OllamaOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    num_predict: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Deserialize)]
struct OllamaResponse {
    message: Option<OllamaResponseMessage>,
    model: Option<String>,
    done: Option<bool>,
    done_reason: Option<String>,
    prompt_eval_count: Option<u32>,
    eval_count: Option<u32>,
}

#[derive(Deserialize)]
struct OllamaResponseMessage {
    content: Option<String>,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct OllamaTagsResponse {
    models: Option<Vec<OllamaModelTag>>,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct OllamaModelTag {
    name: Option<String>,
}

// ============================================================
// LlmProvider implementation
// ============================================================

#[async_trait]
impl LlmProvider for OllamaProvider {
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
        _api_key: &str, // Not used for Ollama
    ) -> ArgusResult<LlmResponse> {
        let start = std::time::Instant::now();

        let options = if max_tokens.is_some() || temperature.is_some() {
            Some(OllamaOptions {
                num_predict: max_tokens,
                temperature,
            })
        } else {
            None
        };

        let request_body = OllamaRequest {
            model: model_id.to_string(),
            messages: messages
                .iter()
                .map(|m| OllamaMessage {
                    role: m.role.to_string(),
                    content: m.content.clone(),
                })
                .collect(),
            stream: false,
            options,
        };

        let response = self
            .client
            .post(self.chat_url())
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

        let api_response: OllamaResponse = response
            .json()
            .await
            .map_err(|e| ArgusError::Llm(format!("[{}] JSON parse error: {e}", self.id)))?;

        let content = api_response
            .message
            .and_then(|m| m.content)
            .unwrap_or_default();

        let input_tokens = api_response.prompt_eval_count.unwrap_or(0);
        let output_tokens = api_response.eval_count.unwrap_or(0);
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
            finish_reason: api_response.done_reason,
            latency_ms,
            cost_usd: 0.0, // Local model — free
        })
    }

    async fn complete_stream(
        &self,
        model_id: &str,
        messages: &[ChatMessage],
        max_tokens: Option<u32>,
        temperature: Option<f32>,
        _api_key: &str,
    ) -> ArgusResult<Pin<Box<dyn Stream<Item = ArgusResult<StreamChunk>> + Send>>> {
        let options = if max_tokens.is_some() || temperature.is_some() {
            Some(OllamaOptions {
                num_predict: max_tokens,
                temperature,
            })
        } else {
            None
        };

        let request_body = OllamaRequest {
            model: model_id.to_string(),
            messages: messages
                .iter()
                .map(|m| OllamaMessage {
                    role: m.role.to_string(),
                    content: m.content.clone(),
                })
                .collect(),
            stream: true,
            options,
        };

        let response = self
            .client
            .post(self.chat_url())
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

        // Ollama streams newline-delimited JSON (not SSE)
        let stream = byte_stream.map(move |chunk_result| {
            match chunk_result {
                Err(e) => Err(ArgusError::Llm(format!(
                    "[{provider_id}] Stream read error: {e}"
                ))),
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes);
                    // Each line is a JSON object
                    for line in text.lines() {
                        let line = line.trim();
                        if line.is_empty() {
                            continue;
                        }
                        if let Ok(response) = serde_json::from_str::<OllamaResponse>(line) {
                            let delta =
                                response.message.and_then(|m| m.content).unwrap_or_default();
                            let is_final = response.done.unwrap_or(false);
                            let usage = if is_final {
                                Some(TokenUsage {
                                    input_tokens: response.prompt_eval_count.unwrap_or(0),
                                    output_tokens: response.eval_count.unwrap_or(0),
                                    total_tokens: response.prompt_eval_count.unwrap_or(0)
                                        + response.eval_count.unwrap_or(0),
                                })
                            } else {
                                None
                            };

                            return Ok(StreamChunk {
                                delta,
                                is_final,
                                finish_reason: response.done_reason,
                                usage,
                            });
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

    /// Health check: verify Ollama server is running.
    async fn health_check(&self) -> bool {
        match self.client.get(self.tags_url()).send().await {
            Ok(response) => response.status().is_success(),
            Err(_) => false,
        }
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ModelClass;

    fn make_provider() -> OllamaProvider {
        OllamaProvider::new(
            "ollama",
            "Ollama (Local)",
            "http://localhost:11434",
            vec![
                ModelInfo {
                    id: "qwen2.5:72b".to_string(),
                    class: ModelClass::Coding,
                    cost_per_1k_input: None,
                    cost_per_1k_output: None,
                    max_context_tokens: None,
                },
                ModelInfo {
                    id: "qwen2.5:7b".to_string(),
                    class: ModelClass::Fast,
                    cost_per_1k_input: None,
                    cost_per_1k_output: None,
                    max_context_tokens: None,
                },
            ],
        )
    }

    #[test]
    fn test_provider_basics() {
        let provider = make_provider();
        assert_eq!(provider.id(), "ollama");
        assert_eq!(provider.name(), "Ollama (Local)");
        assert!(provider.is_enabled());
        assert_eq!(provider.models().len(), 2);
    }

    #[test]
    fn test_chat_url() {
        let provider = make_provider();
        assert_eq!(provider.chat_url(), "http://localhost:11434/api/chat");
    }

    #[test]
    fn test_free_model() {
        let provider = make_provider();
        let model = provider.get_model("qwen2.5:72b").unwrap();
        assert_eq!(model.estimate_cost(10000, 5000), 0.0);
    }

    #[test]
    fn test_has_model_class() {
        let provider = make_provider();
        assert!(provider.has_model_class(&ModelClass::Coding));
        assert!(provider.has_model_class(&ModelClass::Fast));
        assert!(!provider.has_model_class(&ModelClass::Reasoning));
    }
}
