//! Provider configuration types for TOML deserialization.
//!
//! Matches the structure in `config/providers.toml` and `LLM_PROVIDER.md`.

use serde::{Deserialize, Serialize};

use crate::types::RateLimitConfig;

// ============================================================
// Top-level providers config
// ============================================================

/// Top-level configuration for all LLM providers.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProvidersConfig {
    /// Global settings.
    #[serde(default)]
    pub global: GlobalLlmConfig,

    /// Named providers (key = provider ID).
    #[serde(default)]
    pub providers: std::collections::HashMap<String, ProviderConfig>,

    /// Embedding configuration.
    #[serde(default)]
    pub embedding: EmbeddingConfig,
}

/// Global LLM settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GlobalLlmConfig {
    /// Default model class for requests without explicit class.
    #[serde(default = "GlobalLlmConfig::default_model_class")]
    pub default_model_class: String,

    /// Maximum monthly budget across all providers (USD).
    #[serde(default)]
    pub max_monthly_budget_usd: f64,

    /// Maximum daily budget across all providers (USD).
    #[serde(default)]
    pub max_daily_budget_usd: f64,

    /// Maximum budget per individual task (USD).
    #[serde(default = "GlobalLlmConfig::default_per_task_budget")]
    pub max_per_task_budget_usd: f64,
}

impl GlobalLlmConfig {
    fn default_model_class() -> String {
        "coding".to_string()
    }
    fn default_per_task_budget() -> f64 {
        0.50
    }
}

impl Default for GlobalLlmConfig {
    fn default() -> Self {
        Self {
            default_model_class: Self::default_model_class(),
            max_monthly_budget_usd: 200.0,
            max_daily_budget_usd: 20.0,
            max_per_task_budget_usd: Self::default_per_task_budget(),
        }
    }
}

// ============================================================
// Per-provider configuration
// ============================================================

/// Configuration for a single LLM provider.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Whether this provider is enabled.
    #[serde(default = "ProviderConfig::default_enabled")]
    pub enabled: bool,

    /// Provider type: "openai_compatible", "anthropic", "gemini", "ollama", "web_scraper".
    #[serde(rename = "type")]
    pub provider_type: String,

    /// Priority (lower = higher priority). Used for routing order.
    #[serde(default = "ProviderConfig::default_priority")]
    pub priority: u32,

    /// API keys for this provider (rotated round-robin).
    #[serde(default)]
    pub api_keys: Vec<String>,

    /// Base URL for API calls.
    #[serde(default)]
    pub base_url: Option<String>,

    /// Models offered by this provider.
    #[serde(default)]
    pub models: Vec<ModelConfig>,

    /// Rate limit configuration.
    #[serde(default)]
    pub rate_limit: Option<RateLimitConfig>,

    /// Monthly budget for this specific provider (USD).
    #[serde(default)]
    pub monthly_budget_usd: Option<f64>,

    // --- Web scraper specific ---
    /// URL for web scraper type providers.
    #[serde(default)]
    pub url: Option<String>,

    /// Session tokens for web scraper.
    #[serde(default)]
    pub session_tokens: Vec<String>,

    /// Max concurrent sessions for web scraper.
    #[serde(default)]
    pub max_concurrent: Option<u32>,
}

impl ProviderConfig {
    fn default_enabled() -> bool {
        true
    }
    fn default_priority() -> u32 {
        50
    }
}

/// Configuration for a model within a provider.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelConfig {
    /// Model ID used in API calls.
    pub id: String,

    /// Model class.
    pub class: String,

    /// Cost per 1K input tokens (USD). None for free models.
    #[serde(default)]
    pub cost_per_1k_input: Option<f64>,

    /// Cost per 1K output tokens (USD). None for free models.
    #[serde(default)]
    pub cost_per_1k_output: Option<f64>,

    /// Maximum context window size in tokens.
    #[serde(default)]
    pub max_context_tokens: Option<u32>,
}

// ============================================================
// Embedding configuration
// ============================================================

/// Embedding model configuration (fixed, not affected by LLM routing).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// Embedding type: "local" or "api".
    #[serde(default = "EmbeddingConfig::default_type", rename = "type")]
    pub embedding_type: String,

    /// Model name/ID.
    #[serde(default = "EmbeddingConfig::default_model")]
    pub model: String,

    /// Embedding dimension.
    #[serde(default = "EmbeddingConfig::default_dimension")]
    pub dimension: u32,
}

impl EmbeddingConfig {
    fn default_type() -> String {
        "local".to_string()
    }
    fn default_model() -> String {
        "multilingual-e5-large".to_string()
    }
    fn default_dimension() -> u32 {
        1024
    }
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            embedding_type: Self::default_type(),
            model: Self::default_model(),
            dimension: Self::default_dimension(),
        }
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_global_config() {
        let config = GlobalLlmConfig::default();
        assert_eq!(config.default_model_class, "coding");
        assert!((config.max_monthly_budget_usd - 200.0).abs() < 0.01);
        assert!((config.max_daily_budget_usd - 20.0).abs() < 0.01);
        assert!((config.max_per_task_budget_usd - 0.50).abs() < 0.01);
    }

    #[test]
    fn test_provider_config_toml_parsing() {
        let toml_str = r#"
[global]
default_model_class = "coding"
max_monthly_budget_usd = 100.0

[providers.openai]
enabled = true
type = "openai_compatible"
priority = 2
api_keys = ["sk-xxx1", "sk-xxx2"]
base_url = "https://api.openai.com/v1"
monthly_budget_usd = 50.0

[[providers.openai.models]]
id = "gpt-4o"
class = "coding"
cost_per_1k_input = 0.0025
cost_per_1k_output = 0.01

[[providers.openai.models]]
id = "gpt-4o-mini"
class = "fast"
cost_per_1k_input = 0.00015
cost_per_1k_output = 0.0006

[providers.ollama]
enabled = true
type = "ollama"
priority = 50
base_url = "http://localhost:11434"

[[providers.ollama.models]]
id = "qwen2.5:7b"
class = "fast"

[embedding]
type = "local"
model = "multilingual-e5-large"
dimension = 1024
"#;
        let config: ProvidersConfig = toml::from_str(toml_str).expect("parse providers config");
        assert_eq!(config.global.default_model_class, "coding");
        assert!((config.global.max_monthly_budget_usd - 100.0).abs() < 0.01);

        let openai = config.providers.get("openai").expect("openai provider");
        assert!(openai.enabled);
        assert_eq!(openai.provider_type, "openai_compatible");
        assert_eq!(openai.priority, 2);
        assert_eq!(openai.api_keys.len(), 2);
        assert_eq!(openai.models.len(), 2);
        assert_eq!(openai.models[0].id, "gpt-4o");
        assert_eq!(openai.models[0].class, "coding");
        assert!((openai.monthly_budget_usd.unwrap() - 50.0).abs() < 0.01);

        let ollama = config.providers.get("ollama").expect("ollama provider");
        assert_eq!(ollama.provider_type, "ollama");
        assert_eq!(ollama.priority, 50);
        assert!(ollama.api_keys.is_empty());
        assert_eq!(ollama.models.len(), 1);
        assert!(ollama.models[0].cost_per_1k_input.is_none());

        assert_eq!(config.embedding.embedding_type, "local");
        assert_eq!(config.embedding.dimension, 1024);
    }

    #[test]
    fn test_empty_providers_config() {
        let config = ProvidersConfig::default();
        assert!(config.providers.is_empty());
        assert_eq!(config.embedding.dimension, 1024);
    }
}
