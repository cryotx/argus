//! V0.39 Project State persistence.
//!
//! Deliberately **one** repository trait ([`ProjectStateRepository`]) rather
//! than one per entity: the tables form a single aggregate (a project's
//! state) and a planner/API reads them together. It follows the same shape
//! as `WorkspaceRepository`/`ArtifactRepository` (native `Uuid`/`DateTime`
//! binding, `query_as` tuples, `ON CONFLICT` for idempotency).

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value as JsonValue;
use sqlx::PgPool;
use uuid::Uuid;

use crate::task_types::Priority;
use crate::types::{ArgusError, ArgusResult};

use super::types::*;

// ============================================================
// Row mapping helpers
// ============================================================

/// Parse a priority from either representation: the `task_priority`
/// PostgreSQL enum (`critical`/`high`/`medium`/`low`/`background`, used by
/// `projects.priority`) or the Rust `Priority` snake_case rendering (`normal`,
/// used by `work_items.priority`, a TEXT column).
fn parse_priority(s: &str) -> Priority {
    match s {
        "critical" => Priority::Critical,
        "urgent" => Priority::Urgent,
        "high" => Priority::High,
        "normal" | "medium" => Priority::Normal,
        "low" => Priority::Low,
        "background" => Priority::Background,
        _ => Priority::Normal,
    }
}

/// Map `Priority` onto the `task_priority` PostgreSQL enum. Mirrors
/// `task_repository::database_priority` — the enum has no `urgent`.
fn db_priority(p: Priority) -> &'static str {
    match p {
        Priority::Critical => "critical",
        Priority::Urgent | Priority::High => "high",
        Priority::Normal => "medium",
        Priority::Low => "low",
        Priority::Background => "background",
    }
}

fn priority_str(p: Priority) -> &'static str {
    match p {
        Priority::Critical => "critical",
        Priority::Urgent => "urgent",
        Priority::High => "high",
        Priority::Normal => "normal",
        Priority::Low => "low",
        Priority::Background => "background",
    }
}

fn strings_from_json(v: JsonValue) -> Vec<String> {
    match v {
        JsonValue::Array(items) => items
            .into_iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

fn strings_to_json(v: &[String]) -> JsonValue {
    JsonValue::Array(v.iter().map(|s| JsonValue::String(s.clone())).collect())
}

// ============================================================
// Trait
// ============================================================

#[async_trait]
pub trait ProjectStateRepository: Send + Sync {
    // --- Project ---
    async fn ensure_project(&self, project: NewProject) -> ArgusResult<ProjectRecord>;
    async fn get_project(&self, project_id: Uuid) -> ArgusResult<Option<ProjectRecord>>;
    async fn get_project_by_slug(&self, slug: &str) -> ArgusResult<Option<ProjectRecord>>;
    async fn list_projects(&self) -> ArgusResult<Vec<ProjectRecord>>;
    async fn set_project_status(&self, project_id: Uuid, status: ProjectStatus) -> ArgusResult<()>;

    // --- Charter ---
    async fn save_charter(&self, charter: NewCharter) -> ArgusResult<CharterRecord>;
    async fn get_charter(&self, project_id: Uuid) -> ArgusResult<Option<CharterRecord>>;

    // --- Milestone ---
    async fn create_milestone(&self, milestone: NewMilestone) -> ArgusResult<MilestoneRecord>;
    async fn get_milestone(&self, milestone_id: Uuid) -> ArgusResult<Option<MilestoneRecord>>;
    async fn list_milestones(&self, project_id: Uuid) -> ArgusResult<Vec<MilestoneRecord>>;
    async fn set_milestone_status(
        &self,
        milestone_id: Uuid,
        status: MilestoneStatus,
    ) -> ArgusResult<()>;

    // --- Phase (V0.40) ---
    async fn create_phase(&self, phase: NewPhase) -> ArgusResult<PhaseRecord>;
    async fn get_phase(&self, phase_id: Uuid) -> ArgusResult<Option<PhaseRecord>>;
    async fn list_phases(&self, project_id: Uuid) -> ArgusResult<Vec<PhaseRecord>>;
    async fn set_phase_status(&self, phase_id: Uuid, status: MilestoneStatus) -> ArgusResult<()>;

    // --- WorkItem ---
    async fn create_work_item(&self, work_item: NewWorkItem) -> ArgusResult<WorkItemRecord>;
    async fn get_work_item(&self, work_item_id: Uuid) -> ArgusResult<Option<WorkItemRecord>>;
    /// The WorkItem a Task is linked to (0..1), if any.
    async fn get_work_item_by_task_id(&self, task_id: Uuid) -> ArgusResult<Option<WorkItemRecord>>;
    async fn list_work_items(&self, project_id: Uuid) -> ArgusResult<Vec<WorkItemRecord>>;
    async fn set_work_item_status(
        &self,
        work_item_id: Uuid,
        status: WorkItemStatus,
    ) -> ArgusResult<()>;
    async fn link_work_item_task(&self, work_item_id: Uuid, task_id: Uuid) -> ArgusResult<()>;
    async fn list_work_item_tasks(&self, work_item_id: Uuid) -> ArgusResult<Vec<Uuid>>;
    async fn add_work_item_evidence(
        &self,
        evidence: NewWorkItemEvidence,
    ) -> ArgusResult<WorkItemEvidenceRecord>;
    async fn list_work_item_evidence(
        &self,
        work_item_id: Uuid,
    ) -> ArgusResult<Vec<WorkItemEvidenceRecord>>;

    // --- Roadmap dependencies (V0.40) ---
    async fn add_work_item_dependency(
        &self,
        work_item_id: Uuid,
        depends_on_id: Uuid,
    ) -> ArgusResult<()>;
    async fn list_work_item_dependencies(
        &self,
        project_id: Uuid,
    ) -> ArgusResult<Vec<WorkItemDependencyRecord>>;

    // --- Decision ---
    async fn record_decision(&self, decision: NewDecision) -> ArgusResult<DecisionRecord>;
    async fn get_decision(&self, decision_id: Uuid) -> ArgusResult<Option<DecisionRecord>>;
    async fn list_decisions(&self, project_id: Uuid) -> ArgusResult<Vec<DecisionRecord>>;
    async fn supersede_decision(&self, old_id: Uuid, new_id: Uuid) -> ArgusResult<()>;

    // --- Risk ---
    async fn create_risk(&self, risk: NewRisk) -> ArgusResult<RiskRecord>;
    async fn list_risks(&self, project_id: Uuid) -> ArgusResult<Vec<RiskRecord>>;
    async fn set_risk_status(&self, risk_id: Uuid, status: RiskStatus) -> ArgusResult<()>;

    // --- Blocker ---
    async fn create_blocker(&self, blocker: NewBlocker) -> ArgusResult<BlockerRecord>;
    async fn list_blockers(&self, project_id: Uuid) -> ArgusResult<Vec<BlockerRecord>>;
    async fn resolve_blocker(&self, blocker_id: Uuid) -> ArgusResult<()>;

    // --- Deferred Improvement ---
    async fn create_improvement(
        &self,
        improvement: NewImprovement,
    ) -> ArgusResult<ImprovementRecord>;
    async fn list_improvements(&self, project_id: Uuid) -> ArgusResult<Vec<ImprovementRecord>>;
    async fn set_improvement_status(
        &self,
        improvement_id: Uuid,
        status: ImprovementStatus,
    ) -> ArgusResult<()>;

    // --- Activity ---
    async fn record_activity(&self, activity: NewActivity) -> ArgusResult<ActivityRecord>;
    async fn list_activity(&self, project_id: Uuid, limit: i64)
    -> ArgusResult<Vec<ActivityRecord>>;

    // --- V0.38 Artifact cross-read (source of truth, not duplicated) ---
    async fn recent_artifacts(
        &self,
        project_id: Uuid,
        limit: i64,
    ) -> ArgusResult<Vec<ArtifactSummary>>;
    async fn get_artifact(&self, artifact_id: Uuid) -> ArgusResult<Option<ArtifactSummary>>;
}

// ============================================================
// PostgreSQL implementation
// ============================================================

pub struct PostgresProjectStateRepository {
    pool: PgPool,
}

impl PostgresProjectStateRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

type ProjectRow = (
    Uuid,
    String,
    String,
    Option<String>,
    String,
    String,
    Option<String>,
    DateTime<Utc>,
    DateTime<Utc>,
);

fn project_from_row(r: ProjectRow) -> ProjectRecord {
    ProjectRecord {
        project_id: r.0,
        name: r.1,
        slug: r.2,
        description: r.3.unwrap_or_default(),
        status: ProjectStatus::parse(&r.4).unwrap_or(ProjectStatus::Active),
        priority: parse_priority(&r.5),
        git_repo_url: r.6,
        created_at: r.7,
        updated_at: r.8,
    }
}

const PROJECT_COLUMNS: &str =
    "id, name, slug, description, status, priority::text, git_repo_url, created_at, updated_at";

type CharterRow = (
    Uuid,
    String,
    String,
    JsonValue,
    JsonValue,
    JsonValue,
    i32,
    DateTime<Utc>,
    DateTime<Utc>,
);

fn charter_from_row(r: CharterRow) -> CharterRecord {
    CharterRecord {
        project_id: r.0,
        vision: r.1,
        goal: r.2,
        success_criteria: strings_from_json(r.3),
        constraints: strings_from_json(r.4),
        non_goals: strings_from_json(r.5),
        revision: r.6,
        created_at: r.7,
        updated_at: r.8,
    }
}

type MilestoneRow = (
    Uuid,
    Uuid,
    String,
    Option<String>,
    String,
    i32,
    Option<Uuid>,
    Option<String>,
    Option<String>,
    i32,
    Option<DateTime<Utc>>,
    Option<DateTime<Utc>>,
    DateTime<Utc>,
    DateTime<Utc>,
);

fn milestone_from_row(r: MilestoneRow) -> MilestoneRecord {
    MilestoneRecord {
        milestone_id: r.0,
        project_id: r.1,
        title: r.2,
        description: r.3,
        status: MilestoneStatus::parse(&r.4).unwrap_or(MilestoneStatus::Planned),
        sequence: r.5,
        parent_id: r.6,
        definition_of_done: r.7,
        external_key: r.8,
        weight: r.9,
        started_at: r.10,
        completed_at: r.11,
        created_at: r.12,
        updated_at: r.13,
    }
}

const MILESTONE_COLUMNS: &str = "id, project_id, name, description, status, sequence, \
    parent_id, definition_of_done, external_key, weight, started_at, completed_at, created_at, updated_at";

type PhaseRow = (
    Uuid,
    Uuid,
    Uuid,
    String,
    Option<String>,
    String,
    i32,
    Option<String>,
    Option<String>,
    i32,
    Option<DateTime<Utc>>,
    Option<DateTime<Utc>>,
    DateTime<Utc>,
    DateTime<Utc>,
);

fn phase_from_row(r: PhaseRow) -> PhaseRecord {
    PhaseRecord {
        phase_id: r.0,
        project_id: r.1,
        milestone_id: r.2,
        title: r.3,
        description: r.4,
        status: MilestoneStatus::parse(&r.5).unwrap_or(MilestoneStatus::Planned),
        sequence: r.6,
        external_key: r.7,
        definition_of_done: r.8,
        weight: r.9,
        started_at: r.10,
        completed_at: r.11,
        created_at: r.12,
        updated_at: r.13,
    }
}

const PHASE_COLUMNS: &str = "phase_id, project_id, milestone_id, title, description, status, \
    sequence, external_key, definition_of_done, weight, started_at, completed_at, created_at, updated_at";

type WorkItemRow = (
    Uuid,
    Uuid,
    Option<Uuid>,
    Option<Uuid>,
    String,
    Option<String>,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    i32,
    DateTime<Utc>,
    DateTime<Utc>,
    Option<DateTime<Utc>>,
);

fn work_item_from_row(r: WorkItemRow) -> WorkItemRecord {
    WorkItemRecord {
        work_item_id: r.0,
        project_id: r.1,
        milestone_id: r.2,
        phase_id: r.3,
        title: r.4,
        description: r.5.unwrap_or_default(),
        status: WorkItemStatus::parse(&r.6).unwrap_or(WorkItemStatus::Planned),
        priority: parse_priority(&r.7),
        definition_of_done: r.8,
        source: r.9,
        external_key: r.10,
        weight: r.11,
        created_at: r.12,
        updated_at: r.13,
        completed_at: r.14,
    }
}

const WORK_ITEM_COLUMNS: &str = "work_item_id, project_id, milestone_id, phase_id, title, description, \
    status, priority, definition_of_done, source, external_key, weight, created_at, updated_at, completed_at";

type EvidenceRow = (
    Uuid,
    Uuid,
    String,
    Option<Uuid>,
    Option<Uuid>,
    Option<Uuid>,
    Option<String>,
    DateTime<Utc>,
);

fn evidence_from_row(r: EvidenceRow) -> WorkItemEvidenceRecord {
    WorkItemEvidenceRecord {
        evidence_id: r.0,
        work_item_id: r.1,
        evidence_type: r.2,
        artifact_id: r.3,
        task_id: r.4,
        execution_id: r.5,
        note: r.6,
        created_at: r.7,
    }
}

const EVIDENCE_COLUMNS: &str = "evidence_id, work_item_id, evidence_type, artifact_id, task_id, execution_id, note, created_at";

type DecisionRow = (
    Uuid,
    Uuid,
    String,
    Option<String>,
    String,
    Option<String>,
    String,
    Option<String>,
    Option<Uuid>,
    DateTime<Utc>,
);

fn decision_from_row(r: DecisionRow) -> DecisionRecord {
    DecisionRecord {
        decision_id: r.0,
        project_id: r.1,
        title: r.2,
        context: r.3,
        decision: r.4,
        reason: r.5,
        status: DecisionStatus::parse(&r.6).unwrap_or(DecisionStatus::Proposed),
        source: r.7,
        superseded_by: r.8,
        created_at: r.9,
    }
}

const DECISION_COLUMNS: &str = "decision_id, project_id, title, context, decision, reason, \
    status, source, superseded_by, created_at";

type RiskRow = (
    Uuid,
    Uuid,
    Option<Uuid>,
    String,
    Option<String>,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    DateTime<Utc>,
    DateTime<Utc>,
    Option<DateTime<Utc>>,
);

fn risk_from_row(r: RiskRow) -> RiskRecord {
    RiskRecord {
        risk_id: r.0,
        project_id: r.1,
        milestone_id: r.2,
        title: r.3,
        description: r.4,
        severity: RiskSeverity::parse(&r.5).unwrap_or(RiskSeverity::Medium),
        likelihood: RiskLikelihood::parse(&r.6).unwrap_or(RiskLikelihood::Medium),
        status: RiskStatus::parse(&r.7).unwrap_or(RiskStatus::Open),
        mitigation: r.8,
        owner: r.9,
        created_at: r.10,
        updated_at: r.11,
        resolved_at: r.12,
    }
}

const RISK_COLUMNS: &str = "risk_id, project_id, milestone_id, title, description, severity, \
    likelihood, status, mitigation, owner, created_at, updated_at, resolved_at";

type BlockerRow = (
    Uuid,
    Uuid,
    Option<Uuid>,
    String,
    Option<String>,
    String,
    DateTime<Utc>,
    Option<DateTime<Utc>>,
);

fn blocker_from_row(r: BlockerRow) -> BlockerRecord {
    BlockerRecord {
        blocker_id: r.0,
        project_id: r.1,
        work_item_id: r.2,
        title: r.3,
        description: r.4,
        status: BlockerStatus::parse(&r.5).unwrap_or(BlockerStatus::Open),
        created_at: r.6,
        resolved_at: r.7,
    }
}

const BLOCKER_COLUMNS: &str =
    "blocker_id, project_id, work_item_id, title, description, status, created_at, resolved_at";

type ImprovementRow = (
    Uuid,
    Uuid,
    Option<String>,
    String,
    Option<String>,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    DateTime<Utc>,
    DateTime<Utc>,
);

fn improvement_from_row(r: ImprovementRow) -> ImprovementRecord {
    ImprovementRecord {
        improvement_id: r.0,
        project_id: r.1,
        external_key: r.2,
        title: r.3,
        description: r.4,
        priority: ImprovementPriority::parse(&r.5).unwrap_or(ImprovementPriority::Medium),
        status: ImprovementStatus::parse(&r.6).unwrap_or(ImprovementStatus::Deferred),
        source: r.7,
        why_deferred: r.8,
        required_before: r.9,
        created_at: r.10,
        updated_at: r.11,
    }
}

const IMPROVEMENT_COLUMNS: &str = "improvement_id, project_id, external_key, title, description, \
    priority, status, source, why_deferred, required_before, created_at, updated_at";

type ActivityRow = (
    Uuid,
    Uuid,
    String,
    String,
    Option<Uuid>,
    String,
    DateTime<Utc>,
    JsonValue,
);

fn activity_from_row(r: ActivityRow) -> ActivityRecord {
    ActivityRecord {
        activity_id: r.0,
        project_id: r.1,
        activity_type: ActivityType::parse(&r.2).unwrap_or(ActivityType::ProjectCreated),
        entity_type: r.3,
        entity_id: r.4,
        summary: r.5,
        occurred_at: r.6,
        metadata: r.7,
    }
}

const ACTIVITY_COLUMNS: &str = "activity_id, project_id, activity_type, entity_type, entity_id, summary, occurred_at, metadata";

#[async_trait]
impl ProjectStateRepository for PostgresProjectStateRepository {
    async fn ensure_project(&self, project: NewProject) -> ArgusResult<ProjectRecord> {
        let query = format!(
            r#"
            INSERT INTO projects (name, slug, description, git_local_path, active, priority, status)
            VALUES ($1, $2, $3, $4, true, $5::task_priority, 'active')
            ON CONFLICT (slug) DO UPDATE SET updated_at = NOW()
            RETURNING {PROJECT_COLUMNS}
            "#
        );
        let row = sqlx::query_as::<_, ProjectRow>(&query)
            .bind(&project.name)
            .bind(&project.slug)
            .bind(&project.description)
            .bind(format!("/tmp/argus/projects/{}", project.slug))
            .bind(db_priority(project.priority))
            .fetch_one(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(project_from_row(row))
    }

    async fn get_project(&self, project_id: Uuid) -> ArgusResult<Option<ProjectRecord>> {
        let query = format!("SELECT {PROJECT_COLUMNS} FROM projects WHERE id = $1");
        let row = sqlx::query_as::<_, ProjectRow>(&query)
            .bind(project_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(row.map(project_from_row))
    }

    async fn get_project_by_slug(&self, slug: &str) -> ArgusResult<Option<ProjectRecord>> {
        let query = format!("SELECT {PROJECT_COLUMNS} FROM projects WHERE slug = $1");
        let row = sqlx::query_as::<_, ProjectRow>(&query)
            .bind(slug)
            .fetch_optional(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(row.map(project_from_row))
    }

    async fn list_projects(&self) -> ArgusResult<Vec<ProjectRecord>> {
        let query = format!("SELECT {PROJECT_COLUMNS} FROM projects ORDER BY created_at ASC");
        let rows = sqlx::query_as::<_, ProjectRow>(&query)
            .fetch_all(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(rows.into_iter().map(project_from_row).collect())
    }

    async fn set_project_status(&self, project_id: Uuid, status: ProjectStatus) -> ArgusResult<()> {
        sqlx::query("UPDATE projects SET status = $2, active = ($2 = 'active') WHERE id = $1")
            .bind(project_id)
            .bind(status.as_str())
            .execute(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(())
    }

    async fn save_charter(&self, charter: NewCharter) -> ArgusResult<CharterRecord> {
        // Revision is bumped only when content actually differs — a no-op save
        // does not create a new revision.
        let query = r#"
            INSERT INTO project_charters (
                project_id, vision, goal, success_criteria, constraints, non_goals, revision, created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, 1, NOW(), NOW())
            ON CONFLICT (project_id) DO UPDATE SET
                vision = EXCLUDED.vision,
                goal = EXCLUDED.goal,
                success_criteria = EXCLUDED.success_criteria,
                constraints = EXCLUDED.constraints,
                non_goals = EXCLUDED.non_goals,
                revision = CASE
                    WHEN project_charters.vision IS DISTINCT FROM EXCLUDED.vision
                      OR project_charters.goal IS DISTINCT FROM EXCLUDED.goal
                      OR project_charters.success_criteria IS DISTINCT FROM EXCLUDED.success_criteria
                      OR project_charters.constraints IS DISTINCT FROM EXCLUDED.constraints
                      OR project_charters.non_goals IS DISTINCT FROM EXCLUDED.non_goals
                    THEN project_charters.revision + 1
                    ELSE project_charters.revision
                END,
                updated_at = NOW()
            RETURNING project_id, vision, goal, success_criteria, constraints, non_goals, revision, created_at, updated_at
            "#;
        let row = sqlx::query_as::<_, CharterRow>(query)
            .bind(charter.project_id)
            .bind(&charter.vision)
            .bind(&charter.goal)
            .bind(strings_to_json(&charter.success_criteria))
            .bind(strings_to_json(&charter.constraints))
            .bind(strings_to_json(&charter.non_goals))
            .fetch_one(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(charter_from_row(row))
    }

    async fn get_charter(&self, project_id: Uuid) -> ArgusResult<Option<CharterRecord>> {
        let query = "SELECT project_id, vision, goal, success_criteria, constraints, non_goals, revision, created_at, updated_at FROM project_charters WHERE project_id = $1";
        let row = sqlx::query_as::<_, CharterRow>(query)
            .bind(project_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(row.map(charter_from_row))
    }

    async fn create_milestone(&self, milestone: NewMilestone) -> ArgusResult<MilestoneRecord> {
        let query = format!(
            r#"
            INSERT INTO milestones (
                project_id, name, description, sequence, status, parent_id, definition_of_done,
                external_key, weight, created_at
            )
            VALUES (
                $1, $2, $3,
                COALESCE($4, (SELECT COALESCE(MAX(sequence), 0) + 1 FROM milestones WHERE project_id = $1)),
                'planned', $5, $6, $7, $8, NOW()
            )
            RETURNING {MILESTONE_COLUMNS}
            "#
        );
        let row = sqlx::query_as::<_, MilestoneRow>(&query)
            .bind(milestone.project_id)
            .bind(&milestone.title)
            .bind(&milestone.description)
            .bind(milestone.sequence)
            .bind(milestone.parent_id)
            .bind(&milestone.definition_of_done)
            .bind(&milestone.external_key)
            .bind(milestone.weight)
            .fetch_one(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(milestone_from_row(row))
    }

    async fn create_phase(&self, phase: NewPhase) -> ArgusResult<PhaseRecord> {
        let query = format!(
            r#"
            INSERT INTO phases (
                project_id, milestone_id, title, description, sequence, status,
                external_key, definition_of_done, weight, created_at
            )
            VALUES (
                $1, $2, $3, $4,
                COALESCE($5, (SELECT COALESCE(MAX(sequence), 0) + 1 FROM phases WHERE milestone_id = $2)),
                'planned', $6, $7, $8, NOW()
            )
            RETURNING {PHASE_COLUMNS}
            "#
        );
        let row = sqlx::query_as::<_, PhaseRow>(&query)
            .bind(phase.project_id)
            .bind(phase.milestone_id)
            .bind(&phase.title)
            .bind(&phase.description)
            .bind(phase.sequence)
            .bind(&phase.external_key)
            .bind(&phase.definition_of_done)
            .bind(phase.weight)
            .fetch_one(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(phase_from_row(row))
    }

    async fn get_phase(&self, phase_id: Uuid) -> ArgusResult<Option<PhaseRecord>> {
        let query = format!("SELECT {PHASE_COLUMNS} FROM phases WHERE phase_id = $1");
        let row = sqlx::query_as::<_, PhaseRow>(&query)
            .bind(phase_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(row.map(phase_from_row))
    }

    async fn list_phases(&self, project_id: Uuid) -> ArgusResult<Vec<PhaseRecord>> {
        let query = format!(
            "SELECT {PHASE_COLUMNS} FROM phases WHERE project_id = $1 ORDER BY sequence ASC"
        );
        let rows = sqlx::query_as::<_, PhaseRow>(&query)
            .bind(project_id)
            .fetch_all(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(rows.into_iter().map(phase_from_row).collect())
    }

    async fn set_phase_status(&self, phase_id: Uuid, status: MilestoneStatus) -> ArgusResult<()> {
        sqlx::query(
            r#"
            UPDATE phases SET
                status = $2,
                started_at = CASE WHEN $2 = 'active' AND started_at IS NULL THEN NOW() ELSE started_at END,
                completed_at = CASE
                    WHEN $2 IN ('completed', 'cancelled') THEN COALESCE(completed_at, NOW())
                    ELSE completed_at
                END
            WHERE phase_id = $1
            "#,
        )
        .bind(phase_id)
        .bind(status.as_str())
        .execute(&self.pool)
        .await
        .map_err(ArgusError::Database)?;
        Ok(())
    }

    async fn get_milestone(&self, milestone_id: Uuid) -> ArgusResult<Option<MilestoneRecord>> {
        let query = format!("SELECT {MILESTONE_COLUMNS} FROM milestones WHERE id = $1");
        let row = sqlx::query_as::<_, MilestoneRow>(&query)
            .bind(milestone_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(row.map(milestone_from_row))
    }

    async fn list_milestones(&self, project_id: Uuid) -> ArgusResult<Vec<MilestoneRecord>> {
        let query = format!(
            "SELECT {MILESTONE_COLUMNS} FROM milestones WHERE project_id = $1 ORDER BY sequence ASC"
        );
        let rows = sqlx::query_as::<_, MilestoneRow>(&query)
            .bind(project_id)
            .fetch_all(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(rows.into_iter().map(milestone_from_row).collect())
    }

    async fn set_milestone_status(
        &self,
        milestone_id: Uuid,
        status: MilestoneStatus,
    ) -> ArgusResult<()> {
        sqlx::query(
            r#"
            UPDATE milestones SET
                status = $2,
                started_at = CASE WHEN $2 = 'active' AND started_at IS NULL THEN NOW() ELSE started_at END,
                completed_at = CASE
                    WHEN $2 IN ('completed', 'cancelled') THEN COALESCE(completed_at, NOW())
                    ELSE completed_at
                END
            WHERE id = $1
            "#,
        )
        .bind(milestone_id)
        .bind(status.as_str())
        .execute(&self.pool)
        .await
        .map_err(ArgusError::Database)?;
        Ok(())
    }

    async fn create_work_item(&self, work_item: NewWorkItem) -> ArgusResult<WorkItemRecord> {
        let query = format!(
            r#"
            INSERT INTO work_items (
                project_id, milestone_id, phase_id, title, description, status, priority,
                definition_of_done, source, external_key, weight, created_at
            )
            VALUES ($1, $2, $3, $4, $5, 'planned', $6, $7, $8, $9, $10, NOW())
            RETURNING {WORK_ITEM_COLUMNS}
            "#
        );
        let row = sqlx::query_as::<_, WorkItemRow>(&query)
            .bind(work_item.project_id)
            .bind(work_item.milestone_id)
            .bind(work_item.phase_id)
            .bind(&work_item.title)
            .bind(&work_item.description)
            .bind(priority_str(work_item.priority))
            .bind(&work_item.definition_of_done)
            .bind(&work_item.source)
            .bind(&work_item.external_key)
            .bind(work_item.weight)
            .fetch_one(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(work_item_from_row(row))
    }

    async fn get_work_item(&self, work_item_id: Uuid) -> ArgusResult<Option<WorkItemRecord>> {
        let query = format!("SELECT {WORK_ITEM_COLUMNS} FROM work_items WHERE work_item_id = $1");
        let row = sqlx::query_as::<_, WorkItemRow>(&query)
            .bind(work_item_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(row.map(work_item_from_row))
    }

    async fn get_work_item_by_task_id(&self, task_id: Uuid) -> ArgusResult<Option<WorkItemRecord>> {
        let row: Option<(Uuid,)> =
            sqlx::query_as("SELECT work_item_id FROM work_item_tasks WHERE task_id = $1 LIMIT 1")
                .bind(task_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(ArgusError::Database)?;
        match row {
            Some((work_item_id,)) => self.get_work_item(work_item_id).await,
            None => Ok(None),
        }
    }

    async fn list_work_items(&self, project_id: Uuid) -> ArgusResult<Vec<WorkItemRecord>> {
        let query = format!(
            "SELECT {WORK_ITEM_COLUMNS} FROM work_items WHERE project_id = $1 \
             ORDER BY created_at ASC"
        );
        let rows = sqlx::query_as::<_, WorkItemRow>(&query)
            .bind(project_id)
            .fetch_all(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(rows.into_iter().map(work_item_from_row).collect())
    }

    async fn set_work_item_status(
        &self,
        work_item_id: Uuid,
        status: WorkItemStatus,
    ) -> ArgusResult<()> {
        sqlx::query(
            r#"
            UPDATE work_items SET
                status = $2,
                completed_at = CASE
                    WHEN $2 IN ('verified_complete', 'accepted', 'cancelled')
                        THEN COALESCE(completed_at, NOW())
                    ELSE completed_at
                END
            WHERE work_item_id = $1
            "#,
        )
        .bind(work_item_id)
        .bind(status.as_str())
        .execute(&self.pool)
        .await
        .map_err(ArgusError::Database)?;
        Ok(())
    }

    async fn link_work_item_task(&self, work_item_id: Uuid, task_id: Uuid) -> ArgusResult<()> {
        let mut tx = self.pool.begin().await?;
        let (project_id,): (Uuid,) =
            sqlx::query_as("SELECT project_id FROM work_items WHERE work_item_id=$1")
                .bind(work_item_id)
                .fetch_one(&mut *tx)
                .await?;
        sqlx::query(
            "INSERT INTO project_revisions (project_id) VALUES ($1) ON CONFLICT DO NOTHING",
        )
        .bind(project_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("SELECT project_id FROM project_revisions WHERE project_id=$1 FOR UPDATE")
            .bind(project_id)
            .fetch_one(&mut *tx)
            .await?;
        let item: (Uuid, String) = sqlx::query_as(
            "SELECT project_id,status FROM work_items WHERE work_item_id=$1 FOR UPDATE",
        )
        .bind(work_item_id)
        .fetch_one(&mut *tx)
        .await?;
        let task: (Uuid,) = sqlx::query_as("SELECT project_id FROM tasks WHERE id=$1")
            .bind(task_id)
            .fetch_one(&mut *tx)
            .await?;
        if item.0 != task.0 {
            return Err(ArgusError::Validation(
                "Cannot link tasks from another project".into(),
            ));
        }
        let inserted = sqlx::query(
            "INSERT INTO work_item_tasks (work_item_id, task_id) VALUES ($1, $2) \
             ON CONFLICT DO NOTHING",
        )
        .bind(work_item_id)
        .bind(task_id)
        .execute(&mut *tx)
        .await
        .map_err(ArgusError::Database)?
        .rows_affected();
        if inserted != 0 && matches!(item.1.as_str(), "verified_complete" | "accepted") {
            sqlx::query(
                "UPDATE work_items SET status='active', completed_at=NULL WHERE work_item_id=$1",
            )
            .bind(work_item_id)
            .execute(&mut *tx)
            .await?;
        }
        if inserted != 0 {
            sqlx::query(
                "UPDATE project_revisions SET state_revision=state_revision+1 WHERE project_id=$1",
            )
            .bind(item.0)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn list_work_item_tasks(&self, work_item_id: Uuid) -> ArgusResult<Vec<Uuid>> {
        let rows: Vec<(Uuid,)> = sqlx::query_as(
            "SELECT task_id FROM work_item_tasks WHERE work_item_id = $1 ORDER BY created_at ASC",
        )
        .bind(work_item_id)
        .fetch_all(&self.pool)
        .await
        .map_err(ArgusError::Database)?;
        Ok(rows.into_iter().map(|r| r.0).collect())
    }

    async fn add_work_item_evidence(
        &self,
        evidence: NewWorkItemEvidence,
    ) -> ArgusResult<WorkItemEvidenceRecord> {
        let query = format!(
            r#"
            INSERT INTO work_item_evidence (
                evidence_id, work_item_id, evidence_type, artifact_id, task_id, execution_id, note, created_at
            )
            VALUES (gen_random_uuid(), $1, $2, $3, $4, $5, $6, NOW())
            RETURNING {EVIDENCE_COLUMNS}
            "#
        );
        let row = sqlx::query_as::<_, EvidenceRow>(&query)
            .bind(evidence.work_item_id)
            .bind(&evidence.evidence_type)
            .bind(evidence.artifact_id)
            .bind(evidence.task_id)
            .bind(evidence.execution_id)
            .bind(&evidence.note)
            .fetch_one(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(evidence_from_row(row))
    }

    async fn list_work_item_evidence(
        &self,
        work_item_id: Uuid,
    ) -> ArgusResult<Vec<WorkItemEvidenceRecord>> {
        let query = format!(
            "SELECT {EVIDENCE_COLUMNS} FROM work_item_evidence WHERE work_item_id = $1 ORDER BY created_at ASC"
        );
        let rows = sqlx::query_as::<_, EvidenceRow>(&query)
            .bind(work_item_id)
            .fetch_all(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(rows.into_iter().map(evidence_from_row).collect())
    }

    async fn add_work_item_dependency(
        &self,
        work_item_id: Uuid,
        depends_on_id: Uuid,
    ) -> ArgusResult<()> {
        if work_item_id == depends_on_id {
            return Err(ArgusError::Validation(
                "a work item cannot depend on itself".to_string(),
            ));
        }
        sqlx::query(
            "INSERT INTO work_item_dependencies (work_item_id, depends_on_id) VALUES ($1, $2) \
             ON CONFLICT DO NOTHING",
        )
        .bind(work_item_id)
        .bind(depends_on_id)
        .execute(&self.pool)
        .await
        .map_err(ArgusError::Database)?;
        Ok(())
    }

    async fn list_work_item_dependencies(
        &self,
        project_id: Uuid,
    ) -> ArgusResult<Vec<WorkItemDependencyRecord>> {
        let rows: Vec<(Uuid, Uuid, DateTime<Utc>)> = sqlx::query_as(
            "SELECT d.work_item_id, d.depends_on_id, d.created_at \
             FROM work_item_dependencies d \
             JOIN work_items w ON w.work_item_id = d.work_item_id \
             WHERE w.project_id = $1 \
             ORDER BY d.created_at ASC",
        )
        .bind(project_id)
        .fetch_all(&self.pool)
        .await
        .map_err(ArgusError::Database)?;
        Ok(rows
            .into_iter()
            .map(|r| WorkItemDependencyRecord {
                work_item_id: r.0,
                depends_on_id: r.1,
                created_at: r.2,
            })
            .collect())
    }

    async fn record_decision(&self, decision: NewDecision) -> ArgusResult<DecisionRecord> {
        let query = format!(
            r#"
            INSERT INTO project_decisions (
                decision_id, project_id, title, context, decision, reason, status, source, created_at
            )
            VALUES (gen_random_uuid(), $1, $2, $3, $4, $5, $6, $7, NOW())
            RETURNING {DECISION_COLUMNS}
            "#
        );
        let row = sqlx::query_as::<_, DecisionRow>(&query)
            .bind(decision.project_id)
            .bind(&decision.title)
            .bind(&decision.context)
            .bind(&decision.decision)
            .bind(&decision.reason)
            .bind(decision.status.as_str())
            .bind(&decision.source)
            .fetch_one(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(decision_from_row(row))
    }

    async fn get_decision(&self, decision_id: Uuid) -> ArgusResult<Option<DecisionRecord>> {
        let query =
            format!("SELECT {DECISION_COLUMNS} FROM project_decisions WHERE decision_id = $1");
        let row = sqlx::query_as::<_, DecisionRow>(&query)
            .bind(decision_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(row.map(decision_from_row))
    }

    async fn list_decisions(&self, project_id: Uuid) -> ArgusResult<Vec<DecisionRecord>> {
        let query = format!(
            "SELECT {DECISION_COLUMNS} FROM project_decisions WHERE project_id = $1 ORDER BY created_at ASC"
        );
        let rows = sqlx::query_as::<_, DecisionRow>(&query)
            .bind(project_id)
            .fetch_all(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(rows.into_iter().map(decision_from_row).collect())
    }

    async fn supersede_decision(&self, old_id: Uuid, new_id: Uuid) -> ArgusResult<()> {
        sqlx::query(
            "UPDATE project_decisions SET status = 'superseded', superseded_by = $2 \
             WHERE decision_id = $1",
        )
        .bind(old_id)
        .bind(new_id)
        .execute(&self.pool)
        .await
        .map_err(ArgusError::Database)?;
        Ok(())
    }

    async fn create_risk(&self, risk: NewRisk) -> ArgusResult<RiskRecord> {
        let query = format!(
            r#"
            INSERT INTO project_risks (
                risk_id, project_id, milestone_id, title, description, severity, likelihood,
                status, mitigation, owner, created_at
            )
            VALUES (gen_random_uuid(), $1, $2, $3, $4, $5, $6, 'open', $7, $8, NOW())
            RETURNING {RISK_COLUMNS}
            "#
        );
        let row = sqlx::query_as::<_, RiskRow>(&query)
            .bind(risk.project_id)
            .bind(risk.milestone_id)
            .bind(&risk.title)
            .bind(&risk.description)
            .bind(risk.severity.as_str())
            .bind(risk.likelihood.as_str())
            .bind(&risk.mitigation)
            .bind(&risk.owner)
            .fetch_one(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(risk_from_row(row))
    }

    async fn list_risks(&self, project_id: Uuid) -> ArgusResult<Vec<RiskRecord>> {
        let query = format!(
            "SELECT {RISK_COLUMNS} FROM project_risks WHERE project_id = $1 ORDER BY created_at ASC"
        );
        let rows = sqlx::query_as::<_, RiskRow>(&query)
            .bind(project_id)
            .fetch_all(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(rows.into_iter().map(risk_from_row).collect())
    }

    async fn set_risk_status(&self, risk_id: Uuid, status: RiskStatus) -> ArgusResult<()> {
        sqlx::query(
            r#"
            UPDATE project_risks SET
                status = $2,
                resolved_at = CASE WHEN $2 = 'resolved' THEN COALESCE(resolved_at, NOW()) ELSE resolved_at END
            WHERE risk_id = $1
            "#,
        )
        .bind(risk_id)
        .bind(status.as_str())
        .execute(&self.pool)
        .await
        .map_err(ArgusError::Database)?;
        Ok(())
    }

    async fn create_blocker(&self, blocker: NewBlocker) -> ArgusResult<BlockerRecord> {
        let query = format!(
            r#"
            INSERT INTO project_blockers (
                blocker_id, project_id, work_item_id, title, description, status, created_at
            )
            VALUES (gen_random_uuid(), $1, $2, $3, $4, 'open', NOW())
            RETURNING {BLOCKER_COLUMNS}
            "#
        );
        let row = sqlx::query_as::<_, BlockerRow>(&query)
            .bind(blocker.project_id)
            .bind(blocker.work_item_id)
            .bind(&blocker.title)
            .bind(&blocker.description)
            .fetch_one(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(blocker_from_row(row))
    }

    async fn list_blockers(&self, project_id: Uuid) -> ArgusResult<Vec<BlockerRecord>> {
        let query = format!(
            "SELECT {BLOCKER_COLUMNS} FROM project_blockers WHERE project_id = $1 ORDER BY created_at ASC"
        );
        let rows = sqlx::query_as::<_, BlockerRow>(&query)
            .bind(project_id)
            .fetch_all(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(rows.into_iter().map(blocker_from_row).collect())
    }

    async fn resolve_blocker(&self, blocker_id: Uuid) -> ArgusResult<()> {
        sqlx::query(
            "UPDATE project_blockers SET status = 'resolved', resolved_at = COALESCE(resolved_at, NOW()) \
             WHERE blocker_id = $1",
        )
        .bind(blocker_id)
        .execute(&self.pool)
        .await
        .map_err(ArgusError::Database)?;
        Ok(())
    }

    async fn create_improvement(
        &self,
        improvement: NewImprovement,
    ) -> ArgusResult<ImprovementRecord> {
        let query = format!(
            r#"
            INSERT INTO deferred_improvements (
                improvement_id, project_id, external_key, title, description, priority, status,
                source, why_deferred, required_before, created_at
            )
            VALUES (gen_random_uuid(), $1, $2, $3, $4, $5, $6, $7, $8, $9, NOW())
            RETURNING {IMPROVEMENT_COLUMNS}
            "#
        );
        let row = sqlx::query_as::<_, ImprovementRow>(&query)
            .bind(improvement.project_id)
            .bind(&improvement.external_key)
            .bind(&improvement.title)
            .bind(&improvement.description)
            .bind(improvement.priority.as_str())
            .bind(improvement.status.as_str())
            .bind(&improvement.source)
            .bind(&improvement.why_deferred)
            .bind(&improvement.required_before)
            .fetch_one(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(improvement_from_row(row))
    }

    async fn list_improvements(&self, project_id: Uuid) -> ArgusResult<Vec<ImprovementRecord>> {
        let query = format!(
            "SELECT {IMPROVEMENT_COLUMNS} FROM deferred_improvements WHERE project_id = $1 ORDER BY created_at ASC"
        );
        let rows = sqlx::query_as::<_, ImprovementRow>(&query)
            .bind(project_id)
            .fetch_all(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(rows.into_iter().map(improvement_from_row).collect())
    }

    async fn set_improvement_status(
        &self,
        improvement_id: Uuid,
        status: ImprovementStatus,
    ) -> ArgusResult<()> {
        sqlx::query("UPDATE deferred_improvements SET status = $2 WHERE improvement_id = $1")
            .bind(improvement_id)
            .bind(status.as_str())
            .execute(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(())
    }

    async fn record_activity(&self, activity: NewActivity) -> ArgusResult<ActivityRecord> {
        let query = format!(
            r#"
            INSERT INTO project_activities (
                activity_id, project_id, activity_type, entity_type, entity_id, summary, occurred_at, metadata
            )
            VALUES (gen_random_uuid(), $1, $2, $3, $4, $5, NOW(), $6)
            RETURNING {ACTIVITY_COLUMNS}
            "#
        );
        let row = sqlx::query_as::<_, ActivityRow>(&query)
            .bind(activity.project_id)
            .bind(activity.activity_type.as_str())
            .bind(&activity.entity_type)
            .bind(activity.entity_id)
            .bind(&activity.summary)
            .bind(activity.metadata)
            .fetch_one(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(activity_from_row(row))
    }

    async fn list_activity(
        &self,
        project_id: Uuid,
        limit: i64,
    ) -> ArgusResult<Vec<ActivityRecord>> {
        let query = format!(
            "SELECT {ACTIVITY_COLUMNS} FROM project_activities WHERE project_id = $1 \
             ORDER BY occurred_at DESC, activity_id DESC LIMIT $2"
        );
        let rows = sqlx::query_as::<_, ActivityRow>(&query)
            .bind(project_id)
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(rows.into_iter().map(activity_from_row).collect())
    }

    async fn recent_artifacts(
        &self,
        project_id: Uuid,
        limit: i64,
    ) -> ArgusResult<Vec<ArtifactSummary>> {
        let rows: Vec<(
            Uuid,
            Uuid,
            Uuid,
            String,
            String,
            JsonValue,
            Option<String>,
            DateTime<Utc>,
        )> = sqlx::query_as(
            "SELECT artifact_id, task_id, workspace_id, commit_sha, branch_name, files_changed, \
                 quality_status, created_at FROM task_artifacts WHERE project_id = $1 \
                 ORDER BY created_at DESC LIMIT $2",
        )
        .bind(project_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(ArgusError::Database)?;

        Ok(rows
            .into_iter()
            .map(|r| ArtifactSummary {
                artifact_id: r.0,
                task_id: r.1,
                workspace_id: r.2,
                commit_sha: r.3,
                branch_name: r.4,
                files_changed: strings_from_json(r.5),
                quality_status: r.6,
                created_at: r.7,
            })
            .collect())
    }

    async fn get_artifact(&self, artifact_id: Uuid) -> ArgusResult<Option<ArtifactSummary>> {
        let row: Option<(
            Uuid,
            Uuid,
            Uuid,
            String,
            String,
            JsonValue,
            Option<String>,
            DateTime<Utc>,
        )> = sqlx::query_as(
            "SELECT artifact_id, task_id, workspace_id, commit_sha, branch_name, files_changed, \
             quality_status, created_at FROM task_artifacts WHERE artifact_id = $1",
        )
        .bind(artifact_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(ArgusError::Database)?;
        Ok(row.map(|r| ArtifactSummary {
            artifact_id: r.0,
            task_id: r.1,
            workspace_id: r.2,
            commit_sha: r.3,
            branch_name: r.4,
            files_changed: strings_from_json(r.5),
            quality_status: r.6,
            created_at: r.7,
        }))
    }
}
