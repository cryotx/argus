//! V0.43 Manual Web Planner packet rendering.
//!
//! Deterministic template output only — no model. It contains the bound
//! context (V0.41 Markdown) and a compact description of the exact
//! `PlanningResult` JSON the web planner must return.

use super::types::{PROTOCOL_VERSION, PlanningRequest};

/// Render the manual planner packet. `context_markdown` is the V0.41
/// deterministic context rendering; `routing_line` is an optional
/// provider-neutral capability line (e.g. "T2 Advanced / reasoning High").
pub fn render_planner_packet(
    request: &PlanningRequest,
    context_markdown: &str,
    routing_line: Option<&str>,
) -> String {
    let mut out = String::new();
    out.push_str("# ARGUS Planner Packet\n\n");
    out.push_str(&format!("Protocol:\n{PROTOCOL_VERSION}\n\n"));
    out.push_str(&format!(
        "Planning Request ID:\n{}\n\n",
        request.planning_request_id
    ));
    out.push_str(&format!("Planning Kind:\n{}\n\n", request.planning_kind));
    out.push_str(&format!(
        "Role / Purpose:\n{} / {}\n\n",
        request.role, request.purpose
    ));
    out.push_str(&format!("Objective:\n{}\n\n", request.objective));
    out.push_str(&format!("Context Hash:\n{}\n\n", request.context_hash));
    out.push_str(&format!("Compiler:\n{}\n\n", request.compiler_version));
    out.push_str(&format!("State Revision:\n{}\n\n", request.state_revision));
    out.push_str(&format!(
        "Roadmap Revision:\n{}\n\n",
        request.roadmap_revision
    ));
    out.push_str(&format!(
        "Recommended Planner Capability:\n{}\n\n",
        routing_line.unwrap_or("unspecified")
    ));

    out.push_str("Constraints:\n");
    if request.constraints.is_empty() {
        out.push_str("None\n");
    } else {
        for c in &request.constraints {
            out.push_str(&format!("- {c}\n"));
        }
    }
    out.push_str("\nRequested Outputs:\n");
    if request.requested_outputs.is_empty() {
        out.push_str("None\n");
    } else {
        for o in &request.requested_outputs {
            out.push_str(&format!("- {o}\n"));
        }
    }

    out.push_str("\n## Current Project Context\n\n");
    out.push_str(context_markdown);
    out.push_str("\n\n## OUTPUT REQUIREMENT\n\n");
    out.push_str(&output_schema_instructions(request));
    out
}

fn output_schema_instructions(request: &PlanningRequest) -> String {
    format!(
        "Return exactly one JSON object (no prose, no code fence) matching \
planner-protocol-v1. Do not add fields; unknown fields are rejected.\n\n\
Required top-level fields:\n\
- planning_result_id: uuid (any new uuid)\n\
- planning_request_id: \"{request_id}\" (must match exactly)\n\
- status: one of proposed | needs_clarification | no_change | blocked | invalid\n\
- summary: string | null\n\
- proposal: object (see below)\n\
- assumptions: string[]\n\
- clarifications: [{{question, why_needed, blocking}}]\n\
- warnings: string[]\n\
- planner_metadata: {{planner_type: \"manual_web\", planner_ref?, model_ref?, session_ref?, duration_ms?, usage?}}\n\
- protocol_version: \"{protocol}\"\n\
- context_hash: \"{context}\" (must match exactly)\n\
- request_hash: \"{request_hash}\" (must match exactly)\n\
- result_hash: \"\" (leave empty; ARGUS computes it)\n\n\
proposal fields (all optional arrays):\n\
- milestone_changes[], phase_changes[], work_item_changes[], decision_proposals[], risk_proposals[], blocker_proposals[], deferred_improvement_proposals[]\n\n\
Each roadmap change has an \"operation\": create | update | transition | supersede | defer | cancel | no_op.\n\
- create: omit target_id.\n\
- update/transition/defer/cancel/no_op: target_id is required and must be an id from THIS project.\n\
work_item_changes additionally: title, definition_of_done (required on create), milestone_id or phase_id (required on create), priority, status, dependencies[], external_key, rationale.\n\n\
Hard rules:\n\
- Never set a work item status to verified_complete or accepted — that requires real evidence and is rejected.\n\
- Reference only ids that already exist in this project; cross-project ids are rejected.\n\
- No shell commands, SQL, file paths, artifacts, executions, or spend records — those are not part of this protocol.\n\
- If you cannot plan without more information, set status = needs_clarification with clarifications[].\n\n\
Skeleton:\n\
{{\"planning_result_id\":\"...\",\"planning_request_id\":\"{request_id}\",\"status\":\"proposed\",\
\"summary\":\"...\",\"proposal\":{{\"phase_changes\":[],\"work_item_changes\":[]}},\"assumptions\":[],\
\"clarifications\":[],\"warnings\":[],\"planner_metadata\":{{\"planner_type\":\"manual_web\"}},\
\"protocol_version\":\"{protocol}\",\"context_hash\":\"{context}\",\"request_hash\":\"{request_hash}\",\"result_hash\":\"\"}}\n",
        request_id = request.planning_request_id,
        protocol = PROTOCOL_VERSION,
        context = request.context_hash,
        request_hash = request.request_hash,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planner::types::{PlanningKind, RequestedOutput};

    fn request() -> PlanningRequest {
        PlanningRequest {
            planning_request_id: uuid::Uuid::nil(),
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
            objective: "Plan V0.43".to_string(),
            constraints: vec!["additive".to_string()],
            requested_outputs: vec![RequestedOutput::WorkItems],
            protocol_version: PROTOCOL_VERSION.to_string(),
            request_hash: "sha256:req".to_string(),
            created_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn packet_contains_schema_and_bindings() {
        let packet = render_planner_packet(&request(), "## Project\nARGUS", Some("T2 Advanced"));
        assert!(packet.contains("planner-protocol-v1"));
        assert!(packet.contains("sha256:ctx"));
        assert!(packet.contains("sha256:req"));
        assert!(packet.contains("OUTPUT REQUIREMENT"));
        assert!(packet.contains("verified_complete"));
        assert!(packet.contains("T2 Advanced"));
        // No provider names.
        assert!(!packet.to_lowercase().contains("claude"));
        assert!(!packet.to_lowercase().contains("gpt"));
    }
}
