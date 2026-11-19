//! V0.43 [`PlanningResultValidator`]. Everything a Planner returns is treated
//! as untrusted input: schema/version, project scope, entity existence,
//! cross-project references, illegal state transitions, fabricated
//! verification, dependency validity, duplicate external keys, missing DoD,
//! and stale context are all checked before anything is persisted as a valid
//! proposal.

use std::collections::HashSet;

use uuid::Uuid;

use crate::project_state::{
    ImprovementRecord, MilestoneRecord, PhaseRecord, ProjectStateRepository, WorkItemRecord,
};
use crate::types::ArgusResult;

use super::protocol::protocol_version_supported;
use super::types::*;

#[derive(Default)]
pub struct ProjectEntities {
    pub milestones: Vec<MilestoneRecord>,
    pub phases: Vec<PhaseRecord>,
    pub work_items: Vec<WorkItemRecord>,
    pub improvements: Vec<ImprovementRecord>,
}

pub async fn load_entities(
    state: &dyn ProjectStateRepository,
    project_id: Uuid,
) -> ArgusResult<ProjectEntities> {
    Ok(ProjectEntities {
        milestones: state.list_milestones(project_id).await?,
        phases: state.list_phases(project_id).await?,
        work_items: state.list_work_items(project_id).await?,
        improvements: state.list_improvements(project_id).await?,
    })
}

/// Validate a planning result against the request and current Project State.
/// Returns the list of validation errors (empty = valid).
pub async fn validate(
    state: &dyn ProjectStateRepository,
    request: &PlanningRequest,
    result: &PlanningResult,
) -> ArgusResult<Vec<String>> {
    let entities = load_entities(state, request.project_id).await?;
    let mut errors = Vec::new();

    // --- protocol / binding ---
    if !protocol_version_supported(&request.protocol_version) {
        errors.push(format!(
            "unsupported request protocol version '{}'",
            request.protocol_version
        ));
    }
    if !protocol_version_supported(&result.protocol_version) {
        errors.push(format!(
            "unsupported result protocol version '{}'",
            result.protocol_version
        ));
    }
    if result.planning_request_id != request.planning_request_id {
        errors.push("result.planning_request_id does not match the request".to_string());
    }
    if result.request_hash != request.request_hash {
        errors.push("result.request_hash does not match the request".to_string());
    }
    if result.context_hash != request.context_hash {
        errors.push(format!(
            "context binding mismatch: result context_hash '{}' != request context_hash '{}'",
            result.context_hash, request.context_hash
        ));
    }

    if result.status == PlanningStatus::NeedsClarification && result.clarifications.is_empty() {
        errors.push("status needs_clarification requires at least one clarification".to_string());
    }

    // --- entity id sets scoped to this project ---
    let milestone_ids: HashSet<Uuid> = entities.milestones.iter().map(|m| m.milestone_id).collect();
    let phase_ids: HashSet<Uuid> = entities.phases.iter().map(|p| p.phase_id).collect();
    let work_item_ids: HashSet<Uuid> = entities.work_items.iter().map(|w| w.work_item_id).collect();

    let milestone_keys: HashSet<String> = entities
        .milestones
        .iter()
        .filter_map(|m| m.external_key.clone())
        .collect();
    let work_item_keys: HashSet<String> = entities
        .work_items
        .iter()
        .filter_map(|w| w.external_key.clone())
        .collect();
    let improvement_keys: HashSet<String> = entities
        .improvements
        .iter()
        .filter_map(|i| i.external_key.clone())
        .collect();

    let mut seen_milestone_keys: HashSet<String> = HashSet::new();
    let mut seen_phase_keys: HashSet<String> = HashSet::new();
    let mut seen_work_item_keys: HashSet<String> = HashSet::new();
    let mut seen_improvement_keys: HashSet<String> = HashSet::new();

    // --- milestones ---
    for (i, m) in result.proposal.milestone_changes.iter().enumerate() {
        let where_ = format!("milestone_changes[{i}]");
        check_operation(&where_, m.operation, m.target_id, &mut errors);
        if m.title.trim().is_empty() {
            errors.push(format!("{where_}: title must not be empty"));
        }
        if let Some(id) = m.target_id {
            if !milestone_ids.contains(&id) {
                errors.push(
                    cross_project_or_missing(
                        state,
                        "milestone",
                        id,
                        request.project_id,
                        milestone_ids.contains(&id),
                    )
                    .await,
                );
            }
        }
        if let Some(key) = &m.external_key {
            if milestone_keys.contains(key) || !seen_milestone_keys.insert(key.clone()) {
                errors.push(format!(
                    "{where_}: duplicate milestone external_key '{key}'"
                ));
            }
        }
    }

    // --- phases ---
    for (i, p) in result.proposal.phase_changes.iter().enumerate() {
        let where_ = format!("phase_changes[{i}]");
        check_operation(&where_, p.operation, p.target_id, &mut errors);
        if p.title.trim().is_empty() {
            errors.push(format!("{where_}: title must not be empty"));
        }
        if p.operation == ProposalOperation::Create {
            match p.milestone_id {
                Some(id) if milestone_ids.contains(&id) => {}
                Some(id) => errors.push(
                    cross_project_or_missing(
                        state,
                        "milestone",
                        id,
                        request.project_id,
                        milestone_ids.contains(&id),
                    )
                    .await,
                ),
                None => errors.push(format!("{where_}: create requires milestone_id")),
            }
        } else if let Some(id) = p.milestone_id {
            if !milestone_ids.contains(&id) {
                errors.push(format!(
                    "{where_}: referenced milestone does not belong to this project"
                ));
            }
        }
        if let Some(id) = p.target_id {
            if !phase_ids.contains(&id) {
                errors.push(
                    cross_project_or_missing(
                        state,
                        "phase",
                        id,
                        request.project_id,
                        phase_ids.contains(&id),
                    )
                    .await,
                );
            }
        }
        if let Some(key) = &p.external_key {
            if !seen_phase_keys.insert(key.clone()) {
                errors.push(format!("{where_}: duplicate phase external_key '{key}'"));
            }
        }
    }

    // --- work items ---
    for (i, w) in result.proposal.work_item_changes.iter().enumerate() {
        let where_ = format!("work_item_changes[{i}]");
        check_operation(&where_, w.operation, w.target_id, &mut errors);
        if w.title.trim().is_empty() {
            errors.push(format!("{where_}: title must not be empty"));
        }
        if w.operation == ProposalOperation::Create {
            match w.definition_of_done.as_deref() {
                Some(dod) if !dod.trim().is_empty() => {}
                _ => errors.push(format!(
                    "{where_}: create requires a non-empty definition_of_done"
                )),
            }
            if w.milestone_id.is_none() && w.phase_id.is_none() {
                errors.push(format!(
                    "{where_}: create requires milestone_id or phase_id"
                ));
            }
        }
        if let Some(id) = w.milestone_id {
            if !milestone_ids.contains(&id) {
                errors.push(
                    cross_project_or_missing(
                        state,
                        "milestone",
                        id,
                        request.project_id,
                        milestone_ids.contains(&id),
                    )
                    .await,
                );
            }
        }
        if let Some(id) = w.phase_id {
            if !phase_ids.contains(&id) {
                errors.push(
                    cross_project_or_missing(
                        state,
                        "phase",
                        id,
                        request.project_id,
                        phase_ids.contains(&id),
                    )
                    .await,
                );
            }
        }
        if let Some(id) = w.target_id {
            if !work_item_ids.contains(&id) {
                errors.push(
                    cross_project_or_missing(
                        state,
                        "work_item",
                        id,
                        request.project_id,
                        work_item_ids.contains(&id),
                    )
                    .await,
                );
            }
        }
        // A planner may never fabricate verification.
        if matches!(
            w.status,
            Some(crate::project_state::WorkItemStatus::VerifiedComplete)
                | Some(crate::project_state::WorkItemStatus::Accepted)
        ) {
            errors.push(format!(
                "{where_}: planner may not set VerifiedComplete/Accepted (evidence required)"
            ));
        }
        // Dependencies.
        let mut deps_seen = HashSet::new();
        for dep in &w.dependencies {
            if Some(*dep) == w.target_id {
                errors.push(format!("{where_}: work item cannot depend on itself"));
            }
            if !deps_seen.insert(*dep) {
                errors.push(format!("{where_}: duplicate dependency {dep}"));
            }
            if !work_item_ids.contains(dep) {
                errors.push(
                    cross_project_or_missing(
                        state,
                        "work_item",
                        *dep,
                        request.project_id,
                        work_item_ids.contains(dep),
                    )
                    .await,
                );
            }
        }
        if let Some(key) = &w.external_key {
            if work_item_keys.contains(key) || !seen_work_item_keys.insert(key.clone()) {
                errors.push(format!(
                    "{where_}: duplicate work item external_key '{key}'"
                ));
            }
        }
    }

    // --- decisions / risks / blockers / deferred ---
    for (i, d) in result.proposal.decision_proposals.iter().enumerate() {
        if d.title.trim().is_empty() || d.proposed_decision.trim().is_empty() {
            errors.push(format!(
                "decision_proposals[{i}]: title and proposed_decision must not be empty"
            ));
        }
    }
    for (i, r) in result.proposal.risk_proposals.iter().enumerate() {
        if r.title.trim().is_empty() {
            errors.push(format!("risk_proposals[{i}]: title must not be empty"));
        }
    }
    for (i, b) in result.proposal.blocker_proposals.iter().enumerate() {
        if b.title.trim().is_empty() {
            errors.push(format!("blocker_proposals[{i}]: title must not be empty"));
        }
        if let Some(id) = b.work_item_id {
            if !work_item_ids.contains(&id) {
                errors.push(
                    cross_project_or_missing(
                        state,
                        "work_item",
                        id,
                        request.project_id,
                        work_item_ids.contains(&id),
                    )
                    .await,
                );
            }
        }
    }
    for (i, imp) in result
        .proposal
        .deferred_improvement_proposals
        .iter()
        .enumerate()
    {
        if imp.title.trim().is_empty() {
            errors.push(format!(
                "deferred_improvement_proposals[{i}]: title must not be empty"
            ));
        }
        if let Some(key) = &imp.external_key {
            if improvement_keys.contains(key) || !seen_improvement_keys.insert(key.clone()) {
                errors.push(format!(
                    "deferred_improvement_proposals[{i}]: duplicate external_key '{key}'"
                ));
            }
        }
    }

    Ok(errors)
}

fn check_operation(
    where_: &str,
    operation: ProposalOperation,
    target_id: Option<Uuid>,
    errors: &mut Vec<String>,
) {
    match operation {
        ProposalOperation::Create => {
            if target_id.is_some() {
                errors.push(format!("{where_}: create must not specify target_id"));
            }
        }
        ProposalOperation::NoOp => {
            if target_id.is_none() {
                errors.push(format!("{where_}: no_op requires target_id"));
            }
        }
        _ => {
            if target_id.is_none() {
                errors.push(format!(
                    "{where_}: {} requires target_id",
                    operation.as_str()
                ));
            }
        }
    }
}

/// Distinguish "not in this project" from "does not exist" so a cross-project
/// injection attempt reads clearly.
async fn cross_project_or_missing(
    state: &dyn ProjectStateRepository,
    entity: &str,
    id: Uuid,
    project_id: Uuid,
    _in_project: bool,
) -> String {
    let foreign = match entity {
        "milestone" => state
            .get_milestone(id)
            .await
            .ok()
            .flatten()
            .is_some_and(|m| m.project_id != project_id),
        "phase" => state
            .get_phase(id)
            .await
            .ok()
            .flatten()
            .is_some_and(|p| p.project_id != project_id),
        "work_item" => state
            .get_work_item(id)
            .await
            .ok()
            .flatten()
            .is_some_and(|w| w.project_id != project_id),
        _ => false,
    };
    if foreign {
        format!("cross-project reference rejected: {entity} {id} belongs to another project")
    } else {
        format!("{entity} {id} does not exist in this project")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_operation_rules() {
        let mut errors = Vec::new();
        check_operation(
            "x",
            ProposalOperation::Create,
            Some(Uuid::new_v4()),
            &mut errors,
        );
        assert_eq!(errors.len(), 1);
        check_operation("x", ProposalOperation::Update, None, &mut errors);
        assert_eq!(errors.len(), 2);
        let mut ok = Vec::new();
        check_operation("x", ProposalOperation::Create, None, &mut ok);
        check_operation(
            "x",
            ProposalOperation::Update,
            Some(Uuid::new_v4()),
            &mut ok,
        );
        check_operation("x", ProposalOperation::NoOp, Some(Uuid::new_v4()), &mut ok);
        assert!(ok.is_empty());
    }
}
