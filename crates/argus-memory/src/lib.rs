//! argus-memory: Four-layer memory, embedding, semantic search, context builder
//!
//! This crate implements the Argus memory system with four layers:
//! - **Working Memory**: In-RAM scratchpad per task (DashMap)
//! - **Short-Term Memory**: 24-72 hour PostgreSQL storage with auto-expiration
//! - **Long-Term Memory**: pgvector semantic search with importance-weighted decay
//! - **Permanent Memory**: Never-fading critical knowledge
//!
//! Embedding is handled by BGE-M3 via fastembed-rs (1024-dim vectors),
//! which is fixed and never changes when LLM providers switch.

pub mod context;
pub mod embedding;
pub mod lifecycle;
pub mod recall;
pub mod store;
pub mod working;

// Re-export primary types for convenience
pub use context::{ContextBuilder, MemoryContext};
pub use embedding::EmbeddingEngine;
pub use lifecycle::MemoryLifecycle;
pub use recall::{RecallOptions, RetrievedMemory};
pub use store::{MemoryRecord, MemoryStore, NewMemory};
pub use working::{WorkingMemory, WorkingMemoryManager};
