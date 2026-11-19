//! V0.39 Project State domain types.
//!
//! Every status is a finite enum (never a bare stringly-typed flag), and
//! every record is plain data — the repository is the only place that talks
//! SQL, the service the only place that composes records into snapshots.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use uuid::Uuid;

use crate::task_types::Priority;

// ============================================================
// Status enums
// ============================================================

/// Project lifecycle. Deliberately **not** `TaskState` — a project and a
/// task are different lifecycles and must never share state values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectStatus {
    Active,
    Paused,
    Archived,
    Completed,
}

impl ProjectStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Archived => "archived",
            Self::Completed => "completed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "active" => Self::Active,
            "paused" => Self::Paused,
            "archived" => Self::Archived,
            "completed" => Self::Completed,
            _ => return None,
        })
    }
}

impl std::fmt::Display for ProjectStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Milestone status on the long-lived roadmap.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MilestoneStatus {
    Planned,
    Active,
    Blocked,
    Deferred,
    Completed,
    Cancelled,
}

impl MilestoneStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Active => "active",
            Self::Blocked => "blocked",
            Self::Deferred => "deferred",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "planned" => Self::Planned,
            "active" => Self::Active,
            "blocked" => Self::Blocked,
            "deferred" => Self::Deferred,
            "completed" => Self::Completed,
            "cancelled" => Self::Cancelled,
            _ => return None,
        })
    }
}

impl std::fmt::Display for MilestoneStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// WorkItem status. `ReportedComplete` and `VerifiedComplete` are separate
/// on purpose: an Agent claiming done is not ARGUS having verified it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkItemStatus {
    Planned,
    Active,
    Blocked,
    Deferred,
    ReportedComplete,
    VerifiedComplete,
    Accepted,
    Cancelled,
}

impl WorkItemStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Active => "active",
            Self::Blocked => "blocked",
            Self::Deferred => "deferred",
            Self::ReportedComplete => "reported_complete",
            Self::VerifiedComplete => "verified_complete",
            Self::Accepted => "accepted",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "planned" => Self::Planned,
            "active" => Self::Active,
            "blocked" => Self::Blocked,
            "deferred" => Self::Deferred,
            "reported_complete" => Self::ReportedComplete,
            "verified_complete" => Self::VerifiedComplete,
            "accepted" => Self::Accepted,
            "cancelled" => Self::Cancelled,
            _ => return None,
        })
    }

    /// Whether this status represents work ARGUS considers durably complete.
    pub fn is_complete(self) -> bool {
        matches!(self, Self::VerifiedComplete | Self::Accepted)
    }
}

impl std::fmt::Display for WorkItemStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionStatus {
    Proposed,
    Accepted,
    Superseded,
    Rejected,
}

impl DecisionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Accepted => "accepted",
            Self::Superseded => "superseded",
            Self::Rejected => "rejected",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "proposed" => Self::Proposed,
            "accepted" => Self::Accepted,
            "superseded" => Self::Superseded,
            "rejected" => Self::Rejected,
            _ => return None,
        })
    }
}

impl std::fmt::Display for DecisionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskSeverity {
    Low,
    Medium,
    High,
    Critical,
}

impl RiskSeverity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Critical => "critical",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "low" => Self::Low,
            "medium" => Self::Medium,
            "high" => Self::High,
            "critical" => Self::Critical,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLikelihood {
    Low,
    Medium,
    High,
}

impl RiskLikelihood {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "low" => Self::Low,
            "medium" => Self::Medium,
            "high" => Self::High,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskStatus {
    Open,
    Mitigated,
    Accepted,
    Resolved,
}

impl RiskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Mitigated => "mitigated",
            Self::Accepted => "accepted",
            Self::Resolved => "resolved",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "open" => Self::Open,
            "mitigated" => Self::Mitigated,
            "accepted" => Self::Accepted,
            "resolved" => Self::Resolved,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockerStatus {
    Open,
    Resolved,
}

impl BlockerStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Resolved => "resolved",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "open" => Self::Open,
            "resolved" => Self::Resolved,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImprovementPriority {
    Low,
    Medium,
    High,
    Critical,
}

impl ImprovementPriority {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Critical => "critical",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "low" => Self::Low,
            "medium" => Self::Medium,
            "high" => Self::High,
            "critical" => Self::Critical,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImprovementStatus {
    Deferred,
    InProgress,
    Resolved,
    Dropped,
}

impl ImprovementStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deferred => "deferred",
            Self::InProgress => "in_progress",
            Self::Resolved => "resolved",
            Self::Dropped => "dropped",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "deferred" => Self::Deferred,
            "in_progress" => Self::InProgress,
            "resolved" => Self::Resolved,
            "dropped" => Self::Dropped,
            _ => return None,
        })
    }
}

/// Structured project timeline event types. This is a *business fact* set,
/// not the runtime EventBus vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityType {
    ProjectCreated,
    ProjectPaused,
    ProjectResumed,
    MilestoneCreated,
    MilestoneStarted,
    MilestoneCompleted,
    PhaseCreated,
    PhaseStarted,
    PhaseCompleted,
    WorkItemCreated,
    WorkItemStarted,
    WorkItemVerified,
    DecisionAccepted,
    RiskOpened,
    RiskResolved,
    BlockerOpened,
    BlockerResolved,
    ArtifactProduced,
    ImprovementRecorded,
    /// A deterministic progress snapshot was recorded (used only for
    /// `ProgressDiff`; not a per-tick analytics stream).
    ProgressRecorded,
}

impl ActivityType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProjectCreated => "project_created",
            Self::ProjectPaused => "project_paused",
            Self::ProjectResumed => "project_resumed",
            Self::MilestoneCreated => "milestone_created",
            Self::MilestoneStarted => "milestone_started",
            Self::MilestoneCompleted => "milestone_completed",
            Self::PhaseCreated => "phase_created",
            Self::PhaseStarted => "phase_started",
            Self::PhaseCompleted => "phase_completed",
            Self::WorkItemCreated => "work_item_created",
            Self::WorkItemStarted => "work_item_started",
            Self::WorkItemVerified => "work_item_verified",
            Self::DecisionAccepted => "decision_accepted",
            Self::RiskOpened => "risk_opened",
            Self::RiskResolved => "risk_resolved",
            Self::BlockerOpened => "blocker_opened",
            Self::BlockerResolved => "blocker_resolved",
            Self::ArtifactProduced => "artifact_produced",
            Self::ImprovementRecorded => "improvement_recorded",
            Self::ProgressRecorded => "progress_recorded",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "project_created" => Self::ProjectCreated,
            "project_paused" => Self::ProjectPaused,
            "project_resumed" => Self::ProjectResumed,
            "milestone_created" => Self::MilestoneCreated,
            "milestone_started" => Self::MilestoneStarted,
            "milestone_completed" => Self::MilestoneCompleted,
            "phase_created" => Self::PhaseCreated,
            "phase_started" => Self::PhaseStarted,
            "phase_completed" => Self::PhaseCompleted,
            "work_item_created" => Self::WorkItemCreated,
            "work_item_started" => Self::WorkItemStarted,
            "work_item_verified" => Self::WorkItemVerified,
            "decision_accepted" => Self::DecisionAccepted,
            "risk_opened" => Self::RiskOpened,
            "risk_resolved" => Self::RiskResolved,
            "blocker_opened" => Self::BlockerOpened,
            "blocker_resolved" => Self::BlockerResolved,
            "artifact_produced" => Self::ArtifactProduced,
            "improvement_recorded" => Self::ImprovementRecorded,
            "progress_recorded" => Self::ProgressRecorded,
            _ => return None,
        })
    }
}

impl std::fmt::Display for ActivityType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ============================================================
// Records
// ============================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectRecord {
    pub project_id: Uuid,
    pub name: String,
    pub slug: String,
    pub description: String,
    pub status: ProjectStatus,
    pub priority: Priority,
    pub git_repo_url: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Fields needed to create/ensure a project (id and timestamps are assigned
/// by the database).
#[derive(Clone, Debug)]
pub struct NewProject {
    pub name: String,
    pub slug: String,
    pub description: String,
    pub priority: Priority,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CharterRecord {
    pub project_id: Uuid,
    pub vision: String,
    pub goal: String,
    pub success_criteria: Vec<String>,
    pub constraints: Vec<String>,
    pub non_goals: Vec<String>,
    pub revision: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct NewCharter {
    pub project_id: Uuid,
    pub vision: String,
    pub goal: String,
    pub success_criteria: Vec<String>,
    pub constraints: Vec<String>,
    pub non_goals: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MilestoneRecord {
    pub milestone_id: Uuid,
    pub project_id: Uuid,
    pub title: String,
    pub description: Option<String>,
    pub status: MilestoneStatus,
    pub sequence: i32,
    pub parent_id: Option<Uuid>,
    pub definition_of_done: Option<String>,
    /// Stable, human-readable handle (e.g. `V0.40`). Never the primary key.
    pub external_key: Option<String>,
    /// Relative weight in the parent's progress aggregation. Default 1.
    pub weight: i32,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct NewMilestone {
    pub project_id: Uuid,
    pub title: String,
    pub description: Option<String>,
    /// `None` lets the repository assign the next sequence in the project.
    pub sequence: Option<i32>,
    pub parent_id: Option<Uuid>,
    pub definition_of_done: Option<String>,
    pub external_key: Option<String>,
    pub weight: i32,
}

/// A Phase groups WorkItems inside one Milestone. Four levels max:
/// Project -> Milestone -> Phase -> WorkItem.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PhaseRecord {
    pub phase_id: Uuid,
    pub project_id: Uuid,
    pub milestone_id: Uuid,
    pub title: String,
    pub description: Option<String>,
    pub status: MilestoneStatus,
    pub sequence: i32,
    pub external_key: Option<String>,
    pub definition_of_done: Option<String>,
    pub weight: i32,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct NewPhase {
    pub project_id: Uuid,
    pub milestone_id: Uuid,
    pub title: String,
    pub description: Option<String>,
    /// `None` lets the repository assign the next sequence in the milestone.
    pub sequence: Option<i32>,
    pub external_key: Option<String>,
    pub definition_of_done: Option<String>,
    pub weight: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkItemRecord {
    pub work_item_id: Uuid,
    pub project_id: Uuid,
    /// Roadmap node this item belongs to when it is a direct milestone child.
    pub milestone_id: Option<Uuid>,
    /// Phase this item belongs to, when it lives inside one.
    pub phase_id: Option<Uuid>,
    pub title: String,
    pub description: String,
    pub status: WorkItemStatus,
    pub priority: Priority,
    pub definition_of_done: Option<String>,
    pub source: Option<String>,
    /// Stable, human-readable handle (e.g. `P4-003`). Never the primary key.
    pub external_key: Option<String>,
    /// Relative weight in the parent's progress aggregation. Default 1.
    pub weight: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug)]
pub struct NewWorkItem {
    pub project_id: Uuid,
    pub milestone_id: Option<Uuid>,
    pub phase_id: Option<Uuid>,
    pub title: String,
    pub description: String,
    pub priority: Priority,
    pub definition_of_done: Option<String>,
    pub source: Option<String>,
    pub external_key: Option<String>,
    pub weight: i32,
}

/// A roadmap-level ordering edge: `work_item_id` should be complete before
/// `depends_on_id`... rather, `depends_on_id` must complete before
/// `work_item_id` starts. This is planning metadata, distinct from the
/// runtime TaskScheduler DAG.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkItemDependencyRecord {
    pub work_item_id: Uuid,
    pub depends_on_id: Uuid,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkItemEvidenceRecord {
    pub evidence_id: Uuid,
    pub work_item_id: Uuid,
    pub evidence_type: String,
    pub artifact_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub execution_id: Option<Uuid>,
    pub note: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct NewWorkItemEvidence {
    pub work_item_id: Uuid,
    pub evidence_type: String,
    pub artifact_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub execution_id: Option<Uuid>,
    pub note: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DecisionRecord {
    pub decision_id: Uuid,
    pub project_id: Uuid,
    pub title: String,
    pub context: Option<String>,
    pub decision: String,
    pub reason: Option<String>,
    pub status: DecisionStatus,
    pub source: Option<String>,
    pub superseded_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct NewDecision {
    pub project_id: Uuid,
    pub title: String,
    pub context: Option<String>,
    pub decision: String,
    pub reason: Option<String>,
    pub status: DecisionStatus,
    pub source: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RiskRecord {
    pub risk_id: Uuid,
    pub project_id: Uuid,
    pub milestone_id: Option<Uuid>,
    pub title: String,
    pub description: Option<String>,
    pub severity: RiskSeverity,
    pub likelihood: RiskLikelihood,
    pub status: RiskStatus,
    pub mitigation: Option<String>,
    pub owner: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug)]
pub struct NewRisk {
    pub project_id: Uuid,
    pub milestone_id: Option<Uuid>,
    pub title: String,
    pub description: Option<String>,
    pub severity: RiskSeverity,
    pub likelihood: RiskLikelihood,
    pub mitigation: Option<String>,
    pub owner: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlockerRecord {
    pub blocker_id: Uuid,
    pub project_id: Uuid,
    pub work_item_id: Option<Uuid>,
    pub title: String,
    pub description: Option<String>,
    pub status: BlockerStatus,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug)]
pub struct NewBlocker {
    pub project_id: Uuid,
    pub work_item_id: Option<Uuid>,
    pub title: String,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImprovementRecord {
    pub improvement_id: Uuid,
    pub project_id: Uuid,
    pub external_key: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub priority: ImprovementPriority,
    pub status: ImprovementStatus,
    pub source: Option<String>,
    pub why_deferred: Option<String>,
    pub required_before: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct NewImprovement {
    pub project_id: Uuid,
    pub external_key: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub priority: ImprovementPriority,
    pub status: ImprovementStatus,
    pub source: Option<String>,
    pub why_deferred: Option<String>,
    pub required_before: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActivityRecord {
    pub activity_id: Uuid,
    pub project_id: Uuid,
    pub activity_type: ActivityType,
    pub entity_type: String,
    pub entity_id: Option<Uuid>,
    pub summary: String,
    pub occurred_at: DateTime<Utc>,
    pub metadata: JsonValue,
}

#[derive(Clone, Debug)]
pub struct NewActivity {
    pub project_id: Uuid,
    pub activity_type: ActivityType,
    pub entity_type: String,
    pub entity_id: Option<Uuid>,
    pub summary: String,
    pub metadata: JsonValue,
}

/// A lightweight view of a V0.38 `task_artifacts` row, used by snapshots.
/// Kept separate from `crate::artifact_repository::ArtifactRecord` so the
/// snapshot does not depend on the full V0.38 artifact shape.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArtifactSummary {
    pub artifact_id: Uuid,
    pub task_id: Uuid,
    pub workspace_id: Uuid,
    pub commit_sha: String,
    pub branch_name: String,
    pub files_changed: Vec<String>,
    pub quality_status: Option<String>,
    pub created_at: DateTime<Utc>,
}

// ============================================================
// Snapshot
// ============================================================

/// A deterministic, machine-readable summary of a project's current state.
/// Generated from structured rows — never a prose summary that has to be
/// re-parsed.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectSnapshot {
    pub project: ProjectRecord,
    pub charter: Option<CharterRecord>,
    pub current_milestone: Option<MilestoneRecord>,
    pub milestones: Vec<MilestoneRecord>,
    pub active_work_items: Vec<WorkItemRecord>,
    pub blocked_work_items: Vec<WorkItemRecord>,
    pub open_risks: Vec<RiskRecord>,
    pub open_blockers: Vec<BlockerRecord>,
    pub deferred_improvements: Vec<ImprovementRecord>,
    pub deferred_count: usize,
    pub recent_artifacts: Vec<ArtifactSummary>,
    pub recent_activity: Vec<ActivityRecord>,
    /// Planned work items ARGUS could pick up next (ordered by priority).
    pub next_candidates: Vec<WorkItemRecord>,
    pub updated_at: DateTime<Utc>,
}

// ============================================================
// V0.40 Progress Engine
// ============================================================

/// How much a progress number can be trusted.
///
/// Formal completion is always `Verified`: the engine only counts work that
/// reached `VerifiedComplete`/`Accepted`. `Estimated` marks a node whose
/// number cannot be pinned down because it still has in-flight items
/// (`Active`, `Blocked`, `ReportedComplete`). `Unknown` means there is
/// nothing to measure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressConfidence {
    Verified,
    Estimated,
    Unknown,
}

/// Deterministic, flat progress summary over a set of roadmap items.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ProgressSummary {
    /// Items counted in the denominator (excludes Deferred and Cancelled).
    pub total_items: usize,
    pub verified_items: usize,
    pub accepted_items: usize,
    pub reported_complete_items: usize,
    pub active_items: usize,
    pub blocked_items: usize,
    pub deferred_items: usize,
    pub cancelled_items: usize,
    /// Weighted completion of included items, in `0.0..=1.0`.
    pub completion_ratio: f64,
    pub confidence: ProgressConfidence,
}

impl ProgressSummary {
    pub fn verified_total(&self) -> usize {
        self.verified_items + self.accepted_items
    }

    pub fn is_complete(&self) -> bool {
        self.total_items > 0
            && self.verified_total() == self.total_items
            && self.active_items == 0
            && self.blocked_items == 0
            && self.reported_complete_items == 0
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoadmapNodeRef {
    pub id: Uuid,
    pub title: String,
    pub status: MilestoneStatus,
    pub external_key: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkItemProgress {
    pub work_item_id: Uuid,
    pub title: String,
    pub status: WorkItemStatus,
    pub priority: Priority,
    pub weight: i32,
    pub external_key: Option<String>,
    /// `1.0` for `VerifiedComplete`/`Accepted`, else `0.0`. Never a guess.
    pub completion: f64,
    /// Whether this item participates in its parent's denominator.
    pub included: bool,
    /// WorkItem ids this item depends on.
    pub depends_on: Vec<Uuid>,
    /// Whether every dependency is complete and the item is Planned.
    pub ready: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PhaseProgress {
    pub phase_id: Uuid,
    pub title: String,
    pub status: MilestoneStatus,
    pub sequence: i32,
    pub external_key: Option<String>,
    pub summary: ProgressSummary,
    pub work_items: Vec<WorkItemProgress>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MilestoneProgress {
    pub milestone_id: Uuid,
    pub title: String,
    pub status: MilestoneStatus,
    pub sequence: i32,
    pub external_key: Option<String>,
    /// This milestone's weight in the project-overall aggregation.
    pub weight: i32,
    pub summary: ProgressSummary,
    /// WorkItems attached directly to the milestone (no phase).
    pub work_items: Vec<WorkItemProgress>,
    pub phases: Vec<PhaseProgress>,
}

/// The full, deterministic roadmap with computed progress.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoadmapProgress {
    pub project_id: Uuid,
    pub overall: ProgressSummary,
    pub current_milestone: Option<RoadmapNodeRef>,
    pub current_phase: Option<RoadmapNodeRef>,
    pub milestones: Vec<MilestoneProgress>,
    pub blocked_count: usize,
    pub deferred_count: usize,
    pub verified_count: usize,
    pub active_count: usize,
    /// Open risk titles (for diffing "resolved" risk transitions).
    pub open_risks: Vec<String>,
    pub open_blockers: Vec<String>,
    /// Deferred-improvement labels (for diffing "newly deferred").
    pub deferred: Vec<String>,
    pub next_candidates: Vec<WorkItemProgress>,
    pub updated_at: DateTime<Utc>,
}

/// Compact deterministic progress report (the `/progress` view).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProgressReport {
    pub project_id: Uuid,
    pub overall: ProgressSummary,
    /// `overall.completion_ratio * 100`, rounded to one decimal (stable).
    pub completion_percent: f64,
    pub current_milestone: Option<RoadmapNodeRef>,
    pub current_phase: Option<RoadmapNodeRef>,
    pub active_work_items: Vec<WorkItemProgress>,
    pub next_candidates: Vec<WorkItemProgress>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProgressChange {
    pub node: String,
    pub before_ratio: f64,
    pub after_ratio: f64,
}

/// A deterministic diff between two progress snapshots. This is the input a
/// future Portfolio / Planner reads to see what changed — never a model's
/// summary of it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProgressDiff {
    pub project_id: Uuid,
    pub before_overall: f64,
    pub after_overall: f64,
    pub completed_added: Vec<String>,
    pub started_added: Vec<String>,
    pub blocked_added: Vec<String>,
    pub deferred_added: Vec<String>,
    pub risks_resolved: Vec<String>,
    pub blockers_resolved: Vec<String>,
    pub progress_changes: Vec<ProgressChange>,
}

impl ProgressDiff {
    /// Compare two roadmap progress snapshots. Pure and deterministic: the
    /// same two inputs always produce the same diff.
    pub fn between(before: &RoadmapProgress, after: &RoadmapProgress) -> Self {
        let before_items = work_item_index(before);
        let after_items = work_item_index(after);

        let mut completed_added = Vec::new();
        let mut started_added = Vec::new();
        let mut blocked_added = Vec::new();
        for (id, after_item) in &after_items {
            let before_item = before_items.get(id);
            let before_status = before_item.map(|w| w.status);
            if after_item.status == WorkItemStatus::VerifiedComplete
                || after_item.status == WorkItemStatus::Accepted
            {
                if !before_status.is_some_and(WorkItemStatus::is_complete) {
                    completed_added.push(item_label(after_item));
                }
            } else if after_item.status == WorkItemStatus::Active
                && before_status != Some(WorkItemStatus::Active)
            {
                started_added.push(item_label(after_item));
            } else if after_item.status == WorkItemStatus::Blocked
                && before_status != Some(WorkItemStatus::Blocked)
            {
                blocked_added.push(item_label(after_item));
            }
        }

        let deferred_added = difference(&after.deferred, &before.deferred);
        let risks_resolved = difference(&before.open_risks, &after.open_risks);
        let blockers_resolved = difference(&before.open_blockers, &after.open_blockers);

        let before_milestones = milestone_ratio_index(before);
        let after_milestones = milestone_ratio_index(after);
        let mut progress_changes = Vec::new();
        for (id, after_ratio) in &after_milestones {
            if let Some(before_ratio) = before_milestones.get(id) {
                if (before_ratio - after_ratio).abs() > f64::EPSILON {
                    progress_changes.push(ProgressChange {
                        node: milestone_label(after, *id),
                        before_ratio: *before_ratio,
                        after_ratio: *after_ratio,
                    });
                }
            }
        }

        Self {
            project_id: after.project_id,
            before_overall: before.overall.completion_ratio,
            after_overall: after.overall.completion_ratio,
            completed_added,
            started_added,
            blocked_added,
            deferred_added,
            risks_resolved,
            blockers_resolved,
            progress_changes,
        }
    }
}

fn work_item_index(r: &RoadmapProgress) -> std::collections::HashMap<Uuid, &WorkItemProgress> {
    let mut map = std::collections::HashMap::new();
    for m in &r.milestones {
        for w in &m.work_items {
            map.insert(w.work_item_id, w);
        }
        for p in &m.phases {
            for w in &p.work_items {
                map.insert(w.work_item_id, w);
            }
        }
    }
    map
}

fn milestone_ratio_index(r: &RoadmapProgress) -> std::collections::HashMap<Uuid, f64> {
    r.milestones
        .iter()
        .map(|m| (m.milestone_id, m.summary.completion_ratio))
        .collect()
}

fn milestone_label(r: &RoadmapProgress, id: Uuid) -> String {
    r.milestones
        .iter()
        .find(|m| m.milestone_id == id)
        .map(|m| match &m.external_key {
            Some(k) => format!("{k} {}", m.title),
            None => m.title.clone(),
        })
        .unwrap_or_else(|| id.to_string())
}

fn item_label(w: &WorkItemProgress) -> String {
    match &w.external_key {
        Some(k) => format!("{k} {}", w.title),
        None => w.title.clone(),
    }
}

/// Elements present in `after` but not in `before`, preserving order.
fn difference(after: &[String], before: &[String]) -> Vec<String> {
    after
        .iter()
        .filter(|s| !before.contains(s))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_status_round_trips() {
        for s in [
            ProjectStatus::Active,
            ProjectStatus::Paused,
            ProjectStatus::Archived,
            ProjectStatus::Completed,
        ] {
            assert_eq!(ProjectStatus::parse(s.as_str()), Some(s));
        }
        for s in [
            MilestoneStatus::Planned,
            MilestoneStatus::Active,
            MilestoneStatus::Blocked,
            MilestoneStatus::Deferred,
            MilestoneStatus::Completed,
            MilestoneStatus::Cancelled,
        ] {
            assert_eq!(MilestoneStatus::parse(s.as_str()), Some(s));
        }
        for s in [
            WorkItemStatus::Planned,
            WorkItemStatus::Active,
            WorkItemStatus::Blocked,
            WorkItemStatus::Deferred,
            WorkItemStatus::ReportedComplete,
            WorkItemStatus::VerifiedComplete,
            WorkItemStatus::Accepted,
            WorkItemStatus::Cancelled,
        ] {
            assert_eq!(WorkItemStatus::parse(s.as_str()), Some(s));
        }
        assert_eq!(
            DecisionStatus::parse("superseded"),
            Some(DecisionStatus::Superseded)
        );
        assert_eq!(
            RiskSeverity::parse("critical"),
            Some(RiskSeverity::Critical)
        );
        assert_eq!(RiskLikelihood::parse("high"), Some(RiskLikelihood::High));
        assert_eq!(RiskStatus::parse("resolved"), Some(RiskStatus::Resolved));
        assert_eq!(BlockerStatus::parse("open"), Some(BlockerStatus::Open));
        assert_eq!(
            ImprovementPriority::parse("medium"),
            Some(ImprovementPriority::Medium)
        );
        assert_eq!(
            ImprovementStatus::parse("deferred"),
            Some(ImprovementStatus::Deferred)
        );
        assert_eq!(
            ActivityType::parse("artifact_produced"),
            Some(ActivityType::ArtifactProduced)
        );
        assert_eq!(ActivityType::parse("nonsense"), None);
    }

    #[test]
    fn test_reported_complete_is_not_verified_complete() {
        // An Agent reporting done must never be treated as ARGUS-verified.
        assert!(!WorkItemStatus::ReportedComplete.is_complete());
        assert!(WorkItemStatus::VerifiedComplete.is_complete());
        assert!(WorkItemStatus::Accepted.is_complete());
        assert!(!WorkItemStatus::Active.is_complete());
    }

    #[test]
    fn test_project_status_is_distinct_from_task_state() {
        // Guard against silently reusing TaskState values for projects: the
        // project lifecycle has its own str set.
        assert_eq!(ProjectStatus::Completed.as_str(), "completed");
        assert_eq!(ProjectStatus::Paused.as_str(), "paused");
        assert!(ProjectStatus::parse("running").is_none());
        assert!(ProjectStatus::parse("queued").is_none());
    }
}
