//! argus-llm: LLM provider management, routing, failover, budget tracking.
//!
//! This crate provides the complete LLM integration layer for Argus:
//!
//! - **`types`**: Core types — model classes, messages, requests/responses, usage, health
//! - **`provider`**: The `LlmProvider` trait that all backends implement
//! - **`providers`**: Concrete implementations (OpenAI, Anthropic, Ollama)
//! - **`config`**: TOML configuration types for providers
//! - **`key_rotation`**: Round-robin API key management with cooldown
//! - **`budget`**: Multi-level spending tracker (global, provider, project, task)
//! - **`router`**: The central `ProviderRouter` that ties everything together

pub mod budget;
pub mod config;
pub mod key_rotation;
pub mod provider;
pub mod providers;
pub mod router;
pub mod types;

// Re-export primary entry points for convenience
pub use provider::LlmProvider;
pub use router::ProviderRouter;
pub use types::{
    ChatMessage, ChatRole, LlmRequest, LlmResponse, ModelClass, ModelInfo, StreamChunk, TokenUsage,
};
