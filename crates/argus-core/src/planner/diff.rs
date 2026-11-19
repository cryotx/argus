//! Deterministic [`PlanDiff`] computation. Same state + same proposal always
//! yields the identical, stably ordered diff.

use std::collections::HashMap;

use uuid::Uuid;

use super::types::*;

/// Titles of existing entities, used to label update/transition entries.
#[derive(Default)]
pub struct DiffLabels {
    pub milestones: HashMap<Uuid, String>,
    pub phases: HashMap<Uuid, String>,
    pub work_items: HashMap<Uuid, String>,
}

impl DiffLabels {
    fn label(&self, entity: EntityType, id: Uuid) -> Option<String> {
        match entity {
            EntityType::Milestone => self.milestones.get(&id),
            EntityType::Phase => self.phases.get(&id),
            EntityType::WorkItem => self.work_items.get(&id),
            _ => None,
        }
        .cloned()
    }
}

pub fn build_plan_diff(proposal: &PlanProposal, labels: &DiffLabels) -> PlanDiff {
    let mut entries = Vec::new();

    for m in &proposal.milestone_changes {
        entries.push(entry(
            EntityType::Milestone,
            m.operation,
            m.target_id,
            &m.title,
            m.rationale.clone(),
            labels,
        ));
    }
    for p in &proposal.phase_changes {
        entries.push(entry(
            EntityType::Phase,
            p.operation,
            p.target_id,
            &p.title,
            p.rationale.clone(),
            labels,
        ));
    }
    for w in &proposal.work_item_changes {
        entries.push(entry(
            EntityType::WorkItem,
            w.operation,
            w.target_id,
            &w.title,
            w.rationale.clone(),
            labels,
        ));
    }
    for d in &proposal.decision_proposals {
        entries.push(DiffEntry {
            entity_type: EntityType::Decision,
            operation: ProposalOperation::Create,
            label: d.title.clone(),
            rationale: d.rationale.clone(),
        });
    }
    for r in &proposal.risk_proposals {
        entries.push(DiffEntry {
            entity_type: EntityType::Risk,
            operation: ProposalOperation::Create,
            label: r.title.clone(),
            rationale: r.rationale.clone(),
        });
    }
    for b in &proposal.blocker_proposals {
        entries.push(DiffEntry {
            entity_type: EntityType::Blocker,
            operation: ProposalOperation::Create,
            label: b.title.clone(),
            rationale: b.rationale.clone(),
        });
    }
    for i in &proposal.deferred_improvement_proposals {
        entries.push(DiffEntry {
            entity_type: EntityType::DeferredImprovement,
            operation: ProposalOperation::Create,
            label: i.title.clone(),
            rationale: i.rationale.clone(),
        });
    }

    // Stable ordering: entity type, operation, label, rationale.
    entries.sort_by(|a, b| {
        a.entity_type
            .cmp(&b.entity_type)
            .then_with(|| a.operation.cmp(&b.operation))
            .then_with(|| a.label.cmp(&b.label))
            .then_with(|| a.rationale.cmp(&b.rationale))
    });

    let count = |entity: EntityType, op: ProposalOperation| {
        entries
            .iter()
            .filter(|e| e.entity_type == entity && e.operation == op)
            .count()
    };

    PlanDiff {
        milestone_creates: count(EntityType::Milestone, ProposalOperation::Create),
        phase_creates: count(EntityType::Phase, ProposalOperation::Create),
        work_item_creates: count(EntityType::WorkItem, ProposalOperation::Create),
        decision_proposals: proposal.decision_proposals.len(),
        risk_proposals: proposal.risk_proposals.len(),
        blocker_proposals: proposal.blocker_proposals.len(),
        deferred_improvement_proposals: proposal.deferred_improvement_proposals.len(),
        entries,
    }
}

fn entry(
    entity_type: EntityType,
    operation: ProposalOperation,
    target_id: Option<Uuid>,
    proposed_title: &str,
    rationale: Option<String>,
    labels: &DiffLabels,
) -> DiffEntry {
    let label = match operation {
        ProposalOperation::Create => proposed_title.to_string(),
        _ => target_id
            .and_then(|id| labels.label(entity_type, id))
            .unwrap_or_else(|| {
                target_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| proposed_title.to_string())
            }),
    };
    DiffEntry {
        entity_type,
        operation,
        label,
        rationale,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work_item(title: &str, op: ProposalOperation, target: Option<Uuid>) -> ProposedWorkItem {
        ProposedWorkItem {
            operation: op,
            target_id: target,
            title: title.to_string(),
            description: None,
            definition_of_done: Some("done".to_string()),
            milestone_id: None,
            phase_id: None,
            priority: None,
            status: None,
            dependencies: vec![],
            external_key: None,
            weight: None,
            rationale: None,
        }
    }

    #[test]
    fn diff_is_deterministic_and_stable_ordered() {
        let proposal = PlanProposal {
            phase_changes: vec![ProposedPhase {
                operation: ProposalOperation::Create,
                target_id: None,
                milestone_id: None,
                title: "Phase A".to_string(),
                description: None,
                sequence: None,
                external_key: None,
                definition_of_done: None,
                status: None,
                rationale: None,
            }],
            work_item_changes: vec![
                work_item("Z item", ProposalOperation::Create, None),
                work_item("A item", ProposalOperation::Create, None),
            ],
            ..Default::default()
        };
        let labels = DiffLabels::default();
        let a = build_plan_diff(&proposal, &labels);
        let b = build_plan_diff(&proposal, &labels);
        assert_eq!(
            serde_json::to_value(&a).unwrap(),
            serde_json::to_value(&b).unwrap()
        );
        assert_eq!(a.phase_creates, 1);
        assert_eq!(a.work_item_creates, 2);
        // Sorted by label within (type, operation).
        let wi: Vec<&str> = a
            .entries
            .iter()
            .filter(|e| e.entity_type == EntityType::WorkItem)
            .map(|e| e.label.as_str())
            .collect();
        assert_eq!(wi, vec!["A item", "Z item"]);
    }

    #[test]
    fn update_uses_target_label() {
        let id = Uuid::new_v4();
        let mut labels = DiffLabels::default();
        labels.work_items.insert(id, "Existing".to_string());
        let proposal = PlanProposal {
            work_item_changes: vec![work_item("New title", ProposalOperation::Update, Some(id))],
            ..Default::default()
        };
        let diff = build_plan_diff(&proposal, &labels);
        assert_eq!(diff.entries[0].label, "Existing");
    }
}
