//! V0.38 Phase 4 — Task Artifact persistence.
//!
//! An `ArtifactRecord` is the durable proof that a task's Agent output was
//! verified (Quality Gate) and turned into a real Git commit in the task's
//! own worktree. It is created exactly once per execution — see
//! [`ArtifactRepository::ensure_git_checkpoint_artifact`] for the
//! idempotency guarantee that makes a finalize replay after a crash safe.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value as JsonValue;
use sqlx::PgPool;
use uuid::Uuid;

use crate::types::{ArgusError, ArgusResult};

/// Fields needed to create a new Git-checkpoint artifact. Split from
/// [`ArtifactRecord`] because `artifact_id` and `created_at` are assigned by
/// the repository, not the caller.
#[derive(Clone, Debug)]
pub struct NewArtifact {
    pub project_id: Uuid,
    pub task_id: Uuid,
    pub execution_id: Uuid,
    pub workspace_id: Uuid,
    pub base_revision: String,
    pub branch_name: String,
    pub commit_sha: String,
    pub files_changed: Vec<String>,
    pub quality_status: Option<String>,
    pub quality_summary: Option<String>,
    pub agent_runtime: Option<String>,
    pub model: Option<String>,
    pub session_id: Option<String>,
}

/// A persisted Task Artifact — currently always `artifact_type ==
/// "git_commit"`, kept as a string column rather than an enum with one
/// variant so a future artifact kind doesn't require a schema rewrite.
#[derive(Clone, Debug)]
pub struct ArtifactRecord {
    pub artifact_id: Uuid,
    pub artifact_type: String,
    pub project_id: Uuid,
    pub task_id: Uuid,
    pub execution_id: Uuid,
    pub workspace_id: Uuid,
    pub base_revision: String,
    pub branch_name: String,
    pub commit_sha: String,
    pub files_changed: Vec<String>,
    pub quality_status: Option<String>,
    pub quality_summary: Option<String>,
    pub agent_runtime: Option<String>,
    pub model: Option<String>,
    pub session_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

type ArtifactRow = (
    Uuid,
    String,
    Uuid,
    Uuid,
    Uuid,
    Uuid,
    String,
    String,
    String,
    JsonValue,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    DateTime<Utc>,
);

fn row_to_record(r: ArtifactRow) -> ArtifactRecord {
    let files_changed = match r.9 {
        JsonValue::Array(items) => items
            .into_iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    };
    ArtifactRecord {
        artifact_id: r.0,
        artifact_type: r.1,
        project_id: r.2,
        task_id: r.3,
        execution_id: r.4,
        workspace_id: r.5,
        base_revision: r.6,
        branch_name: r.7,
        commit_sha: r.8,
        files_changed,
        quality_status: r.10,
        quality_summary: r.11,
        agent_runtime: r.12,
        model: r.13,
        session_id: r.14,
        created_at: r.15,
    }
}

const SELECT_COLUMNS: &str = "artifact_id, artifact_type, project_id, task_id, execution_id, \
    workspace_id, base_revision, branch_name, commit_sha, files_changed, \
    quality_status, quality_summary, agent_runtime, model, session_id, created_at";

/// Repository for [`ArtifactRecord`] persistence.
#[async_trait]
pub trait ArtifactRepository: Send + Sync {
    /// Idempotently create a Git-checkpoint artifact for `artifact.execution_id`.
    /// If one already exists (same `execution_id` + `artifact_type`), the
    /// existing record is returned unchanged — this, not application-level
    /// "don't call this twice" discipline, is what makes a finalize replay
    /// after a crash (or any other double-invocation) safe: it can never
    /// produce two artifacts for the same execution.
    async fn ensure_git_checkpoint_artifact(
        &self,
        artifact: NewArtifact,
    ) -> ArgusResult<ArtifactRecord>;

    /// Look up the (at most one) artifact already created for an execution —
    /// the idempotency check every checkpoint attempt starts with.
    async fn get_by_execution_id(&self, execution_id: Uuid) -> ArgusResult<Option<ArtifactRecord>>;

    /// All artifacts a task has ever produced, newest first (rule #33:
    /// `task_id -> artifact` lookup).
    async fn get_by_task_id(&self, task_id: Uuid) -> ArgusResult<Vec<ArtifactRecord>>;
}

/// PostgreSQL-backed [`ArtifactRepository`].
pub struct PostgresArtifactRepository {
    pool: PgPool,
}

impl PostgresArtifactRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ArtifactRepository for PostgresArtifactRepository {
    async fn ensure_git_checkpoint_artifact(
        &self,
        artifact: NewArtifact,
    ) -> ArgusResult<ArtifactRecord> {
        let files_changed = JsonValue::Array(
            artifact
                .files_changed
                .iter()
                .map(|f| JsonValue::String(f.clone()))
                .collect(),
        );

        // UPSERT keyed on (execution_id, artifact_type): a second call for
        // the same execution is a no-op that returns the row already there,
        // never a duplicate and never an error.
        let query = format!(
            r#"
            INSERT INTO task_artifacts (
                artifact_id, artifact_type, project_id, task_id, execution_id,
                workspace_id, base_revision, branch_name, commit_sha, files_changed,
                quality_status, quality_summary, agent_runtime, model, session_id,
                created_at
            )
            VALUES (
                gen_random_uuid(), 'git_commit', $1, $2, $3,
                $4, $5, $6, $7, $8,
                $9, $10, $11, $12, $13,
                NOW()
            )
            ON CONFLICT (execution_id, artifact_type) DO UPDATE SET
                artifact_id = task_artifacts.artifact_id
            RETURNING {SELECT_COLUMNS}
            "#
        );
        let record = sqlx::query_as::<_, ArtifactRow>(&query)
            .bind(artifact.project_id)
            .bind(artifact.task_id)
            .bind(artifact.execution_id)
            .bind(artifact.workspace_id)
            .bind(&artifact.base_revision)
            .bind(&artifact.branch_name)
            .bind(&artifact.commit_sha)
            .bind(files_changed)
            .bind(&artifact.quality_status)
            .bind(&artifact.quality_summary)
            .bind(&artifact.agent_runtime)
            .bind(&artifact.model)
            .bind(&artifact.session_id)
            .fetch_one(&self.pool)
            .await
            .map_err(ArgusError::Database)?;

        Ok(row_to_record(record))
    }

    async fn get_by_execution_id(&self, execution_id: Uuid) -> ArgusResult<Option<ArtifactRecord>> {
        let query = format!("SELECT {SELECT_COLUMNS} FROM task_artifacts WHERE execution_id = $1");
        let record = sqlx::query_as::<_, ArtifactRow>(&query)
            .bind(execution_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(record.map(row_to_record))
    }

    async fn get_by_task_id(&self, task_id: Uuid) -> ArgusResult<Vec<ArtifactRecord>> {
        let query = format!(
            "SELECT {SELECT_COLUMNS} FROM task_artifacts WHERE task_id = $1 ORDER BY created_at DESC"
        );
        let records = sqlx::query_as::<_, ArtifactRow>(&query)
            .bind(task_id)
            .fetch_all(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(records.into_iter().map(row_to_record).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_artifact_record_structure() {
        let a = ArtifactRecord {
            artifact_id: Uuid::new_v4(),
            artifact_type: "git_commit".to_string(),
            project_id: Uuid::new_v4(),
            task_id: Uuid::new_v4(),
            execution_id: Uuid::new_v4(),
            workspace_id: Uuid::new_v4(),
            base_revision: "abc123".to_string(),
            branch_name: "argus/task/1/2".to_string(),
            commit_sha: "def456".to_string(),
            files_changed: vec!["src/lib.rs".to_string()],
            quality_status: Some("passed".to_string()),
            quality_summary: None,
            agent_runtime: Some("claude_code".to_string()),
            model: Some("haiku".to_string()),
            session_id: None,
            created_at: Utc::now(),
        };
        assert_eq!(a.artifact_type, "git_commit");
        assert_eq!(a.files_changed.len(), 1);
    }
}
