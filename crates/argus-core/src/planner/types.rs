//! V0.43 Planner Protocol domain types.
//!
//! A [`PlanningRequest`] is bound to an exact V0.41 ContextPacket (by
//! `context_hash`, never an opaque blob). A Planner returns a
//! [`PlanningResult`] containing a structured [`PlanProposal`] — never free
//! text, never provider-specific data. The proposal is validated, diffed,
//! explicitly approved, and only then applied to Project State.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::project_state::{
    ImprovementPriority, MilestoneStatus, RiskLikelihood, RiskSeverity, WorkItemStatus,
};
use crate::task_types::Priority;

/// The protocol version bound to every request and result.
pub const PROTOCOL_VERSION: &str = "planner-protocol-v1";

// ============================================================
// Enums
// ============================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanningKind {
    ProjectPlanning,
    MilestonePlanning,
    PhasePlanning,
    WorkItemDecomposition,
    Replanning,
    FailureRecovery,
    ArchitecturePlanning,
    StatusReview,
}

impl PlanningKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProjectPlanning => "project_planning",
            Self::MilestonePlanning => "milestone_planning",
            Self::PhasePlanning => "phase_planning",
            Self::WorkItemDecomposition => "work_item_decomposition",
            Self::Replanning => "replanning",
            Self::FailureRecovery => "failure_recovery",
            Self::ArchitecturePlanning => "architecture_planning",
            Self::StatusReview => "status_review",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.replace('-', "_").as_str() {
            "project_planning" => Self::ProjectPlanning,
            "milestone_planning" => Self::MilestonePlanning,
            "phase_planning" => Self::PhasePlanning,
            "work_item_decomposition" => Self::WorkItemDecomposition,
            "replanning" => Self::Replanning,
            "failure_recovery" => Self::FailureRecovery,
            "architecture_planning" => Self::ArchitecturePlanning,
            "status_review" => Self::StatusReview,
            _ => return None,
        })
    }
}

impl std::fmt::Display for PlanningKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestedOutput {
    RoadmapChanges,
    WorkItems,
    Decisions,
    Risks,
    DeferredImprovements,
    Clarifications,
    NoChange,
}

impl RequestedOutput {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RoadmapChanges => "roadmap_changes",
            Self::WorkItems => "work_items",
            Self::Decisions => "decisions",
            Self::Risks => "risks",
            Self::DeferredImprovements => "deferred_improvements",
            Self::Clarifications => "clarifications",
            Self::NoChange => "no_change",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "roadmap_changes" => Self::RoadmapChanges,
            "work_items" => Self::WorkItems,
            "decisions" => Self::Decisions,
            "risks" => Self::Risks,
            "deferred_improvements" => Self::DeferredImprovements,
            "clarifications" => Self::Clarifications,
            "no_change" => Self::NoChange,
            _ => return None,
        })
    }
}

impl std::fmt::Display for RequestedOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanningStatus {
    Proposed,
    NeedsClarification,
    NoChange,
    Blocked,
    Invalid,
}

impl PlanningStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::NeedsClarification => "needs_clarification",
            Self::NoChange => "no_change",
            Self::Blocked => "blocked",
            Self::Invalid => "invalid",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "proposed" => Self::Proposed,
            "needs_clarification" => Self::NeedsClarification,
            "no_change" => Self::NoChange,
            "blocked" => Self::Blocked,
            "invalid" => Self::Invalid,
            _ => return None,
        })
    }
}

/// The only change primitives a Planner may express. No arbitrary SQL / JSON
/// patch is ever applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalOperation {
    Create,
    Update,
    Transition,
    Supersede,
    Defer,
    Cancel,
    NoOp,
}

impl ProposalOperation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Update => "update",
            Self::Transition => "transition",
            Self::Supersede => "supersede",
            Self::Defer => "defer",
            Self::Cancel => "cancel",
            Self::NoOp => "no_op",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "create" => Self::Create,
            "update" => Self::Update,
            "transition" => Self::Transition,
            "supersede" => Self::Supersede,
            "defer" => Self::Defer,
            "cancel" => Self::Cancel,
            "no_op" => Self::NoOp,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityType {
    Milestone,
    Phase,
    WorkItem,
    Decision,
    Risk,
    Blocker,
    DeferredImprovement,
}

impl EntityType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Milestone => "milestone",
            Self::Phase => "phase",
            Self::WorkItem => "work_item",
            Self::Decision => "decision",
            Self::Risk => "risk",
            Self::Blocker => "blocker",
            Self::DeferredImprovement => "deferred_improvement",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    Draft,
    Validated,
    Rejected,
    Approved,
    Applied,
    Stale,
    Failed,
}

impl ProposalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Validated => "validated",
            Self::Rejected => "rejected",
            Self::Approved => "approved",
            Self::Applied => "applied",
            Self::Stale => "stale",
            Self::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "draft" => Self::Draft,
            "validated" => Self::Validated,
            "rejected" => Self::Rejected,
            "approved" => Self::Approved,
            "applied" => Self::Applied,
            "stale" => Self::Stale,
            "failed" => Self::Failed,
            _ => return None,
        })
    }
}

// ============================================================
// Proposal pieces (strict — unknown fields are rejected)
// ============================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposedMilestone {
    pub operation: ProposalOperation,
    pub target_id: Option<Uuid>,
    pub title: String,
    pub description: Option<String>,
    pub sequence: Option<i32>,
    pub external_key: Option<String>,
    pub definition_of_done: Option<String>,
    pub status: Option<MilestoneStatus>,
    pub rationale: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposedPhase {
    pub operation: ProposalOperation,
    pub target_id: Option<Uuid>,
    pub milestone_id: Option<Uuid>,
    pub title: String,
    pub description: Option<String>,
    pub sequence: Option<i32>,
    pub external_key: Option<String>,
    pub definition_of_done: Option<String>,
    pub status: Option<MilestoneStatus>,
    pub rationale: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposedWorkItem {
    pub operation: ProposalOperation,
    pub target_id: Option<Uuid>,
    pub title: String,
    pub description: Option<String>,
    pub definition_of_done: Option<String>,
    pub milestone_id: Option<Uuid>,
    pub phase_id: Option<Uuid>,
    pub priority: Option<Priority>,
    pub status: Option<WorkItemStatus>,
    #[serde(default)]
    pub dependencies: Vec<Uuid>,
    pub external_key: Option<String>,
    pub weight: Option<i32>,
    pub rationale: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionProposal {
    pub title: String,
    pub context: Option<String>,
    pub proposed_decision: String,
    pub rationale: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RiskProposal {
    pub title: String,
    pub description: Option<String>,
    pub severity: RiskSeverity,
    pub likelihood: RiskLikelihood,
    pub mitigation: Option<String>,
    pub owner: Option<String>,
    pub rationale: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockerProposal {
    pub title: String,
    pub description: Option<String>,
    pub work_item_id: Option<Uuid>,
    pub rationale: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeferredImprovementProposal {
    pub title: String,
    pub description: Option<String>,
    pub external_key: Option<String>,
    pub priority: Option<ImprovementPriority>,
    pub why_deferred: Option<String>,
    pub required_before: Option<String>,
    pub rationale: Option<String>,
}

/// The structured proposal. Every field is a typed list of operations.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanProposal {
    #[serde(default)]
    pub milestone_changes: Vec<ProposedMilestone>,
    #[serde(default)]
    pub phase_changes: Vec<ProposedPhase>,
    #[serde(default)]
    pub work_item_changes: Vec<ProposedWorkItem>,
    #[serde(default)]
    pub decision_proposals: Vec<DecisionProposal>,
    #[serde(default)]
    pub risk_proposals: Vec<RiskProposal>,
    #[serde(default)]
    pub blocker_proposals: Vec<BlockerProposal>,
    #[serde(default)]
    pub deferred_improvement_proposals: Vec<DeferredImprovementProposal>,
}

impl PlanProposal {
    pub fn is_empty(&self) -> bool {
        self.milestone_changes.is_empty()
            && self.phase_changes.is_empty()
            && self.work_item_changes.is_empty()
            && self.decision_proposals.is_empty()
            && self.risk_proposals.is_empty()
            && self.blocker_proposals.is_empty()
            && self.deferred_improvement_proposals.is_empty()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanningClarification {
    pub question: String,
    pub why_needed: Option<String>,
    pub blocking: bool,
}

/// Provider-neutral planner metadata. Never a provider/model registry entry.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannerMetadata {
    pub planner_type: String,
    pub planner_ref: Option<String>,
    pub model_ref: Option<String>,
    pub session_ref: Option<String>,
    pub duration_ms: Option<u64>,
    pub usage: Option<PlannerUsage>,
}

/// Usage is best-effort. Missing values are `None` ("unknown"), never `$0`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannerUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cost_usd: Option<f64>,
}

// ============================================================
// Request / Result
// ============================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlanningRequest {
    pub planning_request_id: Uuid,
    pub project_id: Uuid,
    pub planning_kind: PlanningKind,
    pub role: String,
    pub purpose: String,
    pub context_packet_ref: Option<Uuid>,
    pub context_hash: String,
    pub compiler_version: String,
    /// Optional focused WorkItem; used to regenerate the bound context for
    /// manual export.
    pub work_item_id: Option<Uuid>,
    pub task_profile_ref: Option<Uuid>,
    pub routing_decision_ref: Option<Uuid>,
    pub state_revision: i64,
    pub roadmap_revision: i64,
    pub objective: String,
    pub constraints: Vec<String>,
    pub requested_outputs: Vec<RequestedOutput>,
    pub protocol_version: String,
    pub request_hash: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanningResult {
    pub planning_result_id: Uuid,
    pub planning_request_id: Uuid,
    pub status: PlanningStatus,
    pub summary: Option<String>,
    pub proposal: PlanProposal,
    #[serde(default)]
    pub assumptions: Vec<String>,
    #[serde(default)]
    pub clarifications: Vec<PlanningClarification>,
    #[serde(default)]
    pub warnings: Vec<String>,
    pub planner_metadata: PlannerMetadata,
    pub protocol_version: String,
    pub context_hash: String,
    pub request_hash: String,
    #[serde(default)]
    pub result_hash: String,
    pub created_at: DateTime<Utc>,
}

// ============================================================
// Diff
// ============================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiffEntry {
    pub entity_type: EntityType,
    pub operation: ProposalOperation,
    pub label: String,
    pub rationale: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PlanDiff {
    pub entries: Vec<DiffEntry>,
    pub milestone_creates: usize,
    pub phase_creates: usize,
    pub work_item_creates: usize,
    pub decision_proposals: usize,
    pub risk_proposals: usize,
    pub blocker_proposals: usize,
    pub deferred_improvement_proposals: usize,
}

impl PlanDiff {
    pub fn summary(&self) -> String {
        format!(
            "+{} milestone, +{} phase, +{} work item; {} decision, {} risk, {} blocker, {} deferred",
            self.milestone_creates,
            self.phase_creates,
            self.work_item_creates,
            self.decision_proposals,
            self.risk_proposals,
            self.blocker_proposals,
            self.deferred_improvement_proposals
        )
    }
}

// ============================================================
// Persisted records
// ============================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectRevisions {
    pub project_id: Uuid,
    pub state_revision: i64,
    pub roadmap_revision: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlanningRequestRecord {
    pub request: PlanningRequest,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlanningResultRecord {
    pub result: PlanningResult,
    pub project_id: Uuid,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProposalRecord {
    pub proposal_id: Uuid,
    pub planning_result_id: Uuid,
    pub planning_request_id: Uuid,
    pub project_id: Uuid,
    pub status: ProposalStatus,
    pub proposal: PlanProposal,
    pub validation_errors: Vec<String>,
    pub context_hash: String,
    pub request_hash: String,
    pub result_hash: String,
    pub state_revision: i64,
    pub roadmap_revision: i64,
    pub applied_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LifecycleEvent {
    pub event_id: Uuid,
    pub proposal_id: Uuid,
    pub project_id: Uuid,
    pub event_type: String,
    pub detail: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Outcome of applying a proposal.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApplyOutcome {
    pub proposal_id: Uuid,
    pub already_applied: bool,
    pub milestones_created: usize,
    pub phases_created: usize,
    pub work_items_created: usize,
    pub decisions_created: usize,
    pub risks_created: usize,
    pub blockers_created: usize,
    pub deferred_improvements_created: usize,
    pub state_revision: i64,
    pub roadmap_revision: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enums_round_trip() {
        for k in [
            PlanningKind::ProjectPlanning,
            PlanningKind::ArchitecturePlanning,
            PlanningKind::StatusReview,
        ] {
            assert_eq!(PlanningKind::parse(k.as_str()), Some(k));
        }
        for o in [
            ProposalOperation::Create,
            ProposalOperation::Transition,
            ProposalOperation::NoOp,
        ] {
            assert_eq!(ProposalOperation::parse(o.as_str()), Some(o));
        }
        for s in [
            ProposalStatus::Draft,
            ProposalStatus::Validated,
            ProposalStatus::Stale,
            ProposalStatus::Applied,
        ] {
            assert_eq!(ProposalStatus::parse(s.as_str()), Some(s));
        }
    }

    #[test]
    fn proposal_rejects_unknown_fields() {
        let json = r#"{"work_item_changes":[],"command":"rm -rf /"}"#;
        assert!(
            serde_json::from_str::<PlanProposal>(json).is_err(),
            "unknown fields must be rejected"
        );
        let ok = r#"{"work_item_changes":[]}"#;
        assert!(serde_json::from_str::<PlanProposal>(ok).is_ok());
    }
}
