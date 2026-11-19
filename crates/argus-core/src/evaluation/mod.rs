//! V0.45 verification foundation. Evaluators read durable evidence; only the
//! harness can promote verified work, and acceptance is a separate action.

mod evaluators;
mod service;
mod types;

pub use evaluators::{ArtifactEvaluator, Evaluator, GitCheckpointEvaluator};
pub use service::EvaluationHarness;
pub use types::*;
