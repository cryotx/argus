//! LLM Provider trait — the abstraction all providers implement.
//!
//! Each provider (OpenAI, Anthropic, Ollama, etc.) implements `LlmProvider`,
//! giving the router a uniform interface for completion and streaming.

use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;

use argus_core::types::ArgusResult;

use crate::types::{ChatMessage, LlmResponse, ModelClass, ModelInfo, StreamChunk};

// ============================================================
// Provider trait
// ============================================================

/// The core LLM provider abstraction.
///
/// All LLM providers (API-based, local, web scraper) implement this trait.
/// The router selects a provider and calls its methods.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Unique provider identifier (e.g., "openai", "anthropic", "ollama").
    fn id(&self) -> &str;

    /// Human-readable provider name.
    fn name(&self) -> &str;

    /// Whether this provider is currently enabled.
    fn is_enabled(&self) -> bool;

    /// List all models this provider offers.
    fn models(&self) -> &[ModelInfo];

    /// Check if this provider has a model in the given class.
    fn has_model_class(&self, class: &ModelClass) -> bool {
        self.models().iter().any(|m| m.class == *class)
    }

    /// Get the best model for a given class.
    ///
    /// "Best" = first model in the list that matches the class.
    /// Providers should order their models from best to worst within each class.
    fn best_model_for_class(&self, class: &ModelClass) -> Option<&ModelInfo> {
        self.models().iter().find(|m| m.class == *class)
    }

    /// Get a specific model by ID.
    fn get_model(&self, model_id: &str) -> Option<&ModelInfo> {
        self.models().iter().find(|m| m.id == model_id)
    }

    /// Send a chat completion request.
    ///
    /// # Arguments
    /// - `model_id`: Specific model to use (from this provider's model list)
    /// - `messages`: The conversation messages
    /// - `max_tokens`: Maximum tokens to generate (None = provider default)
    /// - `temperature`: Sampling temperature (None = provider default)
    /// - `api_key`: The API key to use for this request
    async fn complete(
        &self,
        model_id: &str,
        messages: &[ChatMessage],
        max_tokens: Option<u32>,
        temperature: Option<f32>,
        api_key: &str,
    ) -> ArgusResult<LlmResponse>;

    /// Send a streaming chat completion request.
    ///
    /// Returns a stream of `StreamChunk`s. The final chunk has `is_final = true`
    /// and may contain usage information.
    async fn complete_stream(
        &self,
        model_id: &str,
        messages: &[ChatMessage],
        max_tokens: Option<u32>,
        temperature: Option<f32>,
        api_key: &str,
    ) -> ArgusResult<Pin<Box<dyn Stream<Item = ArgusResult<StreamChunk>> + Send>>>;

    /// Check if the provider is reachable (health check).
    ///
    /// Default implementation returns true. Providers can override for
    /// actual health checking (e.g., Ollama checks if the server is running).
    async fn health_check(&self) -> bool {
        true
    }
}
