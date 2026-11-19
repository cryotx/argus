//! V0.43 persistence for the planner protocol, including the single
//! transaction that applies an approved proposal (all-or-nothing) and the
//! per-project optimistic-concurrency revisions.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value as JsonValue;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::project_state::{ImprovementPriority, MilestoneStatus};
use crate::task_types::Priority;
use crate::types::{ArgusError, ArgusResult};

use super::types::*;

// ============================================================
// Rows
// ============================================================

#[derive(sqlx::FromRow)]
struct RequestRow {
    planning_request_id: Uuid,
    project_id: Uuid,
    planning_kind: String,
    role: String,
    purpose: String,
    context_packet_ref: Option<Uuid>,
    context_hash: String,
    compiler_version: String,
    work_item_id: Option<Uuid>,
    task_profile_ref: Option<Uuid>,
    routing_decision_ref: Option<Uuid>,
    state_revision: i64,
    roadmap_revision: i64,
    objective: String,
    constraints: JsonValue,
    requested_outputs: JsonValue,
    protocol_version: String,
    request_hash: String,
    created_at: DateTime<Utc>,
}

impl RequestRow {
    fn into_record(self) -> PlanningRequest {
        PlanningRequest {
            planning_request_id: self.planning_request_id,
            project_id: self.project_id,
            planning_kind: PlanningKind::parse(&self.planning_kind)
                .unwrap_or(PlanningKind::ProjectPlanning),
            role: self.role,
            purpose: self.purpose,
            context_packet_ref: self.context_packet_ref,
            context_hash: self.context_hash,
            compiler_version: self.compiler_version,
            work_item_id: self.work_item_id,
            task_profile_ref: self.task_profile_ref,
            routing_decision_ref: self.routing_decision_ref,
            state_revision: self.state_revision,
            roadmap_revision: self.roadmap_revision,
            objective: self.objective,
            constraints: strings(self.constraints),
            requested_outputs: strings(self.requested_outputs)
                .iter()
                .filter_map(|s| RequestedOutput::parse(s))
                .collect(),
            protocol_version: self.protocol_version,
            request_hash: self.request_hash,
            created_at: self.created_at,
        }
    }
}

const REQUEST_COLUMNS: &str = "planning_request_id, project_id, planning_kind, role, purpose, \
    context_packet_ref, context_hash, compiler_version, work_item_id, task_profile_ref, routing_decision_ref, \
    state_revision, roadmap_revision, objective, constraints, requested_outputs, protocol_version, \
    request_hash, created_at";

#[derive(sqlx::FromRow)]
struct ResultRow {
    result_json: JsonValue,
}

#[derive(sqlx::FromRow)]
struct ProposalRow {
    proposal_id: Uuid,
    planning_result_id: Uuid,
    planning_request_id: Uuid,
    project_id: Uuid,
    status: String,
    proposal_json: JsonValue,
    validation_errors: JsonValue,
    context_hash: String,
    request_hash: String,
    result_hash: String,
    state_revision: i64,
    roadmap_revision: i64,
    applied_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl ProposalRow {
    fn into_record(self) -> ArgusResult<ProposalRecord> {
        let proposal: PlanProposal =
            serde_json::from_value(self.proposal_json).map_err(ArgusError::Serialization)?;
        Ok(ProposalRecord {
            proposal_id: self.proposal_id,
            planning_result_id: self.planning_result_id,
            planning_request_id: self.planning_request_id,
            project_id: self.project_id,
            status: ProposalStatus::parse(&self.status).unwrap_or(ProposalStatus::Draft),
            proposal,
            validation_errors: strings(self.validation_errors),
            context_hash: self.context_hash,
            request_hash: self.request_hash,
            result_hash: self.result_hash,
            state_revision: self.state_revision,
            roadmap_revision: self.roadmap_revision,
            applied_at: self.applied_at,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

const PROPOSAL_COLUMNS: &str = "proposal_id, planning_result_id, planning_request_id, project_id, \
    status, proposal_json, validation_errors, context_hash, request_hash, result_hash, \
    state_revision, roadmap_revision, applied_at, created_at, updated_at";

fn strings(v: JsonValue) -> Vec<String> {
    match v {
        JsonValue::Array(items) => items
            .into_iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

fn strings_json(v: &[String]) -> JsonValue {
    JsonValue::Array(v.iter().map(|s| JsonValue::String(s.clone())).collect())
}

// ============================================================
// Trait
// ============================================================

#[async_trait]
pub trait PlanningRepository: Send + Sync {
    async fn get_revisions(&self, project_id: Uuid) -> ArgusResult<ProjectRevisions>;

    async fn save_request(&self, request: PlanningRequest) -> ArgusResult<PlanningRequest>;
    async fn get_request(&self, id: Uuid) -> ArgusResult<Option<PlanningRequest>>;

    async fn save_result(
        &self,
        result: PlanningResult,
        project_id: Uuid,
    ) -> ArgusResult<PlanningResultRecord>;
    async fn get_result_by_hash(
        &self,
        project_id: Uuid,
        result_hash: &str,
    ) -> ArgusResult<Option<PlanningResultRecord>>;

    async fn save_proposal(&self, proposal: ProposalRecord) -> ArgusResult<ProposalRecord>;
    async fn get_proposal(&self, id: Uuid) -> ArgusResult<Option<ProposalRecord>>;
    async fn get_proposal_by_result_hash(
        &self,
        project_id: Uuid,
        result_hash: &str,
    ) -> ArgusResult<Option<ProposalRecord>>;
    async fn list_proposals(
        &self,
        project_id: Uuid,
        limit: i64,
    ) -> ArgusResult<Vec<ProposalRecord>>;
    async fn set_proposal_status(
        &self,
        proposal_id: Uuid,
        status: ProposalStatus,
        validation_errors: Vec<String>,
    ) -> ArgusResult<()>;

    async fn record_lifecycle(
        &self,
        proposal_id: Uuid,
        project_id: Uuid,
        event_type: &str,
        detail: Option<&str>,
    ) -> ArgusResult<()>;
    async fn list_lifecycle(&self, proposal_id: Uuid) -> ArgusResult<Vec<LifecycleEvent>>;

    /// Apply an approved proposal in a single transaction. All-or-nothing;
    /// idempotent (a second call returns `already_applied = true`).
    async fn apply_proposal(&self, proposal_id: Uuid) -> ArgusResult<ApplyOutcome>;
}

// ============================================================
// Postgres
// ============================================================

pub struct PostgresPlanningRepository {
    pool: PgPool,
}

impl PostgresPlanningRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl PlanningRepository for PostgresPlanningRepository {
    async fn get_revisions(&self, project_id: Uuid) -> ArgusResult<ProjectRevisions> {
        sqlx::query(
            "INSERT INTO project_revisions (project_id) VALUES ($1) \
             ON CONFLICT (project_id) DO NOTHING",
        )
        .bind(project_id)
        .execute(&self.pool)
        .await
        .map_err(ArgusError::Database)?;
        let row = sqlx::query(
            "SELECT state_revision, roadmap_revision FROM project_revisions WHERE project_id = $1",
        )
        .bind(project_id)
        .fetch_one(&self.pool)
        .await
        .map_err(ArgusError::Database)?;
        Ok(ProjectRevisions {
            project_id,
            state_revision: row.get("state_revision"),
            roadmap_revision: row.get("roadmap_revision"),
        })
    }

    async fn save_request(&self, request: PlanningRequest) -> ArgusResult<PlanningRequest> {
        let query = format!(
            r#"
            INSERT INTO planning_requests (
                planning_request_id, project_id, planning_kind, role, purpose,
                context_packet_ref, context_hash, compiler_version, work_item_id,
                task_profile_ref, routing_decision_ref, state_revision, roadmap_revision, objective,
                constraints, requested_outputs, protocol_version, request_hash, created_at
            )
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,NOW())
            ON CONFLICT (project_id, request_hash) DO UPDATE SET request_hash = EXCLUDED.request_hash
            RETURNING {REQUEST_COLUMNS}
            "#
        );
        let row = sqlx::query_as::<_, RequestRow>(&query)
            .bind(request.planning_request_id)
            .bind(request.project_id)
            .bind(request.planning_kind.as_str())
            .bind(&request.role)
            .bind(&request.purpose)
            .bind(request.context_packet_ref)
            .bind(&request.context_hash)
            .bind(&request.compiler_version)
            .bind(request.work_item_id)
            .bind(request.task_profile_ref)
            .bind(request.routing_decision_ref)
            .bind(request.state_revision)
            .bind(request.roadmap_revision)
            .bind(&request.objective)
            .bind(strings_json(&request.constraints))
            .bind(strings_json(
                &request
                    .requested_outputs
                    .iter()
                    .map(|o| o.as_str().to_string())
                    .collect::<Vec<_>>(),
            ))
            .bind(&request.protocol_version)
            .bind(&request.request_hash)
            .fetch_one(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(row.into_record())
    }

    async fn get_request(&self, id: Uuid) -> ArgusResult<Option<PlanningRequest>> {
        let query = format!(
            "SELECT {REQUEST_COLUMNS} FROM planning_requests WHERE planning_request_id = $1"
        );
        let row = sqlx::query_as::<_, RequestRow>(&query)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(row.map(RequestRow::into_record))
    }

    async fn save_result(
        &self,
        result: PlanningResult,
        project_id: Uuid,
    ) -> ArgusResult<PlanningResultRecord> {
        let query = r#"
            INSERT INTO planning_results (
                planning_result_id, planning_request_id, project_id, status, summary,
                result_json, planner_metadata, protocol_version, context_hash, request_hash,
                result_hash, created_at
            )
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,NOW())
            ON CONFLICT (project_id, result_hash) DO UPDATE SET result_hash = EXCLUDED.result_hash
            RETURNING result_json
        "#;
        let row = sqlx::query_as::<_, ResultRow>(query)
            .bind(result.planning_result_id)
            .bind(result.planning_request_id)
            .bind(project_id)
            .bind(result.status.as_str())
            .bind(&result.summary)
            .bind(serde_json::to_value(&result).unwrap_or(JsonValue::Null))
            .bind(serde_json::to_value(&result.planner_metadata).unwrap_or(JsonValue::Null))
            .bind(&result.protocol_version)
            .bind(&result.context_hash)
            .bind(&result.request_hash)
            .bind(&result.result_hash)
            .fetch_one(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        let stored: PlanningResult =
            serde_json::from_value(row.result_json).map_err(ArgusError::Serialization)?;
        Ok(PlanningResultRecord {
            result: stored,
            project_id,
        })
    }

    async fn get_result_by_hash(
        &self,
        project_id: Uuid,
        result_hash: &str,
    ) -> ArgusResult<Option<PlanningResultRecord>> {
        let row = sqlx::query_as::<_, ResultRow>(
            "SELECT result_json FROM planning_results WHERE project_id = $1 AND result_hash = $2",
        )
        .bind(project_id)
        .bind(result_hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(ArgusError::Database)?;
        match row {
            None => Ok(None),
            Some(r) => Ok(Some(PlanningResultRecord {
                result: serde_json::from_value(r.result_json).map_err(ArgusError::Serialization)?,
                project_id,
            })),
        }
    }

    async fn save_proposal(&self, proposal: ProposalRecord) -> ArgusResult<ProposalRecord> {
        let query = format!(
            r#"
            INSERT INTO plan_proposals (
                proposal_id, planning_result_id, planning_request_id, project_id, status,
                proposal_json, validation_errors, context_hash, request_hash, result_hash,
                state_revision, roadmap_revision, applied_at, created_at, updated_at
            )
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,NOW(),NOW())
            ON CONFLICT (project_id, result_hash) DO UPDATE SET result_hash = EXCLUDED.result_hash
            RETURNING {PROPOSAL_COLUMNS}
            "#
        );
        let row = sqlx::query_as::<_, ProposalRow>(&query)
            .bind(proposal.proposal_id)
            .bind(proposal.planning_result_id)
            .bind(proposal.planning_request_id)
            .bind(proposal.project_id)
            .bind(proposal.status.as_str())
            .bind(serde_json::to_value(&proposal.proposal).unwrap_or(JsonValue::Null))
            .bind(strings_json(&proposal.validation_errors))
            .bind(&proposal.context_hash)
            .bind(&proposal.request_hash)
            .bind(&proposal.result_hash)
            .bind(proposal.state_revision)
            .bind(proposal.roadmap_revision)
            .bind(proposal.applied_at)
            .fetch_one(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        row.into_record()
    }

    async fn get_proposal(&self, id: Uuid) -> ArgusResult<Option<ProposalRecord>> {
        let query = format!("SELECT {PROPOSAL_COLUMNS} FROM plan_proposals WHERE proposal_id = $1");
        let row = sqlx::query_as::<_, ProposalRow>(&query)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        row.map(ProposalRow::into_record).transpose()
    }

    async fn get_proposal_by_result_hash(
        &self,
        project_id: Uuid,
        result_hash: &str,
    ) -> ArgusResult<Option<ProposalRecord>> {
        let query = format!(
            "SELECT {PROPOSAL_COLUMNS} FROM plan_proposals WHERE project_id = $1 AND result_hash = $2"
        );
        let row = sqlx::query_as::<_, ProposalRow>(&query)
            .bind(project_id)
            .bind(result_hash)
            .fetch_optional(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        row.map(ProposalRow::into_record).transpose()
    }

    async fn list_proposals(
        &self,
        project_id: Uuid,
        limit: i64,
    ) -> ArgusResult<Vec<ProposalRecord>> {
        let query = format!(
            "SELECT {PROPOSAL_COLUMNS} FROM plan_proposals WHERE project_id = $1 \
             ORDER BY created_at DESC LIMIT $2"
        );
        let rows = sqlx::query_as::<_, ProposalRow>(&query)
            .bind(project_id)
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        rows.into_iter().map(ProposalRow::into_record).collect()
    }

    async fn set_proposal_status(
        &self,
        proposal_id: Uuid,
        status: ProposalStatus,
        validation_errors: Vec<String>,
    ) -> ArgusResult<()> {
        sqlx::query(
            "UPDATE plan_proposals SET status = $2, validation_errors = $3 WHERE proposal_id = $1",
        )
        .bind(proposal_id)
        .bind(status.as_str())
        .bind(strings_json(&validation_errors))
        .execute(&self.pool)
        .await
        .map_err(ArgusError::Database)?;
        Ok(())
    }

    async fn record_lifecycle(
        &self,
        proposal_id: Uuid,
        project_id: Uuid,
        event_type: &str,
        detail: Option<&str>,
    ) -> ArgusResult<()> {
        sqlx::query(
            "INSERT INTO proposal_lifecycle_events (proposal_id, project_id, event_type, detail) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(proposal_id)
        .bind(project_id)
        .bind(event_type)
        .bind(detail)
        .execute(&self.pool)
        .await
        .map_err(ArgusError::Database)?;
        Ok(())
    }

    async fn list_lifecycle(&self, proposal_id: Uuid) -> ArgusResult<Vec<LifecycleEvent>> {
        let rows = sqlx::query_as::<_, (Uuid, Uuid, Uuid, String, Option<String>, DateTime<Utc>)>(
            "SELECT event_id, proposal_id, project_id, event_type, detail, created_at \
             FROM proposal_lifecycle_events WHERE proposal_id = $1 ORDER BY created_at ASC",
        )
        .bind(proposal_id)
        .fetch_all(&self.pool)
        .await
        .map_err(ArgusError::Database)?;
        Ok(rows
            .into_iter()
            .map(|r| LifecycleEvent {
                event_id: r.0,
                proposal_id: r.1,
                project_id: r.2,
                event_type: r.3,
                detail: r.4,
                created_at: r.5,
            })
            .collect())
    }

    async fn apply_proposal(&self, proposal_id: Uuid) -> ArgusResult<ApplyOutcome> {
        let mut tx = self.pool.begin().await.map_err(ArgusError::Database)?;

        let row = sqlx::query(
            "SELECT status, project_id, state_revision, roadmap_revision, proposal_json \
             FROM plan_proposals WHERE proposal_id = $1 FOR UPDATE",
        )
        .bind(proposal_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(ArgusError::Database)?
        .ok_or_else(|| ArgusError::NotFound(format!("proposal {proposal_id} not found")))?;

        let status_text: String = row.get("status");
        let status = ProposalStatus::parse(&status_text).unwrap_or(ProposalStatus::Draft);
        let project_id: Uuid = row.get("project_id");
        let snapshot_state: i64 = row.get("state_revision");
        let snapshot_roadmap: i64 = row.get("roadmap_revision");
        let proposal_json: JsonValue = row.get("proposal_json");

        if status == ProposalStatus::Applied {
            tx.commit().await.map_err(ArgusError::Database)?;
            let revs = self.get_revisions(project_id).await?;
            return Ok(ApplyOutcome {
                proposal_id,
                already_applied: true,
                milestones_created: 0,
                phases_created: 0,
                work_items_created: 0,
                decisions_created: 0,
                risks_created: 0,
                blockers_created: 0,
                deferred_improvements_created: 0,
                state_revision: revs.state_revision,
                roadmap_revision: revs.roadmap_revision,
            });
        }
        if status != ProposalStatus::Approved {
            return Err(ArgusError::Validation(format!(
                "proposal {proposal_id} is '{}', not approved",
                status.as_str()
            )));
        }

        // Lock and check revisions inside the transaction (stale protection).
        sqlx::query(
            "INSERT INTO project_revisions (project_id) VALUES ($1) ON CONFLICT DO NOTHING",
        )
        .bind(project_id)
        .execute(&mut *tx)
        .await
        .map_err(ArgusError::Database)?;
        let rev_row = sqlx::query(
            "SELECT state_revision, roadmap_revision FROM project_revisions \
             WHERE project_id = $1 FOR UPDATE",
        )
        .bind(project_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(ArgusError::Database)?;
        let current_state: i64 = rev_row.get("state_revision");
        let current_roadmap: i64 = rev_row.get("roadmap_revision");
        if current_state != snapshot_state || current_roadmap != snapshot_roadmap {
            sqlx::query("UPDATE plan_proposals SET status = 'stale' WHERE proposal_id = $1")
                .bind(proposal_id)
                .execute(&mut *tx)
                .await
                .map_err(ArgusError::Database)?;
            tx.commit().await.map_err(ArgusError::Database)?;
            return Err(ArgusError::Validation(format!(
                "stale proposal: revisions changed (state {snapshot_state}->{current_state}, \
                 roadmap {snapshot_roadmap}->{current_roadmap})"
            )));
        }

        let proposal: PlanProposal =
            serde_json::from_value(proposal_json).map_err(ArgusError::Serialization)?;

        let mut outcome = ApplyOutcome {
            proposal_id,
            already_applied: false,
            milestones_created: 0,
            phases_created: 0,
            work_items_created: 0,
            decisions_created: 0,
            risks_created: 0,
            blockers_created: 0,
            deferred_improvements_created: 0,
            state_revision: current_state + 1,
            roadmap_revision: current_roadmap + 1,
        };

        apply_milestones(&mut tx, project_id, &proposal, &mut outcome).await?;
        apply_phases(&mut tx, project_id, &proposal, &mut outcome).await?;
        apply_work_items(&mut tx, project_id, &proposal, &mut outcome).await?;
        apply_decisions(&mut tx, project_id, &proposal, &mut outcome).await?;
        apply_risks(&mut tx, project_id, &proposal, &mut outcome).await?;
        apply_blockers(&mut tx, project_id, &proposal, &mut outcome).await?;
        apply_improvements(&mut tx, project_id, &proposal, &mut outcome).await?;

        sqlx::query(
            "UPDATE project_revisions SET state_revision = state_revision + 1, \
             roadmap_revision = roadmap_revision + 1, updated_at = NOW() WHERE project_id = $1",
        )
        .bind(project_id)
        .execute(&mut *tx)
        .await
        .map_err(ArgusError::Database)?;

        sqlx::query(
            "UPDATE plan_proposals SET status = 'applied', applied_at = NOW() \
             WHERE proposal_id = $1",
        )
        .bind(proposal_id)
        .execute(&mut *tx)
        .await
        .map_err(ArgusError::Database)?;
        sqlx::query(
            "INSERT INTO proposal_lifecycle_events (proposal_id, project_id, event_type, detail) \
             VALUES ($1, $2, 'applied', $3)",
        )
        .bind(proposal_id)
        .bind(project_id)
        .bind(format!(
            "+{} milestone, +{} phase, +{} work item",
            outcome.milestones_created, outcome.phases_created, outcome.work_items_created
        ))
        .execute(&mut *tx)
        .await
        .map_err(ArgusError::Database)?;

        tx.commit().await.map_err(ArgusError::Database)?;
        Ok(outcome)
    }
}

// ============================================================
// Apply helpers (run inside the transaction)
// ============================================================

type Tx<'a> = sqlx::Transaction<'a, sqlx::Postgres>;

async fn apply_milestones(
    tx: &mut Tx<'_>,
    project_id: Uuid,
    proposal: &PlanProposal,
    outcome: &mut ApplyOutcome,
) -> ArgusResult<()> {
    for m in &proposal.milestone_changes {
        match m.operation {
            ProposalOperation::Create => {
                let seq: i32 = match m.sequence {
                    Some(s) => s,
                    None => sqlx::query("SELECT COALESCE(MAX(sequence),0)+1 AS s FROM milestones WHERE project_id=$1")
                        .bind(project_id)
                        .fetch_one(&mut **tx)
                        .await
                        .map_err(ArgusError::Database)?
                        .get("s"),
                };
                let status = m.status.unwrap_or(MilestoneStatus::Planned);
                sqlx::query(
                    "INSERT INTO milestones (project_id, name, description, sequence, status, external_key, definition_of_done) \
                     VALUES ($1,$2,$3,$4,$5,$6,$7)",
                )
                .bind(project_id)
                .bind(&m.title)
                .bind(&m.description)
                .bind(seq)
                .bind(status.as_str())
                .bind(&m.external_key)
                .bind(&m.definition_of_done)
                .execute(&mut **tx)
                .await
                .map_err(ArgusError::Database)?;
                outcome.milestones_created += 1;
            }
            ProposalOperation::Transition => {
                set_milestone_status(tx, m.target_id, m.status).await?;
            }
            ProposalOperation::Defer => {
                set_milestone_status(tx, m.target_id, Some(MilestoneStatus::Deferred)).await?;
            }
            ProposalOperation::Cancel => {
                set_milestone_status(tx, m.target_id, Some(MilestoneStatus::Cancelled)).await?;
            }
            ProposalOperation::Update | ProposalOperation::Supersede => {
                if let Some(id) = m.target_id {
                    sqlx::query(
                        "UPDATE milestones SET name = COALESCE($2,name), description = COALESCE($3,description), \
                         external_key = COALESCE($4,external_key), definition_of_done = COALESCE($5,definition_of_done), \
                         status = COALESCE($6,status) WHERE id = $1",
                    )
                    .bind(id)
                    .bind(&m.title)
                    .bind(&m.description)
                    .bind(&m.external_key)
                    .bind(&m.definition_of_done)
                    .bind(m.status.map(|s| s.as_str().to_string()))
                    .execute(&mut **tx)
                    .await
                    .map_err(ArgusError::Database)?;
                }
            }
            ProposalOperation::NoOp => {}
        }
    }
    Ok(())
}

async fn set_milestone_status(
    tx: &mut Tx<'_>,
    target: Option<Uuid>,
    status: Option<MilestoneStatus>,
) -> ArgusResult<()> {
    if let (Some(id), Some(status)) = (target, status) {
        sqlx::query("UPDATE milestones SET status = $2 WHERE id = $1")
            .bind(id)
            .bind(status.as_str())
            .execute(&mut **tx)
            .await
            .map_err(ArgusError::Database)?;
    }
    Ok(())
}

async fn apply_phases(
    tx: &mut Tx<'_>,
    project_id: Uuid,
    proposal: &PlanProposal,
    outcome: &mut ApplyOutcome,
) -> ArgusResult<()> {
    for p in &proposal.phase_changes {
        match p.operation {
            ProposalOperation::Create => {
                let milestone_id = p.milestone_id.ok_or_else(|| {
                    ArgusError::Validation("phase create missing milestone".into())
                })?;
                let seq: i32 = match p.sequence {
                    Some(s) => s,
                    None => sqlx::query(
                        "SELECT COALESCE(MAX(sequence),0)+1 AS s FROM phases WHERE milestone_id=$1",
                    )
                    .bind(milestone_id)
                    .fetch_one(&mut **tx)
                    .await
                    .map_err(ArgusError::Database)?
                    .get("s"),
                };
                let status = p.status.unwrap_or(MilestoneStatus::Planned);
                sqlx::query(
                    "INSERT INTO phases (project_id, milestone_id, title, description, sequence, status, external_key, definition_of_done) \
                     VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
                )
                .bind(project_id)
                .bind(milestone_id)
                .bind(&p.title)
                .bind(&p.description)
                .bind(seq)
                .bind(status.as_str())
                .bind(&p.external_key)
                .bind(&p.definition_of_done)
                .execute(&mut **tx)
                .await
                .map_err(ArgusError::Database)?;
                outcome.phases_created += 1;
            }
            ProposalOperation::Transition
            | ProposalOperation::Defer
            | ProposalOperation::Cancel => {
                if let Some(id) = p.target_id {
                    let status = match p.operation {
                        ProposalOperation::Transition => {
                            p.status.unwrap_or(MilestoneStatus::Active)
                        }
                        ProposalOperation::Defer => MilestoneStatus::Deferred,
                        _ => MilestoneStatus::Cancelled,
                    };
                    sqlx::query("UPDATE phases SET status = $2 WHERE phase_id = $1")
                        .bind(id)
                        .bind(status.as_str())
                        .execute(&mut **tx)
                        .await
                        .map_err(ArgusError::Database)?;
                }
            }
            ProposalOperation::Update | ProposalOperation::Supersede => {
                if let Some(id) = p.target_id {
                    sqlx::query(
                        "UPDATE phases SET title = COALESCE($2,title), description = COALESCE($3,description), \
                         external_key = COALESCE($4,external_key), definition_of_done = COALESCE($5,definition_of_done), \
                         status = COALESCE($6,status) WHERE phase_id = $1",
                    )
                    .bind(id)
                    .bind(&p.title)
                    .bind(&p.description)
                    .bind(&p.external_key)
                    .bind(&p.definition_of_done)
                    .bind(p.status.map(|s| s.as_str().to_string()))
                    .execute(&mut **tx)
                    .await
                    .map_err(ArgusError::Database)?;
                }
            }
            ProposalOperation::NoOp => {}
        }
    }
    Ok(())
}

async fn apply_work_items(
    tx: &mut Tx<'_>,
    project_id: Uuid,
    proposal: &PlanProposal,
    outcome: &mut ApplyOutcome,
) -> ArgusResult<()> {
    for w in &proposal.work_item_changes {
        let work_item_id = match w.operation {
            ProposalOperation::Create => {
                let status = w
                    .status
                    .unwrap_or(crate::project_state::WorkItemStatus::Planned);
                let priority = w.priority.unwrap_or(Priority::Normal);
                let item: Uuid = sqlx::query(
                    "INSERT INTO work_items (project_id, milestone_id, phase_id, title, description, status, priority, \
                     definition_of_done, source, external_key, weight) \
                     VALUES ($1,$2,$3,$4,$5,$6,$7,$8,'planner',$9,$10) RETURNING work_item_id",
                )
                .bind(project_id)
                .bind(w.milestone_id)
                .bind(w.phase_id)
                .bind(&w.title)
                .bind(&w.description)
                .bind(status.as_str())
                .bind(priority_str(priority))
                .bind(&w.definition_of_done)
                .bind(&w.external_key)
                .bind(w.weight.unwrap_or(1))
                .fetch_one(&mut **tx)
                .await
                .map_err(ArgusError::Database)?
                .get("work_item_id");
                outcome.work_items_created += 1;
                item
            }
            ProposalOperation::Transition
            | ProposalOperation::Defer
            | ProposalOperation::Cancel => {
                let id = w.target_id.ok_or_else(|| {
                    ArgusError::Validation("work item transition missing target".into())
                })?;
                let status = match w.operation {
                    ProposalOperation::Transition => w
                        .status
                        .unwrap_or(crate::project_state::WorkItemStatus::Active),
                    ProposalOperation::Defer => crate::project_state::WorkItemStatus::Deferred,
                    _ => crate::project_state::WorkItemStatus::Cancelled,
                };
                sqlx::query("UPDATE work_items SET status = $2 WHERE work_item_id = $1")
                    .bind(id)
                    .bind(status.as_str())
                    .execute(&mut **tx)
                    .await
                    .map_err(ArgusError::Database)?;
                id
            }
            ProposalOperation::Update | ProposalOperation::Supersede => {
                let id = w.target_id.ok_or_else(|| {
                    ArgusError::Validation("work item update missing target".into())
                })?;
                sqlx::query(
                    "UPDATE work_items SET title = COALESCE($2,title), description = COALESCE($3,description), \
                     definition_of_done = COALESCE($4,definition_of_done), milestone_id = COALESCE($5,milestone_id), \
                     phase_id = COALESCE($6,phase_id), status = COALESCE($7,status), \
                     priority = COALESCE($8,priority), external_key = COALESCE($9,external_key), \
                     weight = COALESCE($10,weight) WHERE work_item_id = $1",
                )
                .bind(id)
                .bind(&w.title)
                .bind(&w.description)
                .bind(&w.definition_of_done)
                .bind(w.milestone_id)
                .bind(w.phase_id)
                .bind(w.status.map(|s| s.as_str().to_string()))
                .bind(w.priority.map(priority_str).map(str::to_string))
                .bind(&w.external_key)
                .bind(w.weight)
                .execute(&mut **tx)
                .await
                .map_err(ArgusError::Database)?;
                id
            }
            ProposalOperation::NoOp => continue,
        };

        for dep in &w.dependencies {
            sqlx::query(
                "INSERT INTO work_item_dependencies (work_item_id, depends_on_id) VALUES ($1,$2) \
                 ON CONFLICT DO NOTHING",
            )
            .bind(work_item_id)
            .bind(dep)
            .execute(&mut **tx)
            .await
            .map_err(ArgusError::Database)?;
        }
    }
    Ok(())
}

async fn apply_decisions(
    tx: &mut Tx<'_>,
    project_id: Uuid,
    proposal: &PlanProposal,
    outcome: &mut ApplyOutcome,
) -> ArgusResult<()> {
    for d in &proposal.decision_proposals {
        sqlx::query(
            "INSERT INTO project_decisions (project_id, title, context, decision, reason, status, source) \
             VALUES ($1,$2,$3,$4,$5,'proposed','planner')",
        )
        .bind(project_id)
        .bind(&d.title)
        .bind(&d.context)
        .bind(&d.proposed_decision)
        .bind(&d.rationale)
        .execute(&mut **tx)
        .await
        .map_err(ArgusError::Database)?;
        outcome.decisions_created += 1;
    }
    Ok(())
}

async fn apply_risks(
    tx: &mut Tx<'_>,
    project_id: Uuid,
    proposal: &PlanProposal,
    outcome: &mut ApplyOutcome,
) -> ArgusResult<()> {
    for r in &proposal.risk_proposals {
        sqlx::query(
            "INSERT INTO project_risks (project_id, title, description, severity, likelihood, status, mitigation, owner) \
             VALUES ($1,$2,$3,$4,$5,'open',$6,$7)",
        )
        .bind(project_id)
        .bind(&r.title)
        .bind(&r.description)
        .bind(r.severity.as_str())
        .bind(r.likelihood.as_str())
        .bind(&r.mitigation)
        .bind(&r.owner)
        .execute(&mut **tx)
        .await
        .map_err(ArgusError::Database)?;
        outcome.risks_created += 1;
    }
    Ok(())
}

async fn apply_blockers(
    tx: &mut Tx<'_>,
    project_id: Uuid,
    proposal: &PlanProposal,
    outcome: &mut ApplyOutcome,
) -> ArgusResult<()> {
    for b in &proposal.blocker_proposals {
        sqlx::query(
            "INSERT INTO project_blockers (project_id, work_item_id, title, description, status) \
             VALUES ($1,$2,$3,$4,'open')",
        )
        .bind(project_id)
        .bind(b.work_item_id)
        .bind(&b.title)
        .bind(&b.description)
        .execute(&mut **tx)
        .await
        .map_err(ArgusError::Database)?;
        outcome.blockers_created += 1;
    }
    Ok(())
}

async fn apply_improvements(
    tx: &mut Tx<'_>,
    project_id: Uuid,
    proposal: &PlanProposal,
    outcome: &mut ApplyOutcome,
) -> ArgusResult<()> {
    for i in &proposal.deferred_improvement_proposals {
        sqlx::query(
            "INSERT INTO deferred_improvements (project_id, external_key, title, description, priority, status, source, why_deferred, required_before) \
             VALUES ($1,$2,$3,$4,$5,'deferred','planner',$6,$7)",
        )
        .bind(project_id)
        .bind(&i.external_key)
        .bind(&i.title)
        .bind(&i.description)
        .bind(improvement_priority_str(i.priority.unwrap_or(ImprovementPriority::Medium)))
        .bind(&i.why_deferred)
        .bind(&i.required_before)
        .execute(&mut **tx)
        .await
        .map_err(ArgusError::Database)?;
        outcome.deferred_improvements_created += 1;
    }
    Ok(())
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

fn improvement_priority_str(p: ImprovementPriority) -> &'static str {
    p.as_str()
}
