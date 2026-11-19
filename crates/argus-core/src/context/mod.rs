//! V0.41 — Context Compiler.
//!
//! ```text
//! Project Memory
//!   (Project State / Roadmap / Decisions / Risks / Artifacts / WorkItems)
//!         │
//!         ▼
//!   Context Compiler          ← deterministic infrastructure, no model
//!         │
//!         ▼
//!   Role-specific Context Packet
//!         │
//!         ▼
//!   Planner / Agent
//! ```
//!
//! The compiler only selects, ranks, filters, compresses structurally,
//! packages, and records provenance. It never plans, reasons, classifies
//! difficulty, chooses a model, calls an LLM, or executes anything.
//!
//! A [`ContextPacket`] is an immutable, hashed snapshot of what a role would
//! see at a point in time. [`compiler::ContextCompiler`] builds it from the
//! structured [`crate::project_state`] repository; [`render`] turns it into
//! deterministic JSON or Markdown.

pub mod compiler;
pub mod render;
pub mod types;

pub use compiler::ContextCompiler;
pub use render::{ContextFormat, render_json, render_markdown};
pub use types::*;
