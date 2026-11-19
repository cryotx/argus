//! V0.43 — Planner Harness / Protocol.
//!
//! A provider-neutral planning protocol:
//!
//! ```text
//! Project State -> Context Compiler -> PlanningRequest -> Planner
//!   -> PlanningResult -> Validate -> Diff -> Proposal
//!   -> explicit Approve -> transactional Apply -> Project State
//! ```
//!
//! A Planner (a web model, a future API provider, or the deterministic test
//! fake) may only propose structured changes; it can never mutate Project
//! State or fabricate verification/artifacts. No provider, auth, quota, or
//! model is named here — that is V0.44.

pub mod adapter;
pub mod diff;
pub mod export;
pub mod protocol;
pub mod repository;
pub mod service;
pub mod types;
pub mod validator;

pub use adapter::{FakePlannerAdapter, PlannerAdapter};
pub use diff::{DiffLabels, build_plan_diff};
pub use export::render_planner_packet;
pub use protocol::{compute_request_hash, compute_result_hash, protocol_version_supported};
pub use repository::{PlanningRepository, PostgresPlanningRepository};
pub use service::{CreatePlanningRequestInput, PlannerHarness, PlanningRunOutcome};
pub use types::*;
pub use validator::{load_entities, validate};

/// Marker type so the validator is discoverable under a protocol name.
pub struct PlanningResultValidator;
