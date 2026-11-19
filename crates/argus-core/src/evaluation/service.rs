use std::sync::Arc;

use chrono::Utc;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::project_state::{
    PostgresProjectStateRepository, ProjectStateRepository, WorkItemStatus,
};
use crate::types::{ArgusError, ArgusResult};

use super::{
    ArtifactEvaluator, EVALUATOR_VERSION, EvaluationContext, EvaluationOutcome, EvaluationReport,
    Evaluator, GitCheckpointEvaluator, TaskEvidence, aggregate,
};

const TASK_EVIDENCE_SQL: &str = "
    SELECT t.id AS task_id, t.project_id, t.state::text AS state, t.execution_id,
        a.artifact_id, a.project_id AS artifact_project_id,
        a.execution_id AS artifact_execution_id,
        a.workspace_id,
        w.project_id AS workspace_project_id, w.task_id AS workspace_task_id,
        w.execution_id AS workspace_execution_id, w.worktree_path,
        a.base_revision, a.commit_sha, a.files_changed, a.quality_status, a.quality_summary,
        jsonb_build_object('title', t.title, 'description', t.description,
            'test_commands', t.context #> '{scheduler_task,test_commands}',
            'acceptance_criteria', t.context #> '{scheduler_task,acceptance_criteria}') AS task_contract
    FROM work_item_tasks link JOIN tasks t ON t.id = link.task_id
    LEFT JOIN task_artifacts a ON a.task_id = t.id AND a.execution_id = t.execution_id
    LEFT JOIN task_workspaces w ON w.workspace_id = a.workspace_id
    WHERE link.work_item_id = $1 ORDER BY t.id";

pub struct EvaluationHarness {
    pool: PgPool,
    state: Arc<dyn ProjectStateRepository>,
    evaluators: Vec<Arc<dyn Evaluator>>,
}

impl EvaluationHarness {
    pub fn new(pool: PgPool) -> Self {
        Self {
            state: Arc::new(PostgresProjectStateRepository::new(pool.clone())),
            pool,
            evaluators: vec![
                Arc::new(ArtifactEvaluator),
                Arc::new(GitCheckpointEvaluator),
            ],
        }
    }

    /// Adds a required verifier; the built-in provenance and Git checks remain
    /// mandatory. Custom evaluators cannot replace them or grant a pass alone.
    pub fn with_evaluator(mut self, evaluator: Arc<dyn Evaluator>) -> Self {
        self.evaluators.push(evaluator);
        self
    }

    async fn context(
        &self,
        project_id: Uuid,
        work_item_id: Uuid,
        connection: &mut PgConnection,
    ) -> ArgusResult<EvaluationContext> {
        let row: Option<(serde_json::Value,)> = sqlx::query_as("SELECT to_jsonb(wi) || jsonb_build_object('description', COALESCE(description,'')) FROM work_items wi WHERE work_item_id=$1 AND project_id=$2")
            .bind(work_item_id).bind(project_id).fetch_optional(&mut *connection).await?;
        let work_item = serde_json::from_value(
            row.ok_or_else(|| ArgusError::NotFound("Work item not found in project".into()))?
                .0,
        )?;
        let tasks = sqlx::query_as::<_, TaskEvidence>(TASK_EVIDENCE_SQL)
            .bind(work_item_id)
            .fetch_all(connection)
            .await?;
        Ok(EvaluationContext { work_item, tasks })
    }

    pub async fn evaluate(
        &self,
        project_id: Uuid,
        work_item_id: Uuid,
    ) -> ArgusResult<EvaluationReport> {
        // Lock the WorkItem against concurrent planning edits and task linkage
        // until the report, evidence, state, activity, and revision commit together.
        let mut tx = self.pool.begin().await?;
        self.lock(project_id, work_item_id, &mut tx).await?;
        let context = self.context(project_id, work_item_id, &mut tx).await?;
        if matches!(
            context.work_item.status,
            WorkItemStatus::Cancelled | WorkItemStatus::Deferred
        ) {
            return Err(ArgusError::Validation(
                "Cancelled or deferred work cannot be verified".into(),
            ));
        }
        let mut checks = Vec::new();
        for evaluator in &self.evaluators {
            checks.push(evaluator.evaluate(&context).await);
        }
        let report = EvaluationReport {
            evaluation_id: Uuid::new_v4(),
            project_id,
            work_item_id,
            input_hash: context.input_hash()?,
            evaluator_version: EVALUATOR_VERSION.into(),
            outcome: aggregate(&checks),
            checks,
            created_at: Utc::now(),
        };
        // Task completion may have changed during external Git inspection.
        let current = self.context(project_id, work_item_id, &mut tx).await?;
        if current.input_hash()? != report.input_hash {
            return Err(ArgusError::Validation(
                "Evaluation evidence changed; retry verification".into(),
            ));
        }
        sqlx::query("INSERT INTO work_item_evaluations (evaluation_id, project_id, work_item_id, input_hash, evaluator_version, outcome, report, created_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)")
            .bind(report.evaluation_id).bind(project_id).bind(work_item_id).bind(&report.input_hash)
            .bind(EVALUATOR_VERSION).bind(report.outcome.as_str()).bind(serde_json::to_value(&report)?).bind(report.created_at)
            .execute(&mut *tx).await?;
        let status = if report.outcome == EvaluationOutcome::Passed {
            for task in &context.tasks {
                sqlx::query("INSERT INTO work_item_evidence (work_item_id, evidence_type, artifact_id, task_id, execution_id, note) SELECT $1, 'artifact', $2, $3, $4, $5 WHERE NOT EXISTS (SELECT 1 FROM work_item_evidence WHERE work_item_id=$1 AND artifact_id=$2)")
                    .bind(work_item_id).bind(task.artifact_id).bind(task.task_id).bind(task.execution_id)
                    .bind(format!("evaluation:{}", report.evaluation_id)).execute(&mut *tx).await?;
            }
            if context.work_item.status == WorkItemStatus::Accepted {
                "accepted"
            } else {
                "verified_complete"
            }
        } else if context.work_item.status.is_complete() {
            "reported_complete"
        } else {
            context.work_item.status.as_str()
        };
        if status != context.work_item.status.as_str() {
            sqlx::query("UPDATE work_items SET status=$2, completed_at=CASE WHEN $2 IN ('verified_complete','accepted') THEN NOW() ELSE NULL END WHERE work_item_id=$1")
                .bind(work_item_id).bind(status).execute(&mut *tx).await?;
            Self::bump_revision(project_id, &mut tx).await?;
        }
        if report.outcome == EvaluationOutcome::Passed {
            Self::activity(
                project_id,
                work_item_id,
                "WorkItem independently verified",
                report.evaluation_id,
                &mut tx,
            )
            .await?;
        }
        tx.commit().await?;
        Ok(report)
    }

    async fn lock(
        &self,
        project_id: Uuid,
        work_item_id: Uuid,
        connection: &mut PgConnection,
    ) -> ArgusResult<()> {
        let exists: Option<(Uuid,)> = sqlx::query_as(
            "SELECT work_item_id FROM work_items WHERE work_item_id=$1 AND project_id=$2",
        )
        .bind(work_item_id)
        .bind(project_id)
        .fetch_optional(&mut *connection)
        .await?;
        if exists.is_none() {
            return Err(ArgusError::NotFound(
                "Work item not found in project".into(),
            ));
        }
        // Match Planner apply's project -> WorkItem lock order.
        sqlx::query(
            "INSERT INTO project_revisions (project_id) VALUES ($1) ON CONFLICT DO NOTHING",
        )
        .bind(project_id)
        .execute(&mut *connection)
        .await?;
        sqlx::query("SELECT project_id FROM project_revisions WHERE project_id=$1 FOR UPDATE")
            .bind(project_id)
            .fetch_one(&mut *connection)
            .await?;
        let row: Option<(Uuid,)> = sqlx::query_as("SELECT work_item_id FROM work_items WHERE work_item_id=$1 AND project_id=$2 FOR UPDATE")
            .bind(work_item_id).bind(project_id).fetch_optional(connection).await?;
        if row.is_none() {
            return Err(ArgusError::NotFound(
                "Work item not found in project".into(),
            ));
        }
        Ok(())
    }

    async fn bump_revision(project_id: Uuid, connection: &mut PgConnection) -> ArgusResult<()> {
        sqlx::query("INSERT INTO project_revisions (project_id, state_revision) VALUES ($1,1) ON CONFLICT (project_id) DO UPDATE SET state_revision=project_revisions.state_revision+1, updated_at=NOW()")
            .bind(project_id).execute(connection).await?;
        Ok(())
    }

    async fn activity(
        project_id: Uuid,
        work_item_id: Uuid,
        summary: &str,
        evaluation_id: Uuid,
        connection: &mut PgConnection,
    ) -> ArgusResult<()> {
        sqlx::query("INSERT INTO project_activities (project_id, activity_type, entity_type, entity_id, summary, metadata) VALUES ($1,'work_item_verified','work_item',$2,$3,$4)")
            .bind(project_id).bind(work_item_id).bind(summary)
            .bind(serde_json::json!({"evaluation_id":evaluation_id})).execute(connection).await?;
        Ok(())
    }

    /// Human acceptance references a passing, current report, never a supplied
    /// boolean. The authenticated entrypoint supplies the actor identity.
    pub async fn accept(
        &self,
        project_id: Uuid,
        work_item_id: Uuid,
        evaluation_id: Uuid,
        actor: &str,
    ) -> ArgusResult<()> {
        if actor.trim().is_empty() {
            return Err(ArgusError::Validation(
                "Acceptance requires an actor".into(),
            ));
        }
        let mut tx = self.pool.begin().await?;
        self.lock(project_id, work_item_id, &mut tx).await?;
        let row: Option<(serde_json::Value,)> = sqlx::query_as("SELECT report FROM work_item_evaluations WHERE project_id=$1 AND work_item_id=$2 AND evaluation_id=$3")
            .bind(project_id).bind(work_item_id).bind(evaluation_id).fetch_optional(&mut *tx).await?;
        let report: EvaluationReport = serde_json::from_value(
            row.ok_or_else(|| {
                ArgusError::NotFound("Evaluation not found in project/work item".into())
            })?
            .0,
        )?;
        let context = self.context(project_id, work_item_id, &mut tx).await?;
        if report.outcome != EvaluationOutcome::Passed
            || context.input_hash()? != report.input_hash
            || !context.work_item.status.is_complete()
        {
            return Err(ArgusError::Validation(
                "Acceptance requires a current passing evaluation and verified work item".into(),
            ));
        }
        let mut current_checks = Vec::new();
        for evaluator in &self.evaluators {
            current_checks.push(evaluator.evaluate(&context).await);
        }
        if aggregate(&current_checks) != EvaluationOutcome::Passed {
            return Err(ArgusError::Validation(
                "Acceptance evidence no longer verifies".into(),
            ));
        }
        let inserted = sqlx::query("INSERT INTO work_item_acceptances (work_item_id,evaluation_id,actor) VALUES ($1,$2,$3) ON CONFLICT (work_item_id,evaluation_id) DO NOTHING")
            .bind(work_item_id).bind(evaluation_id).bind(actor).execute(&mut *tx).await?.rows_affected();
        if inserted != 0 {
            sqlx::query("UPDATE work_items SET status='accepted' WHERE work_item_id=$1")
                .bind(work_item_id)
                .execute(&mut *tx)
                .await?;
            Self::bump_revision(project_id, &mut tx).await?;
            Self::activity(
                project_id,
                work_item_id,
                "WorkItem accepted by operator",
                evaluation_id,
                &mut tx,
            )
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn get(
        &self,
        project_id: Uuid,
        work_item_id: Uuid,
        evaluation_id: Uuid,
    ) -> ArgusResult<EvaluationReport> {
        let row: Option<(serde_json::Value,)> = sqlx::query_as("SELECT report FROM work_item_evaluations WHERE project_id=$1 AND work_item_id=$2 AND evaluation_id=$3")
            .bind(project_id).bind(work_item_id).bind(evaluation_id).fetch_optional(&self.pool).await?;
        row.map(|r| serde_json::from_value(r.0).map_err(ArgusError::from))
            .transpose()?
            .ok_or_else(|| ArgusError::NotFound("Evaluation not found in project/work item".into()))
    }

    pub async fn list(
        &self,
        project_id: Uuid,
        work_item_id: Uuid,
    ) -> ArgusResult<Vec<EvaluationReport>> {
        let item = self.state.get_work_item(work_item_id).await?;
        if item.is_none_or(|w| w.project_id != project_id) {
            return Err(ArgusError::NotFound(
                "Work item not found in project".into(),
            ));
        }
        let rows: Vec<(serde_json::Value,)> = sqlx::query_as("SELECT report FROM work_item_evaluations WHERE project_id=$1 AND work_item_id=$2 ORDER BY created_at DESC, evaluation_id LIMIT 100")
            .bind(project_id).bind(work_item_id).fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|r| serde_json::from_value(r.0).map_err(ArgusError::from))
            .collect()
    }

    pub async fn evaluate_linked_task(&self, project_id: Uuid, task_id: Uuid) -> ArgusResult<()> {
        let rows: Vec<(Uuid,)> = sqlx::query_as("SELECT wi.work_item_id FROM work_item_tasks link JOIN work_items wi ON wi.work_item_id=link.work_item_id WHERE link.task_id=$1 AND wi.project_id=$2 ORDER BY wi.work_item_id")
            .bind(task_id).bind(project_id).fetch_all(&self.pool).await?;
        for (id,) in rows {
            self.evaluate(project_id, id).await?;
        }
        Ok(())
    }
}
