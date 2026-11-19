//! LLM provider implementations.
//!
//! Each module implements the `LlmProvider` trait for a specific API protocol:
//! - `openai`: OpenAI-compatible chat completions API (OpenAI, DeepSeek, Qwen, Zhipu, Moonshot)
//! - `anthropic`: Anthropic Messages API (Claude models)
//! - `ollama`: Local Ollama server (free, no API key)

pub mod anthropic;
pub mod ollama;
pub mod openai;
