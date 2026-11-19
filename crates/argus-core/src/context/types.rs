//! V0.41 Context Compiler domain types.
//!
//! [`ContextRequest`] is what a caller asks for (role + purpose + optional
//! focus + budget); [`ContextPacket`] is the immutable, hashed result. Every
//! fact in a packet carries a [`ContextSourceRef`], so a later Planner output
//! can always be traced back to exactly what it saw.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::project_state::ProgressSummary;
use crate::task_types::Priority;

/// Bumping this changes every context hash and signals that the selection
/// rules changed — historical executions stay explainable.
pub const CONTEXT_COMPILER_VERSION: &str = "context-compiler-v1";

// ============================================================
// Role / purpose / sections
// ============================================================

/// A logical role, never a provider. Claude/Codex/OpenCode are runtimes, not
/// roles (ADR #1/#2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextRole {
    Chief,
    ProjectManager,
    Developer,
    Tester,
    Reviewer,
    Researcher,
    Integrator,
}

impl ContextRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chief => "chief",
            Self::ProjectManager => "project_manager",
            Self::Developer => "developer",
            Self::Tester => "tester",
            Self::Reviewer => "reviewer",
            Self::Researcher => "researcher",
            Self::Integrator => "integrator",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "chief" => Self::Chief,
            "project_manager" | "project-manager" => Self::ProjectManager,
            "developer" => Self::Developer,
            "tester" => Self::Tester,
            "reviewer" => Self::Reviewer,
            "researcher" => Self::Researcher,
            "integrator" => Self::Integrator,
            _ => return None,
        })
    }
}

impl std::fmt::Display for ContextRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What the consumer is about to do. The same role needs different context
/// for planning versus incident review.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextPurpose {
    Planning,
    Execution,
    Review,
    Debugging,
    Research,
    Integration,
    StatusReview,
}

impl ContextPurpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Planning => "planning",
            Self::Execution => "execution",
            Self::Review => "review",
            Self::Debugging => "debugging",
            Self::Research => "research",
            Self::Integration => "integration",
            Self::StatusReview => "status_review",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "planning" => Self::Planning,
            "execution" => Self::Execution,
            "review" => Self::Review,
            "debugging" => Self::Debugging,
            "research" => Self::Research,
            "integration" => Self::Integration,
            "status_review" | "status-review" => Self::StatusReview,
            _ => return None,
        })
    }
}

impl std::fmt::Display for ContextPurpose {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A named section of a packet. Used for `requested_sections` and to report
/// what truncation omitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextSection {
    ProjectIdentity,
    Charter,
    CurrentMilestone,
    CurrentPhase,
    CurrentWorkItem,
    Roadmap,
    Decisions,
    Risks,
    Blockers,
    Artifacts,
    RecentActivity,
    Dependencies,
    DeferredImprovements,
    Constraints,
}

impl ContextSection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProjectIdentity => "project_identity",
            Self::Charter => "charter",
            Self::CurrentMilestone => "current_milestone",
            Self::CurrentPhase => "current_phase",
            Self::CurrentWorkItem => "current_work_item",
            Self::Roadmap => "roadmap",
            Self::Decisions => "decisions",
            Self::Risks => "risks",
            Self::Blockers => "blockers",
            Self::Artifacts => "artifacts",
            Self::RecentActivity => "recent_activity",
            Self::Dependencies => "dependencies",
            Self::DeferredImprovements => "deferred_improvements",
            Self::Constraints => "constraints",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "project_identity" => Self::ProjectIdentity,
            "charter" => Self::Charter,
            "current_milestone" => Self::CurrentMilestone,
            "current_phase" => Self::CurrentPhase,
            "current_work_item" => Self::CurrentWorkItem,
            "roadmap" => Self::Roadmap,
            "decisions" => Self::Decisions,
            "risks" => Self::Risks,
            "blockers" => Self::Blockers,
            "artifacts" => Self::Artifacts,
            "recent_activity" => Self::RecentActivity,
            "dependencies" => Self::Dependencies,
            "deferred_improvements" => Self::DeferredImprovements,
            "constraints" => Self::Constraints,
            _ => return None,
        })
    }
}

impl std::fmt::Display for ContextSection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ============================================================
// Provenance
// ============================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextSourceType {
    Project,
    Charter,
    Milestone,
    Phase,
    WorkItem,
    Decision,
    Risk,
    Blocker,
    Artifact,
    Activity,
    Improvement,
    Constraint,
    Task,
    Execution,
}

impl ContextSourceType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Charter => "charter",
            Self::Milestone => "milestone",
            Self::Phase => "phase",
            Self::WorkItem => "work_item",
            Self::Decision => "decision",
            Self::Risk => "risk",
            Self::Blocker => "blocker",
            Self::Artifact => "artifact",
            Self::Activity => "activity",
            Self::Improvement => "improvement",
            Self::Constraint => "constraint",
            Self::Task => "task",
            Self::Execution => "execution",
        }
    }
}

/// Where a single fact in the packet came from.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ContextSourceRef {
    pub source_type: ContextSourceType,
    pub source_id: Uuid,
    /// e.g. a Charter revision, when the source is versioned.
    pub revision: Option<i32>,
    pub created_at: DateTime<Utc>,
}

impl ContextSourceRef {
    pub fn new(
        source_type: ContextSourceType,
        source_id: Uuid,
        revision: Option<i32>,
        created_at: DateTime<Utc>,
    ) -> Self {
        Self {
            source_type,
            source_id,
            revision,
            created_at,
        }
    }
}

// ============================================================
// Request
// ============================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextRequest {
    pub project_id: Uuid,
    pub role: ContextRole,
    pub purpose: ContextPurpose,
    pub work_item_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub execution_id: Option<Uuid>,
    pub max_items: Option<usize>,
    pub max_bytes: Option<usize>,
    pub estimated_token_budget: Option<usize>,
    pub requested_sections: Option<Vec<ContextSection>>,
    pub include_recent_activity: bool,
    pub include_artifacts: bool,
    /// Caller-supplied request time. Deliberately excluded from the hash:
    /// only structural inputs affect the compiled context.
    pub created_at: DateTime<Utc>,
}

impl ContextRequest {
    pub fn new(project_id: Uuid, role: ContextRole, purpose: ContextPurpose) -> Self {
        Self {
            project_id,
            role,
            purpose,
            work_item_id: None,
            task_id: None,
            execution_id: None,
            max_items: None,
            max_bytes: None,
            estimated_token_budget: None,
            requested_sections: None,
            include_recent_activity: true,
            include_artifacts: true,
            created_at: Utc::now(),
        }
    }

    pub fn with_work_item(mut self, work_item_id: Uuid) -> Self {
        self.work_item_id = Some(work_item_id);
        self
    }

    pub fn with_task(mut self, task_id: Uuid) -> Self {
        self.task_id = Some(task_id);
        self
    }

    pub fn with_execution(mut self, execution_id: Uuid) -> Self {
        self.execution_id = Some(execution_id);
        self
    }

    pub fn with_budget(mut self, max_items: usize, max_bytes: usize) -> Self {
        self.max_items = Some(max_items);
        self.max_bytes = Some(max_bytes);
        self
    }

    pub fn with_token_budget(mut self, tokens: usize) -> Self {
        self.estimated_token_budget = Some(tokens);
        self
    }

    pub fn with_sections(mut self, sections: Vec<ContextSection>) -> Self {
        self.requested_sections = Some(sections);
        self
    }

    pub fn without_activity(mut self) -> Self {
        self.include_recent_activity = false;
        self
    }

    pub fn without_artifacts(mut self) -> Self {
        self.include_artifacts = false;
        self
    }
}

// ============================================================
// Packet sections
// ============================================================

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectIdentityContext {
    pub project_id: Uuid,
    pub name: String,
    pub slug: String,
    pub description: String,
    pub status: String,
    pub priority: Priority,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CharterContext {
    pub vision: String,
    pub goal: String,
    pub success_criteria: Vec<String>,
    pub constraints: Vec<String>,
    pub non_goals: Vec<String>,
    pub revision: i32,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MilestoneContext {
    pub milestone_id: Uuid,
    pub title: String,
    pub status: String,
    pub sequence: i32,
    pub external_key: Option<String>,
    pub completion_ratio: f64,
    pub verified_items: usize,
    pub total_items: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PhaseContext {
    pub phase_id: Uuid,
    pub title: String,
    pub status: String,
    pub sequence: i32,
    pub external_key: Option<String>,
    pub completion_ratio: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkItemEvidenceRef {
    pub evidence_id: Uuid,
    pub evidence_type: String,
    pub artifact_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub execution_id: Option<Uuid>,
    pub note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkItemContext {
    pub work_item_id: Uuid,
    pub title: String,
    pub description: String,
    pub status: String,
    pub priority: Priority,
    pub definition_of_done: Option<String>,
    pub external_key: Option<String>,
    pub weight: i32,
    pub milestone_id: Option<Uuid>,
    pub phase_id: Option<Uuid>,
    pub depends_on: Vec<Uuid>,
    pub evidence: Vec<WorkItemEvidenceRef>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RoadmapContext {
    pub overall: ProgressSummary,
    pub milestones: Vec<MilestoneContext>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DecisionContext {
    pub decision_id: Uuid,
    pub title: String,
    pub decision: String,
    pub reason: Option<String>,
    pub status: String,
    pub source: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RiskContext {
    pub risk_id: Uuid,
    pub title: String,
    pub severity: String,
    pub likelihood: String,
    pub status: String,
    pub mitigation: Option<String>,
    pub milestone_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlockerContext {
    pub blocker_id: Uuid,
    pub title: String,
    pub status: String,
    pub work_item_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ArtifactContext {
    pub artifact_id: Uuid,
    pub task_id: Uuid,
    pub workspace_id: Uuid,
    pub commit_sha: String,
    pub branch_name: String,
    pub files_changed: Vec<String>,
    pub quality_status: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActivityContext {
    pub activity_id: Uuid,
    pub activity_type: String,
    pub entity_type: String,
    pub entity_id: Option<Uuid>,
    pub summary: String,
    pub occurred_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DependencyContext {
    pub work_item_id: Uuid,
    pub depends_on_id: Uuid,
    pub depends_on_title: Option<String>,
    pub depends_on_status: Option<String>,
    pub satisfied: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImprovementContext {
    pub improvement_id: Uuid,
    pub external_key: Option<String>,
    pub title: String,
    pub status: String,
    pub priority: String,
    pub why_deferred: Option<String>,
    pub required_before: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConstraintContext {
    pub text: String,
    pub source: ContextSourceRef,
}

// ============================================================
// Truncation / stats
// ============================================================

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TruncationInfo {
    pub truncated: bool,
    pub omitted_sections: Vec<ContextSection>,
    /// Section name -> number of items dropped by the budget.
    pub omitted_item_counts: BTreeMap<String, usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ContextStats {
    pub item_count: usize,
    pub estimated_bytes: usize,
    pub estimated_tokens: usize,
}

// ============================================================
// Packet
// ============================================================

/// Immutable, hashed snapshot of what `role`/`purpose` saw. Later changes to
/// Project State do not change an already-compiled packet.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContextPacket {
    /// Non-deterministic identity, excluded from the hash.
    pub packet_id: Uuid,
    pub project_id: Uuid,
    pub role: ContextRole,
    pub purpose: ContextPurpose,
    pub compiler_version: String,
    /// Deterministic hash of the structural content (excludes `packet_id`,
    /// `compiled_at`, and derived size stats).
    pub context_hash: String,

    pub project_identity: ProjectIdentityContext,
    pub charter: Option<CharterContext>,
    pub current_milestone: Option<MilestoneContext>,
    pub current_phase: Option<PhaseContext>,
    pub current_work_item: Option<WorkItemContext>,
    pub roadmap_context: Option<RoadmapContext>,
    /// Active WorkItems (ProjectManager / Integrator views).
    pub active_work_items: Vec<WorkItemContext>,
    /// Blocked WorkItems (ProjectManager / Integrator views).
    pub blocked_work_items: Vec<WorkItemContext>,

    pub relevant_decisions: Vec<DecisionContext>,
    pub relevant_risks: Vec<RiskContext>,
    pub relevant_blockers: Vec<BlockerContext>,
    pub relevant_artifacts: Vec<ArtifactContext>,
    pub recent_activity: Vec<ActivityContext>,
    pub dependencies: Vec<DependencyContext>,
    pub deferred_improvements: Vec<ImprovementContext>,
    pub constraints: Vec<ConstraintContext>,

    pub source_refs: Vec<ContextSourceRef>,
    pub estimated_size: ContextStats,
    pub truncation: TruncationInfo,
    /// Non-deterministic compile time, excluded from the hash.
    pub compiled_at: DateTime<Utc>,
}

/// Structural projection used for hashing — order is fixed by the struct, so
/// serialization is canonical.
#[derive(Serialize)]
struct ContextHashProjection<'a> {
    compiler_version: &'a str,
    project_id: Uuid,
    role: ContextRole,
    purpose: ContextPurpose,
    project_identity: &'a ProjectIdentityContext,
    charter: &'a Option<CharterContext>,
    current_milestone: &'a Option<MilestoneContext>,
    current_phase: &'a Option<PhaseContext>,
    current_work_item: &'a Option<WorkItemContext>,
    roadmap_context: &'a Option<RoadmapContext>,
    active_work_items: &'a [WorkItemContext],
    blocked_work_items: &'a [WorkItemContext],
    relevant_decisions: &'a [DecisionContext],
    relevant_risks: &'a [RiskContext],
    relevant_blockers: &'a [BlockerContext],
    relevant_artifacts: &'a [ArtifactContext],
    recent_activity: &'a [ActivityContext],
    dependencies: &'a [DependencyContext],
    deferred_improvements: &'a [ImprovementContext],
    constraints: &'a [ConstraintContext],
    source_refs: &'a [ContextSourceRef],
    truncation: &'a TruncationInfo,
}

impl ContextPacket {
    /// Deterministic sha256 over the structural content. Same Project State +
    /// same request + same compiler version => same hash.
    pub fn compute_context_hash(&self) -> String {
        let projection = ContextHashProjection {
            compiler_version: &self.compiler_version,
            project_id: self.project_id,
            role: self.role,
            purpose: self.purpose,
            project_identity: &self.project_identity,
            charter: &self.charter,
            current_milestone: &self.current_milestone,
            current_phase: &self.current_phase,
            current_work_item: &self.current_work_item,
            roadmap_context: &self.roadmap_context,
            active_work_items: &self.active_work_items,
            blocked_work_items: &self.blocked_work_items,
            relevant_decisions: &self.relevant_decisions,
            relevant_risks: &self.relevant_risks,
            relevant_blockers: &self.relevant_blockers,
            relevant_artifacts: &self.relevant_artifacts,
            recent_activity: &self.recent_activity,
            dependencies: &self.dependencies,
            deferred_improvements: &self.deferred_improvements,
            constraints: &self.constraints,
            source_refs: &self.source_refs,
            truncation: &self.truncation,
        };
        let bytes = serde_json::to_vec(&projection).unwrap_or_default();
        let digest = Sha256::digest(&bytes);
        format!("sha256:{}", hex::encode(digest))
    }

    /// Deterministic item count across every section.
    pub fn item_count(&self) -> usize {
        let mut n = 1; // project identity
        n += self.charter.iter().count();
        n += self.current_milestone.iter().count();
        n += self.current_phase.iter().count();
        n += self.current_work_item.iter().count();
        n += self
            .roadmap_context
            .as_ref()
            .map(|r| r.milestones.len())
            .unwrap_or(0);
        n += self.active_work_items.len();
        n += self.blocked_work_items.len();
        n += self.relevant_decisions.len();
        n += self.relevant_risks.len();
        n += self.relevant_blockers.len();
        n += self.relevant_artifacts.len();
        n += self.recent_activity.len();
        n += self.dependencies.len();
        n += self.deferred_improvements.len();
        n += self.constraints.len();
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_and_purpose_round_trip() {
        for r in [
            ContextRole::Chief,
            ContextRole::ProjectManager,
            ContextRole::Developer,
            ContextRole::Tester,
            ContextRole::Reviewer,
            ContextRole::Researcher,
            ContextRole::Integrator,
        ] {
            assert_eq!(ContextRole::parse(r.as_str()), Some(r));
        }
        for p in [
            ContextPurpose::Planning,
            ContextPurpose::Execution,
            ContextPurpose::Review,
            ContextPurpose::Debugging,
            ContextPurpose::Research,
            ContextPurpose::Integration,
            ContextPurpose::StatusReview,
        ] {
            assert_eq!(ContextPurpose::parse(p.as_str()), Some(p));
        }
        assert_eq!(ContextRole::parse("claude"), None);
        assert_eq!(ContextSourceType::Artifact.as_str(), "artifact");
        assert_eq!(
            ContextSection::parse("current_work_item"),
            Some(ContextSection::CurrentWorkItem)
        );
    }

    fn empty_packet() -> ContextPacket {
        let now = Utc::now();
        ContextPacket {
            packet_id: Uuid::new_v4(),
            project_id: Uuid::nil(),
            role: ContextRole::Developer,
            purpose: ContextPurpose::Execution,
            compiler_version: CONTEXT_COMPILER_VERSION.to_string(),
            context_hash: String::new(),
            project_identity: ProjectIdentityContext {
                project_id: Uuid::nil(),
                name: "ARGUS".to_string(),
                slug: "argus".to_string(),
                description: String::new(),
                status: "active".to_string(),
                priority: Priority::Normal,
                created_at: now,
                updated_at: now,
            },
            charter: None,
            current_milestone: None,
            current_phase: None,
            current_work_item: None,
            roadmap_context: None,
            active_work_items: Vec::new(),
            blocked_work_items: Vec::new(),
            relevant_decisions: Vec::new(),
            relevant_risks: Vec::new(),
            relevant_blockers: Vec::new(),
            relevant_artifacts: Vec::new(),
            recent_activity: Vec::new(),
            dependencies: Vec::new(),
            deferred_improvements: Vec::new(),
            constraints: Vec::new(),
            source_refs: Vec::new(),
            estimated_size: ContextStats::default(),
            truncation: TruncationInfo::default(),
            compiled_at: now,
        }
    }

    #[test]
    fn hash_is_stable_and_ignores_identity_and_time() {
        let mut a = empty_packet();
        a.context_hash = a.compute_context_hash();

        let mut b = a.clone();
        b.packet_id = Uuid::new_v4();
        b.compiled_at = a.compiled_at + chrono::Duration::seconds(5);
        b.context_hash = b.compute_context_hash();

        assert_eq!(
            a.context_hash, b.context_hash,
            "identity/time are not hashed"
        );
        assert_ne!(a.packet_id, b.packet_id);

        // A content change changes the hash.
        b.project_identity.description = "changed".to_string();
        assert_ne!(a.context_hash, b.compute_context_hash());
    }

    #[test]
    fn markdown_render_is_deterministic_and_hash_pure() {
        let mut p = empty_packet();
        p.context_hash = p.compute_context_hash();
        let m1 = crate::context::render::render_markdown(&p);
        let m2 = crate::context::render::render_markdown(&p);
        assert_eq!(m1, m2);
        assert!(m1.contains("# ARGUS Context Packet"));
        assert!(m1.contains("Project: ARGUS"));
        assert!(m1.contains("Role: developer"));

        let mut q = p.clone();
        q.packet_id = Uuid::new_v4();
        q.compiled_at = p.compiled_at + chrono::Duration::seconds(1);
        q.context_hash = q.compute_context_hash();
        assert_eq!(
            m1,
            crate::context::render::render_markdown(&q),
            "markdown must not depend on packet identity or compile time"
        );
    }
}
