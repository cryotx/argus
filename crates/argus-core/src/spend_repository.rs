//! Persistent spend ledger for Agent cost tracking.
//!
//! Single source of truth for all execution costs. Prevents double-counting
//! across restarts, retries, and result replays.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[allow(unused_imports)]
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::types::ArgusResult;

/// Record of a single Agent execution's cost.
#[derive(Clone, Debug)]
pub struct SpendRecord {
    pub id: Uuid,
    pub task_id: Uuid,
    pub project_id: Uuid,
    /// Stable execution identifier for idempotency
    pub execution_id: Option<Uuid>,
    /// Which attempt of this execution (1-indexed)
    pub attempt_number: u32,
    /// Agent/Driver type (claude_code, codex, kimi, etc.)
    pub agent_type: String,
    /// Model used (claude-opus-5, gpt-4, etc.)
    pub model: String,
    /// Provider name
    pub provider: String,
    /// External session ID from the agent (for audit trail)
    pub external_session_id: Option<String>,
    /// Input tokens consumed
    pub input_tokens: Option<u32>,
    /// Output tokens consumed
    pub output_tokens: Option<u32>,
    /// Total tokens
    pub total_tokens: Option<u32>,
    /// USD cost (None = unknown/not reported)
    pub cost_usd: Option<f64>,
    /// Execution duration in milliseconds
    pub duration_ms: Option<u32>,
    /// Whether execution succeeded
    pub success: bool,
    /// Error message if failed
    pub error: Option<String>,
    /// When recorded
    pub created_at: DateTime<Utc>,
}

/// Trait for persistent spend tracking.
#[async_trait]
pub trait SpendRepository: Send + Sync {
    /// Record a single Agent execution's spend.
    ///
    /// Idempotent based on (execution_id, attempt_number, task_id).
    /// Returns Ok(true) if newly inserted, Ok(false) if already existed.
    async fn record_spend(&self, record: SpendRecord) -> ArgusResult<bool>;

    /// Get total USD spent today (UTC day) for a project.
    ///
    /// Includes successful and failed executions.
    /// Returns Some(amount) or None if no records.
    async fn spent_today_usd(&self, project_id: Uuid) -> ArgusResult<Option<f64>>;

    /// Get total USD spent this month for a project.
    async fn spent_this_month_usd(&self, project_id: Uuid) -> ArgusResult<Option<f64>>;

    /// Get all spend records for a task (across all attempts).
    async fn task_spend(&self, task_id: Uuid) -> ArgusResult<Vec<SpendRecord>>;

    /// Get spend records for a specific execution.
    async fn execution_spend(&self, execution_id: Uuid) -> ArgusResult<Vec<SpendRecord>>;

    /// Get a single spend record by ID.
    async fn get_spend(&self, spend_id: Uuid) -> ArgusResult<Option<SpendRecord>>;
}

/// PostgreSQL implementation of SpendRepository.
pub struct PostgresSpendRepository {
    pool: sqlx::PgPool,
}

impl PostgresSpendRepository {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl SpendRepository for PostgresSpendRepository {
    async fn record_spend(&self, record: SpendRecord) -> ArgusResult<bool> {
        // Bind as native `Uuid`, not `.to_string()`: Postgres has no
        // implicit or assignment cast from `text` to `uuid` for a primary
        // key target column (unlike some other columns), so a text-bound id
        // here fails the INSERT outright with "column "id" is of type uuid
        // but expression is of type text".
        let id = record.id;
        let task_id = record.task_id;
        let project_id = record.project_id;
        let execution_id = record.execution_id;
        let attempt = i16::try_from(record.attempt_number).unwrap_or(1);

        // Use INSERT ... ON CONFLICT for idempotency.
        //
        // `model_class` is `llm_usage`'s pre-V0.37 NOT NULL column (what kind
        // of call this was — reasoning/coding/fast/...) that `SpendRecord`
        // has no equivalent field for; every row this executor writes is an
        // Agent doing a coding task, so the literal `'coding'` is accurate
        // here, not a placeholder.
        let result = sqlx::query(
            r#"
            INSERT INTO llm_usage (
                id, task_id, project_id, execution_id, attempt_number,
                agent_type, model, model_class, provider, external_session_id,
                input_tokens, output_tokens, total_tokens, cost_usd,
                duration_ms, success, error, created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, 'coding', $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)
            ON CONFLICT (execution_id, attempt_number, task_id) WHERE execution_id IS NOT NULL DO NOTHING
            "#,
        )
        .bind(id)
        .bind(task_id)
        .bind(project_id)
        .bind(execution_id)
        .bind(attempt)
        .bind(&record.agent_type)
        .bind(&record.model)
        .bind(&record.provider)
        .bind(&record.external_session_id)
        // `input_tokens`/`output_tokens`/`total_tokens` are NOT NULL columns
        // predating per-Agent token accounting; not every driver reports
        // them (e.g. a failed run, or a CLI that only gives cost). `0` is a
        // safe, honest sentinel for "unknown" — it never inflates spend,
        // since budget enforcement keys off `cost_usd`, not token counts.
        .bind(
            record
                .input_tokens
                .map(|t| i32::try_from(t).unwrap_or(i32::MAX))
                .unwrap_or(0),
        )
        .bind(
            record
                .output_tokens
                .map(|t| i32::try_from(t).unwrap_or(i32::MAX))
                .unwrap_or(0),
        )
        .bind(
            record
                .total_tokens
                .map(|t| i32::try_from(t).unwrap_or(i32::MAX))
                .unwrap_or(0),
        )
        // `cost_usd` is also a NOT NULL column (default 0.0); binding
        // `Option::None` explicitly would override the default with NULL and
        // violate the constraint. `0.0` is the honest "unknown cost" sentinel
        // — exactly like the token columns above — and a driver that does
        // report cost always wins over it.
        .bind(record.cost_usd.unwrap_or(0.0))
        .bind(
            record
                .duration_ms
                .map(|d| i32::try_from(d).unwrap_or(i32::MAX)),
        )
        .bind(record.success)
        .bind(&record.error)
        .bind(record.created_at)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    async fn spent_today_usd(&self, project_id: Uuid) -> ArgusResult<Option<f64>> {
        let row: (Option<f64>,) = sqlx::query_as(
            r#"
            SELECT COALESCE(SUM(cost_usd), 0.0)::float8
            FROM llm_usage
            WHERE project_id = $1
              AND DATE_TRUNC('day', created_at)::DATE = CURRENT_DATE
              AND cost_usd IS NOT NULL
            "#,
        )
        .bind(project_id)
        .fetch_one(&self.pool)
        .await?;

        match row.0 {
            Some(amount) if amount > 0.0 => Ok(Some(amount)),
            _ => Ok(None),
        }
    }

    async fn spent_this_month_usd(&self, project_id: Uuid) -> ArgusResult<Option<f64>> {
        let row: (Option<f64>,) = sqlx::query_as(
            r#"
            SELECT COALESCE(SUM(cost_usd), 0.0)::float8
            FROM llm_usage
            WHERE project_id = $1
              AND DATE_TRUNC('month', created_at)::DATE = DATE_TRUNC('month', CURRENT_DATE)
              AND cost_usd IS NOT NULL
            "#,
        )
        .bind(project_id)
        .fetch_one(&self.pool)
        .await?;

        match row.0 {
            Some(amount) if amount > 0.0 => Ok(Some(amount)),
            _ => Ok(None),
        }
    }

    async fn task_spend(&self, task_id: Uuid) -> ArgusResult<Vec<SpendRecord>> {
        let rows = sqlx::query(
            r#"
            SELECT
                id, task_id, project_id, execution_id, attempt_number,
                agent_type, model, provider, external_session_id,
                input_tokens, output_tokens, total_tokens, cost_usd::float8 AS cost_usd,
                duration_ms, success, error, created_at
            FROM llm_usage
            WHERE task_id = $1
            ORDER BY created_at DESC
            "#,
        )
        .bind(task_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .filter_map(|row| extract_spend_record(&row).ok())
            .collect())
    }

    async fn execution_spend(&self, execution_id: Uuid) -> ArgusResult<Vec<SpendRecord>> {
        let rows = sqlx::query(
            r#"
            SELECT
                id, task_id, project_id, execution_id, attempt_number,
                agent_type, model, provider, external_session_id,
                input_tokens, output_tokens, total_tokens, cost_usd::float8 AS cost_usd,
                duration_ms, success, error, created_at
            FROM llm_usage
            WHERE execution_id = $1
            ORDER BY attempt_number ASC, created_at ASC
            "#,
        )
        .bind(execution_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .filter_map(|row| extract_spend_record(&row).ok())
            .collect())
    }

    async fn get_spend(&self, spend_id: Uuid) -> ArgusResult<Option<SpendRecord>> {
        let row = sqlx::query(
            r#"
            SELECT
                id, task_id, project_id, execution_id, attempt_number,
                agent_type, model, provider, external_session_id,
                input_tokens, output_tokens, total_tokens, cost_usd::float8 AS cost_usd,
                duration_ms, success, error, created_at
            FROM llm_usage
            WHERE id = $1
            "#,
        )
        .bind(spend_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.and_then(|r| extract_spend_record(&r).ok()))
    }
}

/// Extract a SpendRecord from a sqlx PostgreSQL Row.
///
/// Handles all field conversions from PostgreSQL types to Rust types.
/// Returns error if any required field is missing or cannot be converted.
fn extract_spend_record(row: &sqlx::postgres::PgRow) -> Result<SpendRecord, String> {
    let id: Uuid = row.get("id");
    let task_id: Uuid = row.get("task_id");
    let project_id: Uuid = row.get("project_id");
    let execution_id: Option<Uuid> = row.get("execution_id");
    let attempt_number: i16 = row.get("attempt_number");
    let agent_type: Option<String> = row.get("agent_type");
    let model: String = row.get("model");
    let provider: String = row.get("provider");
    let external_session_id: Option<String> = row.get("external_session_id");
    let input_tokens: Option<i32> = row.get("input_tokens");
    let output_tokens: Option<i32> = row.get("output_tokens");
    let total_tokens: Option<i32> = row.get("total_tokens");
    let cost_usd: Option<f64> = row.get("cost_usd");
    let duration_ms: Option<i32> = row.get("duration_ms");
    let success: bool = row.get("success");
    let error: Option<String> = row.get("error");
    let created_at: DateTime<Utc> = row.get("created_at");

    Ok(SpendRecord {
        id,
        task_id,
        project_id,
        execution_id,
        attempt_number: attempt_number as u32,
        agent_type: agent_type.unwrap_or_default(),
        model,
        provider,
        external_session_id,
        input_tokens: input_tokens.map(|t| t as u32),
        output_tokens: output_tokens.map(|t| t as u32),
        total_tokens: total_tokens.map(|t| t as u32),
        cost_usd,
        duration_ms: duration_ms.map(|d| d as u32),
        success,
        error,
        created_at,
    })
}
