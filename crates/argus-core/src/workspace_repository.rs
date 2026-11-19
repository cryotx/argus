//! V0.38 Workspace Repository — persistent workspace lifecycle tracking
//!
//! Single source of truth for task workspace ownership, paths, and status.
//! Each execution maps to exactly one workspace (idempotent).

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::types::{ArgusError, ArgusResult};

/// Persistent workspace record
#[derive(Clone, Debug)]
pub struct WorkspaceRecord {
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub task_id: Uuid,
    pub execution_id: Uuid,
    pub repository_path: String,
    pub worktree_path: String,
    pub branch_name: String,
    pub base_revision: String,
    pub status: String, // Stored as text, maps to WorkspaceStatus
    pub failure_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub activated_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// Raw row shape shared by every query below: id columns as `Uuid` (bound
/// and fetched as native `UUID`, never round-tripped through text — Postgres
/// has no `uuid = text` operator, so a text-bound id in a `WHERE` clause
/// fails outright) and timestamps as `DateTime<Utc>` (fetched natively
/// rather than cast to `TEXT`, since `TIMESTAMPTZ::TEXT`'s
/// `2024-01-01 00:00:00+00` format is not valid RFC 3339 and does not
/// round-trip through `DateTime::parse_from_rfc3339`).
type WorkspaceRow = (
    Uuid,
    Uuid,
    Uuid,
    Uuid,
    String,
    String,
    String,
    String,
    String,
    DateTime<Utc>,
    Option<DateTime<Utc>>,
    Option<DateTime<Utc>>,
    Option<String>,
);

fn row_to_record(r: WorkspaceRow) -> WorkspaceRecord {
    WorkspaceRecord {
        workspace_id: r.0,
        project_id: r.1,
        task_id: r.2,
        execution_id: r.3,
        repository_path: r.4,
        worktree_path: r.5,
        branch_name: r.6,
        base_revision: r.7,
        status: r.8,
        created_at: r.9,
        activated_at: r.10,
        completed_at: r.11,
        failure_reason: r.12,
    }
}

const SELECT_COLUMNS: &str = "workspace_id, project_id, task_id, execution_id, \
    repository_path, worktree_path, branch_name, base_revision, \
    status, created_at, activated_at, completed_at, failure_reason";

/// Repository trait for workspace persistence
#[async_trait]
pub trait WorkspaceRepository: Send + Sync {
    /// Create or idempotent-fetch a workspace for an execution
    #[allow(clippy::too_many_arguments)]
    async fn ensure_workspace(
        &self,
        execution_id: Uuid,
        project_id: Uuid,
        task_id: Uuid,
        repository_path: String,
        worktree_path: String,
        branch_name: String,
        base_revision: String,
    ) -> ArgusResult<WorkspaceRecord>;

    /// Retrieve workspace by ID
    async fn get_workspace(&self, workspace_id: Uuid) -> ArgusResult<Option<WorkspaceRecord>>;

    /// Retrieve workspace by execution ID (unique mapping)
    async fn get_by_execution_id(&self, execution_id: Uuid)
    -> ArgusResult<Option<WorkspaceRecord>>;

    /// Update workspace status
    async fn update_status(
        &self,
        workspace_id: Uuid,
        status: &str,
        failure_reason: Option<&str>,
    ) -> ArgusResult<()>;

    /// Mark workspace as active (after successful creation)
    async fn mark_active(&self, workspace_id: Uuid) -> ArgusResult<()>;

    /// Mark workspace as completed
    async fn mark_completed(&self, workspace_id: Uuid) -> ArgusResult<()>;

    /// Mark workspace as failed
    async fn mark_failed(&self, workspace_id: Uuid, reason: &str) -> ArgusResult<()>;

    /// Mark workspace as needing manual recovery: the DB record exists but
    /// the on-disk worktree failed live verification (missing, not a real
    /// worktree, wrong branch, or HEAD drift). Phase 3 never auto-repairs
    /// this — a human (or a later phase) decides what happens next.
    async fn mark_recovery_needed(&self, workspace_id: Uuid, reason: &str) -> ArgusResult<()> {
        self.update_status(workspace_id, "recovery_needed", Some(reason))
            .await
    }

    /// List workspaces needing recovery
    async fn list_recoverable(&self) -> ArgusResult<Vec<WorkspaceRecord>>;

    /// Get active workspace for task (if any)
    async fn get_active_for_task(&self, task_id: Uuid) -> ArgusResult<Option<WorkspaceRecord>>;
}

/// PostgreSQL workspace repository
pub struct PostgresWorkspaceRepository {
    pool: PgPool,
}

impl PostgresWorkspaceRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl WorkspaceRepository for PostgresWorkspaceRepository {
    async fn ensure_workspace(
        &self,
        execution_id: Uuid,
        project_id: Uuid,
        task_id: Uuid,
        repository_path: String,
        worktree_path: String,
        branch_name: String,
        base_revision: String,
    ) -> ArgusResult<WorkspaceRecord> {
        // Use UPSERT (INSERT ... ON CONFLICT) for idempotency
        // If execution_id already exists, return existing record
        let query = format!(
            r#"
            INSERT INTO task_workspaces (
                workspace_id, project_id, task_id, execution_id,
                repository_path, worktree_path, branch_name, base_revision,
                status, created_at
            )
            VALUES (
                gen_random_uuid(), $1, $2, $3,
                $4, $5, $6, $7,
                'preparing', NOW()
            )
            ON CONFLICT (execution_id) DO UPDATE SET
                workspace_id = task_workspaces.workspace_id
            RETURNING {SELECT_COLUMNS}
            "#
        );
        let record = sqlx::query_as::<_, WorkspaceRow>(&query)
            .bind(project_id)
            .bind(task_id)
            .bind(execution_id)
            .bind(repository_path)
            .bind(worktree_path)
            .bind(branch_name)
            .bind(base_revision)
            .fetch_one(&self.pool)
            .await
            .map_err(ArgusError::Database)?;

        Ok(row_to_record(record))
    }

    async fn get_workspace(&self, workspace_id: Uuid) -> ArgusResult<Option<WorkspaceRecord>> {
        let query = format!("SELECT {SELECT_COLUMNS} FROM task_workspaces WHERE workspace_id = $1");
        let record = sqlx::query_as::<_, WorkspaceRow>(&query)
            .bind(workspace_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(ArgusError::Database)?;

        Ok(record.map(row_to_record))
    }

    async fn get_by_execution_id(
        &self,
        execution_id: Uuid,
    ) -> ArgusResult<Option<WorkspaceRecord>> {
        let query = format!("SELECT {SELECT_COLUMNS} FROM task_workspaces WHERE execution_id = $1");
        let record = sqlx::query_as::<_, WorkspaceRow>(&query)
            .bind(execution_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(ArgusError::Database)?;

        Ok(record.map(row_to_record))
    }

    async fn update_status(
        &self,
        workspace_id: Uuid,
        status: &str,
        failure_reason: Option<&str>,
    ) -> ArgusResult<()> {
        sqlx::query(
            r#"
            UPDATE task_workspaces
            SET status = $2, failure_reason = $3
            WHERE workspace_id = $1
            "#,
        )
        .bind(workspace_id)
        .bind(status)
        .bind(failure_reason)
        .execute(&self.pool)
        .await
        .map_err(ArgusError::Database)?;

        Ok(())
    }

    async fn mark_active(&self, workspace_id: Uuid) -> ArgusResult<()> {
        sqlx::query(
            r#"
            UPDATE task_workspaces
            SET status = 'active', activated_at = NOW()
            WHERE workspace_id = $1
            "#,
        )
        .bind(workspace_id)
        .execute(&self.pool)
        .await
        .map_err(ArgusError::Database)?;

        Ok(())
    }

    async fn mark_completed(&self, workspace_id: Uuid) -> ArgusResult<()> {
        sqlx::query(
            r#"
            UPDATE task_workspaces
            SET status = 'completed', completed_at = NOW()
            WHERE workspace_id = $1
            "#,
        )
        .bind(workspace_id)
        .execute(&self.pool)
        .await
        .map_err(ArgusError::Database)?;

        Ok(())
    }

    async fn mark_failed(&self, workspace_id: Uuid, reason: &str) -> ArgusResult<()> {
        sqlx::query(
            r#"
            UPDATE task_workspaces
            SET status = 'failed', failure_reason = $2, completed_at = NOW()
            WHERE workspace_id = $1
            "#,
        )
        .bind(workspace_id)
        .bind(reason)
        .execute(&self.pool)
        .await
        .map_err(ArgusError::Database)?;

        Ok(())
    }

    async fn list_recoverable(&self) -> ArgusResult<Vec<WorkspaceRecord>> {
        let query = format!(
            r#"
            SELECT {SELECT_COLUMNS}
            FROM task_workspaces
            WHERE status IN ('preparing', 'active', 'recovery_needed')
            "#
        );
        let records = sqlx::query_as::<_, WorkspaceRow>(&query)
            .fetch_all(&self.pool)
            .await
            .map_err(ArgusError::Database)?;

        Ok(records.into_iter().map(row_to_record).collect())
    }

    async fn get_active_for_task(&self, task_id: Uuid) -> ArgusResult<Option<WorkspaceRecord>> {
        let query = format!(
            r#"
            SELECT {SELECT_COLUMNS}
            FROM task_workspaces
            WHERE task_id = $1 AND status = 'active'
            ORDER BY created_at DESC
            LIMIT 1
            "#
        );
        let record = sqlx::query_as::<_, WorkspaceRow>(&query)
            .bind(task_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(ArgusError::Database)?;

        Ok(record.map(row_to_record))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workspace_record_structure() {
        let ws = WorkspaceRecord {
            workspace_id: Uuid::new_v4(),
            project_id: Uuid::new_v4(),
            task_id: Uuid::new_v4(),
            execution_id: Uuid::new_v4(),
            repository_path: "/repo".to_string(),
            worktree_path: "/workspace/task".to_string(),
            branch_name: "argus/task/123/456".to_string(),
            base_revision: "abc123def456".to_string(),
            status: "active".to_string(),
            failure_reason: None,
            created_at: Utc::now(),
            activated_at: Some(Utc::now()),
            completed_at: None,
        };

        assert_eq!(ws.status, "active");
        assert!(ws.failure_reason.is_none());
    }
}
