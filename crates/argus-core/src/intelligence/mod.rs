//! V0.42 — Task Intelligence / Model Routing.
//!
//! ```text
//! WorkItem / Task
//!       ↓
//! Task Intelligence (deterministic rules)
//!       ↓
//! TaskProfile (signals, capabilities, reasons)
//!       ↓
//! Model Routing Policy
//!       ↓
//! RoutingDecision (minimum capability tier)
//!       ↓
//! Execution / Planner          (V0.43+)
//!       ↓
//! Outcome → Failure Classification → Retry / Escalate / Replan / Human
//! ```
//!
//! A tier is a **capability requirement**, never a provider or model. V0.44
//! maps tiers onto concrete providers/subscriptions/quotas; V0.43 runs the
//! planner. There is no LLM here: classification is deterministic rules over
//! structured Project State and history, with an explainable reason for every
//! non-trivial decision.

pub mod classifier;
pub mod escalation;
pub mod repository;
pub mod router;
pub mod service;
pub mod types;

pub use classifier::{TaskClassifier, classify_tier, risk_floor};
pub use escalation::{ESCALATION_POLICY_VERSION, EscalationChoice, EscalationPolicy};
pub use repository::{PostgresTaskIntelligenceRepository, TaskIntelligenceRepository};
pub use router::RoutingPolicy;
pub use service::{FailureInput, OutcomeInput, TaskIntelligenceService};
pub use types::*;
