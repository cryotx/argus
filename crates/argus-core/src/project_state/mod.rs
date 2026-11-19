//! V0.39 — Project State / Memory foundation.
//!
//! ARGUS's own durable understanding of a project: what it is, what it has
//! completed, what is active, what is blocked, and why — independent of any
//! single conversation, model context window, or Markdown file.
//!
//! ```text
//! Conversation = Evidence
//! Project State = Source of Truth
//! ```
//!
//! The state is **structured first**. A deterministic snapshot (and its
//! human-readable rendering) is *derived* from the tables — never the other
//! way around, and never by asking a model to summarize prose.
//!
//! Layering:
//! - [`types`] — the domain objects (Project, Charter, Milestone, WorkItem,
//!   Decision, Risk, Blocker, DeferredImprovement, ProjectActivity,
//!   ProjectSnapshot) and their finite status enums.
//! - [`repository`] — the single [`ProjectStateRepository`] persistence
//!   boundary (one trait, not nine), with a PostgreSQL implementation.
//! - [`service`] — [`service::ProjectStateService`], the upper layer API
//!   handlers and future planners call; it composes the tables into snapshots
//!   and deterministic status views.
//! - [`progress`] — the V0.40 deterministic Progress Engine: reads the
//!   structured roadmap and computes verified progress without mutating
//!   anything and without a model.
//!
//! This is deliberately **not** vector/RAG memory (that is `argus-memory`).
//! Structured project memory and semantic memory are separate systems.

pub mod progress;
pub mod repository;
pub mod service;
pub mod types;

pub use progress::ProgressEngine;
pub use repository::{PostgresProjectStateRepository, ProjectStateRepository};
pub use service::ProjectStateService;
pub use types::*;
