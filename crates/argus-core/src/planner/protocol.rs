//! V0.43 canonical hashing for the planner protocol.
//!
//! `request_hash` binds identity-relevant inputs (context hash, revisions,
//! objective, kind, capability refs) and excludes ids/timestamps.
//! `result_hash` binds the canonical result content and is used for duplicate
//! import detection and audit.

use sha2::{Digest, Sha256};

use super::types::{PROTOCOL_VERSION, PlanningRequest, PlanningResult};

fn sha256_hex(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

pub fn compute_request_hash(request: &PlanningRequest) -> String {
    let mut constraints = request.constraints.clone();
    constraints.sort();
    constraints.dedup();
    let mut outputs = request.requested_outputs.clone();
    outputs.sort();
    outputs.dedup();

    let projection = serde_json::json!({
        "protocol_version": PROTOCOL_VERSION,
        "project_id": request.project_id,
        "planning_kind": request.planning_kind.as_str(),
        "role": request.role,
        "purpose": request.purpose,
        "context_hash": request.context_hash,
        "compiler_version": request.compiler_version,
        "work_item_id": request.work_item_id,
        "task_profile_ref": request.task_profile_ref,
        "routing_decision_ref": request.routing_decision_ref,
        "state_revision": request.state_revision,
        "roadmap_revision": request.roadmap_revision,
        "objective": request.objective,
        "constraints": constraints,
        "requested_outputs": outputs
            .iter()
            .map(|o| o.as_str())
            .collect::<Vec<_>>(),
    });
    let bytes = serde_json::to_vec(&projection).unwrap_or_default();
    sha256_hex(&bytes)
}

pub fn compute_result_hash(result: &PlanningResult) -> String {
    let mut assumptions = result.assumptions.clone();
    assumptions.sort();

    let projection = serde_json::json!({
        "protocol_version": PROTOCOL_VERSION,
        "request_hash": result.request_hash,
        "status": result.status.as_str(),
        "summary": result.summary,
        "proposal": result.proposal,
        "assumptions": assumptions,
        "clarifications": result.clarifications,
        "warnings": result.warnings,
        "planner_metadata": result.planner_metadata,
    });
    let bytes = serde_json::to_vec(&projection).unwrap_or_default();
    sha256_hex(&bytes)
}

/// A major protocol compatibility check: only the exact known version is
/// accepted in v1.
pub fn protocol_version_supported(version: &str) -> bool {
    version == PROTOCOL_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planner::types::*;

    fn request() -> PlanningRequest {
        PlanningRequest {
            planning_request_id: uuid::Uuid::new_v4(),
            project_id: uuid::Uuid::nil(),
            planning_kind: PlanningKind::PhasePlanning,
            role: "project_manager".to_string(),
            purpose: "planning".to_string(),
            context_packet_ref: None,
            context_hash: "sha256:abc".to_string(),
            compiler_version: "context-compiler-v1".to_string(),
            work_item_id: None,
            task_profile_ref: None,
            routing_decision_ref: None,
            state_revision: 0,
            roadmap_revision: 0,
            objective: "Plan V0.43".to_string(),
            constraints: vec!["no llm".to_string(), "additive".to_string()],
            requested_outputs: vec![RequestedOutput::WorkItems, RequestedOutput::RoadmapChanges],
            protocol_version: PROTOCOL_VERSION.to_string(),
            request_hash: String::new(),
            created_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn request_hash_is_stable_and_order_independent() {
        let mut a = request();
        a.request_hash = compute_request_hash(&a);
        let mut b = request();
        b.constraints = vec!["additive".to_string(), "no llm".to_string()];
        b.requested_outputs = vec![RequestedOutput::RoadmapChanges, RequestedOutput::WorkItems];
        b.request_hash = compute_request_hash(&b);
        assert_eq!(a.request_hash, b.request_hash);
        assert_ne!(
            a.request_hash,
            compute_request_hash(&{
                let mut c = a.clone();
                c.objective = "different".to_string();
                c.request_hash = String::new();
                c
            })
        );
    }

    #[test]
    fn result_hash_ignores_ids_and_time() {
        let request = request();
        let mk = |id: uuid::Uuid, at: chrono::DateTime<chrono::Utc>| PlanningResult {
            planning_result_id: id,
            planning_request_id: request.planning_request_id,
            status: PlanningStatus::NoChange,
            summary: Some("nothing".to_string()),
            proposal: PlanProposal::default(),
            assumptions: vec!["b".to_string(), "a".to_string()],
            clarifications: vec![],
            warnings: vec![],
            planner_metadata: PlannerMetadata {
                planner_type: "test".to_string(),
                ..Default::default()
            },
            protocol_version: PROTOCOL_VERSION.to_string(),
            context_hash: "sha256:abc".to_string(),
            request_hash: "sha256:req".to_string(),
            result_hash: String::new(),
            created_at: at,
        };
        let a = mk(uuid::Uuid::new_v4(), chrono::Utc::now());
        let b = mk(uuid::Uuid::new_v4(), chrono::Utc::now());
        assert_eq!(compute_result_hash(&a), compute_result_hash(&b));
    }

    #[test]
    fn protocol_version_check() {
        assert!(protocol_version_supported(PROTOCOL_VERSION));
        assert!(!protocol_version_supported("planner-protocol-v2"));
        assert!(!protocol_version_supported(""));
    }
}
