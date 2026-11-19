//! Persistence boundary for scheduler tasks.

use async_trait::async_trait;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::task_types::{Priority, Task, TaskState};
use crate::types::ArgusResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateTaskResult {
    Created(Uuid),
    Existing(Uuid),
}

#[async_trait]
pub trait TaskRepository: Send + Sync {
    async fn create_task(
        &self,
        task: &Task,
        request_key: Option<&str>,
    ) -> ArgusResult<CreateTaskResult>;
    async fn transition_task(
        &self,
        expected_state: TaskState,
        task: &Task,
        checkpoint_context: serde_json::Value,
    ) -> ArgusResult<bool>;
    async fn load_tasks(&self) -> ArgusResult<Vec<Task>>;
    /// Load only non-terminal tasks (for recovery on daemon startup).
    ///
    /// Terminal states (Completed, Failed, Cancelled, TimedOut) are excluded.
    /// This prevents unbounded memory growth from loading hundreds/thousands of historical tasks.
    async fn load_active_tasks(&self) -> ArgusResult<Vec<Task>>;
    async fn find_task_id_by_request_key(&self, request_key: &str) -> ArgusResult<Option<Uuid>>;
}

#[derive(Clone)]
pub struct PostgresTaskRepository {
    pool: PgPool,
}

impl PostgresTaskRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn checkpoint_progress(state: TaskState) -> f32 {
    match state {
        TaskState::Pending | TaskState::Blocked | TaskState::Paused => 0.0,
        TaskState::Ready | TaskState::Queued | TaskState::Assigned => 0.1,
        TaskState::Running => 0.5,
        TaskState::Testing => 0.9,
        TaskState::Completed => 1.0,
        TaskState::Failed | TaskState::Cancelled | TaskState::TimedOut => 0.0,
    }
}

fn database_priority(priority: Priority) -> &'static str {
    match priority {
        Priority::Critical => "critical",
        Priority::Urgent | Priority::High => "high",
        Priority::Normal => "medium",
        Priority::Low => "low",
        Priority::Background => "background",
    }
}

#[async_trait]
impl TaskRepository for PostgresTaskRepository {
    async fn create_task(
        &self,
        task: &Task,
        request_key: Option<&str>,
    ) -> ArgusResult<CreateTaskResult> {
        let serialized = serde_json::to_value(task)?;
        let context = json!({ "scheduler_task": serialized });
        let output = task.output.as_ref().map(serde_json::to_value).transpose()?;
        let error = task.error.as_ref().map(serde_json::to_string).transpose()?;
        let timeout_seconds = i32::try_from(task.timeout.as_secs()).unwrap_or(i32::MAX);
        let estimated_seconds = task
            .estimated_minutes
            .map(|minutes| i32::try_from(minutes.saturating_mul(60)).unwrap_or(i32::MAX));
        let retry_count = i32::try_from(task.attempt.saturating_sub(1)).unwrap_or(i32::MAX);
        let max_retries = i32::try_from(task.max_attempts).unwrap_or(i32::MAX);

        let mut tx = self.pool.begin().await?;
        if let Some(request_key) = request_key {
            sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
                .bind(request_key)
                .execute(&mut *tx)
                .await?;

            if let Some((task_id,)) = sqlx::query_as::<_, (Uuid,)>(
                "SELECT task_id FROM task_request_keys WHERE request_key = $1",
            )
            .bind(request_key)
            .fetch_optional(&mut *tx)
            .await?
            {
                tx.commit().await?;
                return Ok(CreateTaskResult::Existing(task_id));
            }
        }

        let inserted = sqlx::query(
            r#"
            INSERT INTO tasks (
                id, project_id, milestone_id, title, description, task_type,
                state, priority, effective_priority, assigned_worker_id,
                retry_count, max_retries, timeout_seconds, estimated_seconds,
                output, error, context, commit_sha, created_at, updated_at,
                started_at, completed_at, execution_id, resume_count
            )
            VALUES (
                $1, $2, $3, $4, $5, $6,
                $7, $8::task_priority, $9, $10,
                $11, $12, $13, $14,
                $15, $16, $17, $18, $19, $20,
                $21, $22, $23, $24
            )
            ON CONFLICT (id) DO NOTHING
            "#,
        )
        .bind(task.id)
        .bind(task.project_id)
        .bind(task.milestone_id)
        .bind(&task.title)
        .bind(&task.description)
        .bind(&task.task_type)
        .bind(task.state)
        .bind(database_priority(task.priority))
        .bind(task.effective_priority)
        .bind(task.assigned_worker_id)
        .bind(retry_count)
        .bind(max_retries)
        .bind(timeout_seconds)
        .bind(estimated_seconds)
        .bind(&output)
        .bind(error)
        .bind(context)
        .bind(
            task.output
                .as_ref()
                .and_then(|value| value.commit_sha.as_deref()),
        )
        .bind(task.created_at)
        .bind(task.updated_at)
        .bind(task.started_at)
        .bind(task.completed_at)
        .bind(task.execution_id)
        .bind(i32::try_from(task.resume_count).unwrap_or(i32::MAX))
        .execute(&mut *tx)
        .await?;

        if inserted.rows_affected() == 0 {
            tx.commit().await?;
            return Ok(CreateTaskResult::Existing(task.id));
        }

        sqlx::query("DELETE FROM task_dependencies WHERE task_id = $1")
            .bind(task.id)
            .execute(&mut *tx)
            .await?;

        for dependency_id in &task.depends_on {
            sqlx::query(
                "INSERT INTO task_dependencies (task_id, depends_on_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
            )
            .bind(task.id)
            .bind(dependency_id)
            .execute(&mut *tx)
            .await?;
        }

        sqlx::query(
            r#"
            INSERT INTO task_checkpoints (task_id, state, progress, worker_output, context)
            VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(task.id)
        .bind(task.state)
        .bind(checkpoint_progress(task.state))
        .bind(output)
        .bind(json!({"reason": "task_created", "scheduler_task": task}))
        .execute(&mut *tx)
        .await?;

        if let Some(request_key) = request_key {
            sqlx::query(
                r#"
                INSERT INTO task_request_keys (request_key, task_id)
                VALUES ($1, $2)
                "#,
            )
            .bind(request_key)
            .bind(task.id)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(CreateTaskResult::Created(task.id))
    }

    async fn transition_task(
        &self,
        expected_state: TaskState,
        task: &Task,
        checkpoint_context: serde_json::Value,
    ) -> ArgusResult<bool> {
        let serialized = serde_json::to_value(task)?;
        let context = json!({ "scheduler_task": serialized });
        let output = task.output.as_ref().map(serde_json::to_value).transpose()?;
        let error = task.error.as_ref().map(serde_json::to_string).transpose()?;
        let retry_count = i32::try_from(task.attempt.saturating_sub(1)).unwrap_or(i32::MAX);

        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            r#"
            UPDATE tasks
            SET state = $3,
                effective_priority = $4,
                assigned_worker_id = $5,
                retry_count = $6,
                output = $7,
                error = $8,
                context = $9,
                commit_sha = $10,
                updated_at = $11,
                started_at = $12,
                completed_at = $13,
                execution_id = $14,
                resume_count = $15
            WHERE id = $1 AND state = $2
            "#,
        )
        .bind(task.id)
        .bind(expected_state)
        .bind(task.state)
        .bind(task.effective_priority)
        .bind(task.assigned_worker_id)
        .bind(retry_count)
        .bind(output.clone())
        .bind(error)
        .bind(context)
        .bind(
            task.output
                .as_ref()
                .and_then(|value| value.commit_sha.as_deref()),
        )
        .bind(task.updated_at)
        .bind(task.started_at)
        .bind(task.completed_at)
        .bind(task.execution_id)
        .bind(i32::try_from(task.resume_count).unwrap_or(i32::MAX))
        .execute(&mut *tx)
        .await?;

        if result.rows_affected() == 0 {
            tx.rollback().await?;
            return Ok(false);
        }

        sqlx::query(
            r#"
            INSERT INTO task_checkpoints (task_id, state, progress, worker_output, context)
            VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(task.id)
        .bind(task.state)
        .bind(checkpoint_progress(task.state))
        .bind(output)
        .bind(checkpoint_context)
        .execute(&mut *tx)
        .await?;

        if let Some(execution_id) = task.execution_id {
            let status = match task.state {
                TaskState::Queued | TaskState::Assigned | TaskState::Running => "active",
                TaskState::Ready if task.resume_count > 0 => "resuming",
                TaskState::Completed => "completed",
                TaskState::Failed | TaskState::Cancelled | TaskState::TimedOut => "failed",
                _ => "checkpointed",
            };
            sqlx::query(
                r#"
                INSERT INTO task_execution_attempts (
                    execution_id, task_id, attempt, status, updated_at
                )
                VALUES ($1, $2, $3, $4, NOW())
                ON CONFLICT (execution_id) DO UPDATE SET
                    status = EXCLUDED.status,
                    updated_at = NOW()
                "#,
            )
            .bind(execution_id)
            .bind(task.id)
            .bind(i32::try_from(task.attempt).unwrap_or(i32::MAX))
            .bind(status)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(true)
    }

    async fn load_tasks(&self) -> ArgusResult<Vec<Task>> {
        let rows: Vec<(
            serde_json::Value,
            Option<TaskState>,
            Option<serde_json::Value>,
        )> = sqlx::query_as(
            r#"
            SELECT
                tasks.context -> 'scheduler_task',
                checkpoint.state,
                checkpoint.worker_output
            FROM tasks
            LEFT JOIN LATERAL (
                SELECT state, worker_output
                FROM task_checkpoints
                WHERE task_id = tasks.id
                ORDER BY created_at DESC, id DESC
                LIMIT 1
            ) checkpoint ON true
            WHERE tasks.context ? 'scheduler_task'
            ORDER BY tasks.created_at
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|(value, checkpoint_state, checkpoint_output)| {
                let mut task: Task = serde_json::from_value(value)?;
                if let Some(state) = checkpoint_state {
                    task.state = state;
                }
                if let Some(output) = checkpoint_output {
                    task.output = Some(serde_json::from_value(output)?);
                }
                Ok(task)
            })
            .collect()
    }

    async fn load_active_tasks(&self) -> ArgusResult<Vec<Task>> {
        let rows: Vec<(
            serde_json::Value,
            Option<TaskState>,
            Option<serde_json::Value>,
        )> = sqlx::query_as(
            r#"
            SELECT
                tasks.context -> 'scheduler_task',
                checkpoint.state,
                checkpoint.worker_output
            FROM tasks
            LEFT JOIN LATERAL (
                SELECT state, worker_output
                FROM task_checkpoints
                WHERE task_id = tasks.id
                ORDER BY created_at DESC, id DESC
                LIMIT 1
            ) checkpoint ON true
            WHERE tasks.context ? 'scheduler_task'
                AND (
                    checkpoint.state IS NULL  -- No checkpoint = Pending
                    OR checkpoint.state IN ('ready', 'queued', 'assigned', 'running', 'testing', 'paused', 'blocked', 'pending')
                    -- Exclude terminal states: completed, failed, cancelled, timed_out
                )
            ORDER BY tasks.created_at
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|(value, checkpoint_state, checkpoint_output)| {
                let mut task: Task = serde_json::from_value(value)?;
                if let Some(state) = checkpoint_state {
                    task.state = state;
                }
                if let Some(output) = checkpoint_output {
                    task.output = Some(serde_json::from_value(output)?);
                }
                Ok(task)
            })
            .collect()
    }

    async fn find_task_id_by_request_key(&self, request_key: &str) -> ArgusResult<Option<Uuid>> {
        let row: Option<(Uuid,)> =
            sqlx::query_as("SELECT task_id FROM task_request_keys WHERE request_key = $1")
                .bind(request_key)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|(task_id,)| task_id))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use sqlx::postgres::PgPoolOptions;

    use super::*;
    use crate::task_scheduler::TaskScheduler;
    use crate::task_types::{TaskBuilder, TaskState};

    #[tokio::test]
    #[ignore = "requires postgres and migrations"]
    async fn test_postgres_scheduler_restore_and_idempotency() {
        let database_url = std::env::var("ARGUS_DATABASE_URL")
            .or_else(|_| std::env::var("DATABASE_URL"))
            .expect("set ARGUS_DATABASE_URL or DATABASE_URL");
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&database_url)
            .await
            .expect("connect postgres");

        sqlx::migrate!("../../migrations")
            .run(&pool)
            .await
            .expect("run migrations");

        let project_id = Uuid::new_v4();
        let project_name = format!("scheduler-restore-{}", &project_id.to_string()[..8]);
        sqlx::query(
            r#"
            INSERT INTO projects (id, name, description, git_local_path, active, priority)
            VALUES ($1, $2, 'scheduler repository test', $3, true, 'medium')
            "#,
        )
        .bind(project_id)
        .bind(&project_name)
        .bind(format!("/tmp/argus/projects/{project_name}"))
        .execute(&pool)
        .await
        .expect("insert project");

        let repository = Arc::new(PostgresTaskRepository::new(pool.clone()));
        let scheduler = TaskScheduler::new_with_repository(repository.clone(), None);
        let request_key = format!("restore-test-{project_id}");
        let task_id = scheduler
            .add_task_idempotent(
                TaskBuilder::new(project_id, "Persisted Task").build(),
                &request_key,
            )
            .await;
        assert!(scheduler.transition(task_id, TaskState::Queued).await);
        assert!(scheduler.transition(task_id, TaskState::Assigned).await);
        assert!(scheduler.transition(task_id, TaskState::Running).await);

        let restored_scheduler = TaskScheduler::new_with_repository(repository, None);
        let recovery_report = restored_scheduler.restore().await.unwrap();
        assert_eq!(recovery_report.active_tasks, 1);
        let restored = restored_scheduler.get_task(task_id).await.unwrap();
        assert_eq!(restored.state, TaskState::Ready);
        assert_eq!(restored_scheduler.queue_len().await, 1);
        assert_eq!(
            restored_scheduler
                .get_task_id_by_request_key(&request_key)
                .await,
            Some(task_id)
        );

        sqlx::query("DELETE FROM projects WHERE id = $1")
            .bind(project_id)
            .execute(&pool)
            .await
            .expect("delete project");
    }
}
