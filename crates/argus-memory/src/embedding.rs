//! Embedding engine via fastembed-rs.
//!
//! This module wraps fastembed's TextEmbedding with a 1024-dim multilingual model
//! and is the ONLY embedding model used by Argus. It never changes when LLM
//! providers switch, ensuring memory vector consistency across model migrations.
//!
//! Current model: multilingual-e5-large (1024-dim, multilingual including Chinese).
//! Target model: BGE-M3 (1024-dim) — will switch when fastembed supports it on
//! our Rust toolchain version.
//!
//! Because `TextEmbedding::embed()` is synchronous CPU-bound ONNX inference
//! and takes `&mut self`, the model is wrapped in `Arc<Mutex<>>` and all calls
//! are dispatched to `tokio::task::spawn_blocking()`.

use std::sync::{Arc, Mutex};

use argus_core::types::{ArgusError, ArgusResult};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

/// Embedding output dimensionality (1024 for multilingual-e5-large / BGE-M3).
pub const EMBEDDING_DIM: usize = 1024;

/// Wrapper around fastembed's `TextEmbedding`.
///
/// Thread-safe via `Arc<Mutex<>>` internally — clone freely.
#[derive(Clone)]
pub struct EmbeddingEngine {
    model: Arc<Mutex<TextEmbedding>>,
}

impl EmbeddingEngine {
    /// Initialize the embedding model.
    ///
    /// This downloads the model on first run and caches it locally.
    /// Initialization is slow (~2-5 seconds) but only done once at startup.
    pub fn new() -> ArgusResult<Self> {
        let model = TextEmbedding::try_new(InitOptions::new(EmbeddingModel::MultilingualE5Large))
            .map_err(|e| {
            ArgusError::Memory(format!(
                "Failed to initialize multilingual-e5-large embedding model: {e}"
            ))
        })?;

        tracing::info!(
            "Embedding engine initialized: multilingual-e5-large (dim={})",
            EMBEDDING_DIM
        );

        Ok(Self {
            model: Arc::new(Mutex::new(model)),
        })
    }

    /// Embed a single text string into a 1024-dim vector.
    ///
    /// Runs synchronous ONNX inference on a blocking thread to avoid
    /// starving the tokio runtime.
    pub async fn embed(&self, text: &str) -> ArgusResult<Vec<f32>> {
        let model = Arc::clone(&self.model);
        let text = text.to_string();

        let result = tokio::task::spawn_blocking(move || {
            let mut guard = model
                .lock()
                .map_err(|e| ArgusError::Memory(format!("Embedding lock poisoned: {e}")))?;
            guard
                .embed(vec![text.as_str()], None)
                .map_err(|e| ArgusError::Memory(format!("Embedding failed: {e}")))
        })
        .await
        .map_err(|e| ArgusError::Memory(format!("Embedding task panicked: {e}")))?;

        let mut embeddings = result?;
        embeddings
            .pop()
            .ok_or_else(|| ArgusError::Memory("Embedding returned empty result".to_string()))
    }

    /// Embed a batch of texts into 1024-dim vectors.
    ///
    /// More efficient than calling `embed()` in a loop because fastembed
    /// batches the ONNX inference internally.
    pub async fn embed_batch(&self, texts: &[String]) -> ArgusResult<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }

        let model = Arc::clone(&self.model);
        let texts: Vec<String> = texts.to_vec();

        tokio::task::spawn_blocking(move || {
            let refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
            let mut guard = model
                .lock()
                .map_err(|e| ArgusError::Memory(format!("Embedding lock poisoned: {e}")))?;
            guard
                .embed(refs, None)
                .map_err(|e| ArgusError::Memory(format!("Batch embedding failed: {e}")))
        })
        .await
        .map_err(|e| ArgusError::Memory(format!("Batch embedding task panicked: {e}")))?
    }

    /// Get the embedding dimensionality (always 1024).
    pub fn dimension(&self) -> usize {
        EMBEDDING_DIM
    }
}
