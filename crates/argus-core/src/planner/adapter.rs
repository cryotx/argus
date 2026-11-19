//! Provider-neutral [`PlannerAdapter`] boundary + a deterministic test fake.
//!
//! No OAuth, HTTP client, or CLI flags belong here — those are V0.44. A planner
//! is anything that turns a [`PlanningRequest`] into a [`PlanningResult`].

use async_trait::async_trait;

use crate::types::ArgusResult;

use super::types::*;

#[async_trait]
pub trait PlannerAdapter: Send + Sync {
    /// A stable, provider-neutral planner type label (e.g. `test`,
    /// `manual_web`).
    fn planner_type(&self) -> &str;

    async fn plan(&self, request: &PlanningRequest) -> ArgusResult<PlanningResult>;
}

/// Deterministic offline planner used by tests and the harness demo. It never
/// touches the network or a model.
pub struct FakePlannerAdapter {
    planner_type: String,
    status: PlanningStatus,
    summary: Option<String>,
    proposal: PlanProposal,
    assumptions: Vec<String>,
    clarifications: Vec<PlanningClarification>,
    warnings: Vec<String>,
}

impl FakePlannerAdapter {
    pub fn new(status: PlanningStatus, proposal: PlanProposal) -> Self {
        Self {
            planner_type: "test".to_string(),
            status,
            summary: Some("fake planner result".to_string()),
            proposal,
            assumptions: Vec::new(),
            clarifications: Vec::new(),
            warnings: Vec::new(),
        }
    }

    pub fn with_summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }

    pub fn with_assumptions(mut self, assumptions: Vec<String>) -> Self {
        self.assumptions = assumptions;
        self
    }

    pub fn with_clarifications(mut self, clarifications: Vec<PlanningClarification>) -> Self {
        self.clarifications = clarifications;
        self
    }

    pub fn with_warnings(mut self, warnings: Vec<String>) -> Self {
        self.warnings = warnings;
        self
    }
}

#[async_trait]
impl PlannerAdapter for FakePlannerAdapter {
    fn planner_type(&self) -> &str {
        &self.planner_type
    }

    async fn plan(&self, request: &PlanningRequest) -> ArgusResult<PlanningResult> {
        Ok(PlanningResult {
            planning_result_id: uuid::Uuid::new_v4(),
            planning_request_id: request.planning_request_id,
            status: self.status,
            summary: self.summary.clone(),
            proposal: self.proposal.clone(),
            assumptions: self.assumptions.clone(),
            clarifications: self.clarifications.clone(),
            warnings: self.warnings.clone(),
            planner_metadata: PlannerMetadata {
                planner_type: self.planner_type.clone(),
                ..Default::default()
            },
            protocol_version: PROTOCOL_VERSION.to_string(),
            context_hash: request.context_hash.clone(),
            request_hash: request.request_hash.clone(),
            result_hash: String::new(),
            created_at: chrono::Utc::now(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fake_planner_binds_request() {
        let request = PlanningRequest {
            planning_request_id: uuid::Uuid::new_v4(),
            project_id: uuid::Uuid::nil(),
            planning_kind: PlanningKind::PhasePlanning,
            role: "project_manager".to_string(),
            purpose: "planning".to_string(),
            context_packet_ref: None,
            context_hash: "sha256:ctx".to_string(),
            compiler_version: "context-compiler-v1".to_string(),
            work_item_id: None,
            task_profile_ref: None,
            routing_decision_ref: None,
            state_revision: 0,
            roadmap_revision: 0,
            objective: "x".to_string(),
            constraints: vec![],
            requested_outputs: vec![],
            protocol_version: PROTOCOL_VERSION.to_string(),
            request_hash: "sha256:req".to_string(),
            created_at: chrono::Utc::now(),
        };
        let adapter = FakePlannerAdapter::new(PlanningStatus::NoChange, PlanProposal::default());
        let result = adapter.plan(&request).await.unwrap();
        assert_eq!(result.planning_request_id, request.planning_request_id);
        assert_eq!(result.context_hash, request.context_hash);
        assert_eq!(result.request_hash, request.request_hash);
        assert_eq!(result.planner_metadata.planner_type, "test");
    }
}
