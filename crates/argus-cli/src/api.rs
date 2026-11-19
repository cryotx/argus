//! API module for web dashboard

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
};

use argus_core::context::{
    ContextFormat, ContextPurpose, ContextRequest, ContextRole, ContextSection, render_markdown,
};
use chrono::Utc;
use futures::stream::{self, Stream};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::convert::Infallible;
use std::sync::Arc;
use uuid::Uuid;

use argus_core::budget_service::{BudgetPolicy, BudgetService};
use argus_core::event_bus::EventBus;
use argus_core::events::event_name;
use argus_core::intelligence::{
    FailureClass, FailureInput, ModelTier, OutcomeInput, PostgresTaskIntelligenceRepository,
    TaskIntelligenceService, TaskKindTierStats,
};
use argus_core::planner::{
    CreatePlanningRequestInput, PlannerHarness, PlanningKind, PlanningResult,
    PostgresPlanningRepository, RequestedOutput,
};
use argus_core::project_state::{
    ActivityType, ImprovementPriority, ImprovementStatus, NewActivity, NewBlocker, NewDecision,
    NewImprovement, NewMilestone, NewRisk, NewWorkItem, PostgresProjectStateRepository,
    ProjectSnapshot, ProjectStateRepository, ProjectStateService, RiskLikelihood, RiskSeverity,
};
use argus_core::provider::{
    AuthKind, AuthStatus, CliProviderAdapter, FallbackPolicy, NewAuthProfile,
    PostgresProviderRepository, ProjectProviderPolicy, ProviderAdapter, ProviderRoutingInput,
    ProviderService, RuntimeProvider, SecretReference, UnknownQuotaProvider,
};
use argus_core::spend_repository::PostgresSpendRepository;
use argus_core::task_scheduler::TaskScheduler;
use argus_core::task_types::{Priority, TaskBuilder, TaskErrorType};

// ============================================================
// Application State
// ============================================================

#[derive(Clone)]
pub struct ApiState {
    pub started_at: std::time::Instant,
    pub db_pool: PgPool,
    pub scheduler: Arc<TaskScheduler>,
    pub event_bus: Arc<EventBus>,
    /// V0.39 — structured Project State. Always attached: the API's project
    /// endpoints are read-mostly views over it.
    pub project_state: Arc<ProjectStateService>,
    /// V0.42 — deterministic task classification and routing.
    pub task_intelligence: Arc<TaskIntelligenceService>,
    /// V0.43 — provider-neutral planner harness.
    pub planner: Arc<PlannerHarness>,
    /// V0.44 — provider/auth/quota routing.
    pub provider: Arc<ProviderService>,
    pub evaluation: Arc<argus_core::evaluation::EvaluationHarness>,
    pub runtime_config: serde_json::Value,
}

impl ApiState {
    #[allow(dead_code)]
    pub fn new(db_pool: PgPool, scheduler: Arc<TaskScheduler>) -> Self {
        let (project_state, task_intelligence, planner, provider) =
            build_project_services(&db_pool);
        Self {
            started_at: std::time::Instant::now(),
            evaluation: Arc::new(argus_core::evaluation::EvaluationHarness::new(
                db_pool.clone(),
            )),
            runtime_config: safe_config(&argus_core::config::ArgusConfig::default()),
            db_pool,
            scheduler,
            event_bus: Arc::new(EventBus::new()),
            project_state,
            task_intelligence,
            planner,
            provider,
        }
    }

    pub fn with_event_bus(
        db_pool: PgPool,
        scheduler: Arc<TaskScheduler>,
        event_bus: Arc<EventBus>,
    ) -> Self {
        let (project_state, task_intelligence, planner, provider) =
            build_project_services(&db_pool);
        Self {
            started_at: std::time::Instant::now(),
            evaluation: Arc::new(argus_core::evaluation::EvaluationHarness::new(
                db_pool.clone(),
            )),
            runtime_config: safe_config(&argus_core::config::ArgusConfig::default()),
            db_pool,
            scheduler,
            event_bus,
            project_state,
            task_intelligence,
            planner,
            provider,
        }
    }
}

type ProjectServices = (
    Arc<ProjectStateService>,
    Arc<TaskIntelligenceService>,
    Arc<PlannerHarness>,
    Arc<ProviderService>,
);

fn build_project_services(pool: &PgPool) -> ProjectServices {
    let state_repo: Arc<dyn ProjectStateRepository> =
        Arc::new(PostgresProjectStateRepository::new(pool.clone()));
    let project_state = Arc::new(ProjectStateService::new(state_repo.clone()));
    let task_intelligence = Arc::new(TaskIntelligenceService::new(
        state_repo.clone(),
        Arc::new(PostgresTaskIntelligenceRepository::new(pool.clone())),
    ));
    let planner = Arc::new(PlannerHarness::new(
        state_repo,
        Arc::new(PostgresPlanningRepository::new(pool.clone())),
        Some(task_intelligence.clone()),
    ));

    let budget_gate = Arc::new(BudgetService::new(
        Arc::new(PostgresSpendRepository::new(pool.clone())),
        BudgetPolicy::default(),
    ));
    let adapters: Vec<Arc<dyn ProviderAdapter>> = vec![
        Arc::new(CliProviderAdapter::new(RuntimeProvider::ClaudeCodeCli)),
        Arc::new(CliProviderAdapter::new(RuntimeProvider::CodexCli)),
        Arc::new(CliProviderAdapter::new(RuntimeProvider::OpenCodeCli)),
    ];
    let provider = Arc::new(ProviderService::new(
        Arc::new(PostgresProviderRepository::new(pool.clone())),
        Arc::new(UnknownQuotaProvider),
        Some(budget_gate),
        adapters,
    ));

    (project_state, task_intelligence, planner, provider)
}

// ============================================================
// Request/Response types
// ============================================================

#[derive(Debug, Serialize)]
pub struct SystemStatus {
    pub version: String,
    pub status: String,
    pub uptime_seconds: u64,
    pub database_connected: bool,
    pub workers_active: usize,
}

#[derive(Debug, Serialize)]
pub struct SchedulerStatsResponse {
    pub total_tasks: usize,
    pub by_state: std::collections::HashMap<String, usize>,
    pub retry_error_counts: std::collections::HashMap<String, usize>,
    pub queue_depth: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectResponse {
    pub id: String,
    pub name: String,
    pub description: String,
    pub git_repo_url: Option<String>,
    pub language: Option<String>,
    pub framework: Option<String>,
    pub status: String,
    pub priority: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateProjectRequest {
    pub name: String,
    pub description: Option<String>,
    pub git_repo_url: Option<String>,
    pub language: Option<String>,
    pub framework: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TaskResponse {
    pub id: String,
    pub project_id: String,
    pub project_name: Option<String>,
    pub title: String,
    pub description: String,
    pub task_type: String,
    pub status: String,
    pub priority: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateTaskRequest {
    pub project_id: String,
    pub title: String,
    pub description: Option<String>,
    pub task_type: Option<String>,
    pub priority: Option<String>,
    #[serde(default)]
    pub test_commands: Vec<String>,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct DeploymentResponse {
    pub id: String,
    pub project_id: String,
    pub project_name: Option<String>,
    pub version: String,
    pub status: String,
    pub environment: String,
    pub commit_sha: Option<String>,
    pub completed_at: Option<String>,
    pub health_check_passed: Option<bool>,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
pub struct LogEntry {
    pub id: String,
    pub level: String,
    pub source: String,
    pub message: String,
    pub timestamp: String,
}

#[derive(Debug, Serialize)]
pub struct LlmUsageEntry {
    pub id: String,
    pub provider: String,
    pub model: String,
    pub input_tokens: i32,
    pub output_tokens: i32,
    pub cost_usd: f64,
    pub timestamp: String,
}

#[derive(Debug, Deserialize)]
pub struct PaginationParams {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub project_id: Option<String>,
}

// ============================================================
// V0.39 Project State request/response types
// ============================================================

#[derive(Debug, Serialize)]
pub struct ProjectStateResponse {
    pub snapshot: ProjectSnapshot,
    /// Deterministic human-readable rendering — derived from the same
    /// structured snapshot, never generated by a model.
    pub status_text: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateMilestoneRequest {
    pub title: String,
    pub description: Option<String>,
    pub sequence: Option<i32>,
    pub parent_id: Option<String>,
    pub definition_of_done: Option<String>,
    pub external_key: Option<String>,
    pub weight: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct CreateWorkItemRequest {
    pub milestone_id: Option<String>,
    pub phase_id: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub priority: Option<String>,
    pub definition_of_done: Option<String>,
    pub external_key: Option<String>,
    pub weight: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct CreateDecisionRequest {
    pub title: String,
    pub context: Option<String>,
    pub decision: String,
    pub reason: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateRiskRequest {
    pub milestone_id: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub severity: Option<String>,
    pub likelihood: Option<String>,
    pub mitigation: Option<String>,
    pub owner: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateBlockerRequest {
    pub work_item_id: Option<String>,
    pub title: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateImprovementRequest {
    pub external_key: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub priority: Option<String>,
    pub status: Option<String>,
    pub source: Option<String>,
    pub why_deferred: Option<String>,
    pub required_before: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RecordActivityRequest {
    pub activity_type: String,
    pub entity_type: Option<String>,
    pub entity_id: Option<String>,
    pub summary: String,
}

// ============================================================
// V0.41 Context Compiler request types
// ============================================================

#[derive(Debug, Deserialize)]
pub struct ContextQuery {
    pub format: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CompileContextRequest {
    pub role: String,
    pub purpose: String,
    pub work_item_id: Option<String>,
    pub task_id: Option<String>,
    pub execution_id: Option<String>,
    pub max_items: Option<usize>,
    pub max_bytes: Option<usize>,
    pub estimated_token_budget: Option<usize>,
    pub requested_sections: Option<Vec<String>>,
    pub include_recent_activity: Option<bool>,
    pub include_artifacts: Option<bool>,
}

// ============================================================
// V0.42 Task Intelligence request types
// ============================================================

#[derive(Debug, Deserialize)]
pub struct ClassifyTaskRequest {
    pub work_item_id: String,
    pub role: String,
    pub purpose: String,
}

#[derive(Debug, Deserialize)]
pub struct RoutingOutcomeRequest {
    pub success: bool,
    pub failure_class: Option<String>,
    pub attempts: Option<u32>,
    pub duration_ms: Option<u32>,
    pub artifact_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RoutingFailureRequest {
    pub failure_class: Option<String>,
    pub error_type: Option<String>,
    pub attempts: Option<u32>,
    pub context_truncated: Option<bool>,
}

// ============================================================
// V0.43 Planner request types
// ============================================================

#[derive(Debug, Deserialize)]
pub struct CreatePlanningRequestBody {
    pub planning_kind: String,
    pub role: String,
    pub purpose: String,
    pub objective: String,
    pub constraints: Option<Vec<String>>,
    pub requested_outputs: Option<Vec<String>>,
    pub work_item_id: Option<String>,
}

// ============================================================
// V0.44 Provider request types
// ============================================================

#[derive(Debug, Deserialize)]
pub struct ProviderRoutingSelectRequest {
    pub routing_decision_id: String,
    pub role: String,
    pub purpose: String,
    pub estimated_cost_usd: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub struct RegisterAuthProfileRequest {
    pub provider_id: String,
    pub auth_kind: String,
    pub account_label: Option<String>,
    /// A credential *locator* (e.g. `env:OPENAI_API_KEY`), never a value.
    pub secret_reference: Option<String>,
    pub credential_source: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpsertProviderPolicyRequest {
    pub allowed_providers: Option<Vec<String>>,
    pub denied_providers: Option<Vec<String>>,
    pub subscription_first: Option<bool>,
    pub api_fallback: Option<String>,
    pub max_tier: Option<String>,
    pub preferred_runtime: Option<String>,
    pub allowed_models: Option<Vec<String>>,
    pub denied_models: Option<Vec<String>>,
}

/// Parse a UUID path/body value, returning a 400-shaped JSON error on failure.
fn parse_uuid_field(
    value: &str,
    field: &str,
) -> Result<Uuid, (StatusCode, Json<serde_json::Value>)> {
    Uuid::parse_str(value).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": format!("invalid {field}; expected UUID") })),
        )
    })
}

fn parse_optional_uuid(
    value: Option<&str>,
    field: &str,
) -> Result<Option<Uuid>, (StatusCode, Json<serde_json::Value>)> {
    match value {
        Some(v) => parse_uuid_field(v, field).map(Some),
        None => Ok(None),
    }
}

fn project_state_error(
    status: StatusCode,
    error: impl std::fmt::Display,
) -> (StatusCode, Json<serde_json::Value>) {
    (
        status,
        Json(serde_json::json!({ "error": error.to_string() })),
    )
}

// ============================================================
// Database Query Functions
// ============================================================

async fn fetch_projects(
    pool: &PgPool,
    limit: i64,
    offset: i64,
) -> Result<Vec<ProjectResponse>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (Uuid, String, Option<String>, Option<String>, Option<String>, Option<String>, bool, String, chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)>(
        "SELECT id, name, description, git_repo_url, language, framework, active, priority::text, created_at, updated_at
         FROM projects ORDER BY created_at DESC LIMIT $1 OFFSET $2"
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| ProjectResponse {
            id: r.0.to_string(),
            name: r.1,
            description: r.2.unwrap_or_default(),
            git_repo_url: r.3,
            language: r.4,
            framework: r.5,
            status: if r.6 { "active" } else { "inactive" }.to_string(),
            priority: r.7,
            created_at: r.8.to_rfc3339(),
            updated_at: r.9.to_rfc3339(),
        })
        .collect())
}

async fn create_project_db(
    pool: &PgPool,
    name: &str,
    description: Option<&str>,
    git_repo_url: Option<&str>,
    language: Option<&str>,
    framework: Option<&str>,
) -> Result<ProjectResponse, sqlx::Error> {
    let git_local_path = format!("/tmp/argus/projects/{name}");
    let row = sqlx::query_as::<_, (Uuid, String, Option<String>, Option<String>, Option<String>, Option<String>, bool, String, chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)>(
        "INSERT INTO projects (name, description, git_repo_url, git_local_path, language, framework, active, priority)
         VALUES ($1, $2, $3, $4, $5, $6, true, 'medium')
         RETURNING id, name, description, git_repo_url, language, framework, active, priority::text, created_at, updated_at"
    )
    .bind(name)
    .bind(description)
    .bind(git_repo_url)
    .bind(git_local_path)
    .bind(language)
    .bind(framework)
    .fetch_one(pool)
    .await?;

    Ok(ProjectResponse {
        id: row.0.to_string(),
        name: row.1,
        description: row.2.unwrap_or_default(),
        git_repo_url: row.3,
        language: row.4,
        framework: row.5,
        status: if row.6 { "active" } else { "inactive" }.to_string(),
        priority: row.7,
        created_at: row.8.to_rfc3339(),
        updated_at: row.9.to_rfc3339(),
    })
}

async fn fetch_tasks(
    pool: &PgPool,
    project_id: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<TaskResponse>, sqlx::Error> {
    let rows = if let Some(pid) = project_id {
        sqlx::query_as::<_, (Uuid, Uuid, String, Option<String>, String, String, String, chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)>(
            "SELECT t.id, t.project_id, t.title, t.description, t.task_type, t.state::text, t.priority::text, t.created_at, t.updated_at 
             FROM tasks t WHERE t.project_id = $1 ORDER BY t.created_at DESC LIMIT $2 OFFSET $3"
        )
        .bind(pid)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as::<_, (Uuid, Uuid, String, Option<String>, String, String, String, chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)>(
            "SELECT id, project_id, title, description, task_type, state::text, priority::text, created_at, updated_at 
             FROM tasks ORDER BY created_at DESC LIMIT $1 OFFSET $2"
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?
    };

    // Fetch project names
    let mut tasks: Vec<TaskResponse> = Vec::new();
    for row in rows {
        tasks.push(TaskResponse {
            id: row.0.to_string(),
            project_id: row.1.to_string(),
            project_name: None,
            title: row.2,
            description: row.3.unwrap_or_default(),
            task_type: row.4,
            status: row.5,
            priority: row.6,
            created_at: row.7.to_rfc3339(),
            updated_at: row.8.to_rfc3339(),
        });
    }
    Ok(tasks)
}

async fn fetch_task_by_id(
    pool: &PgPool,
    task_id: Uuid,
) -> Result<Option<TaskResponse>, sqlx::Error> {
    let row = sqlx::query_as::<_, (Uuid, Uuid, String, Option<String>, String, String, String, chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)>(
        "SELECT id, project_id, title, description, task_type, state::text, priority::text, created_at, updated_at FROM tasks WHERE id = $1 LIMIT 1"
    )
    .bind(task_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| TaskResponse {
        id: r.0.to_string(),
        project_id: r.1.to_string(),
        project_name: None,
        title: r.2,
        description: r.3.unwrap_or_default(),
        task_type: r.4,
        status: r.5,
        priority: r.6,
        created_at: r.7.to_rfc3339(),
        updated_at: r.8.to_rfc3339(),
    }))
}

fn parse_priority(raw: Option<&str>) -> Priority {
    match raw
        .map(|s| s.trim().to_ascii_lowercase())
        .as_deref()
        .unwrap_or("normal")
    {
        "critical" => Priority::Critical,
        "urgent" => Priority::Urgent,
        "high" => Priority::High,
        "low" => Priority::Low,
        "background" => Priority::Background,
        _ => Priority::Normal,
    }
}

fn parse_idempotency_key(headers: &HeaderMap) -> Option<String> {
    headers
        .get("idempotency-key")
        .or_else(|| headers.get("x-idempotency-key"))
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToOwned::to_owned)
}

fn error_task_response(message: &str) -> TaskResponse {
    TaskResponse {
        id: String::new(),
        project_id: String::new(),
        project_name: None,
        title: String::new(),
        description: message.to_string(),
        task_type: "generic".to_string(),
        status: "error".to_string(),
        priority: "medium".to_string(),
        created_at: Utc::now().to_rfc3339(),
        updated_at: Utc::now().to_rfc3339(),
    }
}

async fn fetch_deployments(
    pool: &PgPool,
    limit: i64,
    offset: i64,
) -> Result<Vec<DeploymentResponse>, sqlx::Error> {
    let rows = sqlx::query_as::<
        _,
        (
            Uuid,
            Uuid,
            Option<String>,
            String,
            String,
            chrono::DateTime<chrono::Utc>,
            Option<chrono::DateTime<chrono::Utc>>,
            serde_json::Value,
        ),
    >(
        "SELECT id, project_id, git_ref, status::text, environment::text, created_at,
                finished_at, metadata
         FROM deployments ORDER BY created_at DESC LIMIT $1 OFFSET $2",
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| DeploymentResponse {
            id: r.0.to_string(),
            project_id: r.1.to_string(),
            project_name: None,
            version: r.2.clone().unwrap_or_else(|| "unknown".to_string()),
            status: match r.3.as_str() {
                "queued" => "pending",
                "running" | "waiting_approval" | "approved" => "deploying",
                "succeeded" => "success",
                other => other,
            }
            .to_string(),
            environment: r.4,
            commit_sha: r.2,
            completed_at: r.6.map(|value| value.to_rfc3339()),
            health_check_passed: r
                .7
                .get("health_check_passed")
                .and_then(serde_json::Value::as_bool),
            created_at: r.5.to_rfc3339(),
        })
        .collect())
}

pub async fn get_task(
    State(state): State<Arc<ApiState>>,
    Path(task_id): Path<Uuid>,
) -> (StatusCode, Json<Option<TaskResponse>>) {
    match fetch_task_by_id(&state.db_pool, task_id).await {
        Ok(Some(task)) => (StatusCode::OK, Json(Some(task))),
        Ok(None) => (StatusCode::NOT_FOUND, Json(None)),
        Err(error) => {
            tracing::error!(%error, %task_id, "Failed to fetch task");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(None))
        }
    }
}

pub async fn event_stream(
    State(state): State<Arc<ApiState>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let receiver = state.event_bus.subscribe();
    let stream = stream::unfold(receiver, |mut receiver| async move {
        loop {
            match receiver.recv().await {
                Ok(envelope) => {
                    let payload = serde_json::json!({
                        "type": event_name(&envelope.event),
                        "payload": envelope.event,
                        "id": envelope.id,
                        "timestamp": envelope.timestamp,
                        "source": envelope.source,
                        "correlation_id": envelope.correlation_id,
                    });
                    let event = Event::default()
                        .id(envelope.id.to_string())
                        .json_data(payload)
                        .unwrap_or_else(|_| Event::default().data("{}"));
                    return Some((Ok(event), receiver));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(skipped, "SSE client lagged behind event bus");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

pub async fn get_openapi() -> ([(&'static str, &'static str); 1], &'static str) {
    (
        [("content-type", "application/json; charset=utf-8")],
        include_str!("../../../openapi/argus.json"),
    )
}

async fn fetch_llm_usage(
    pool: &PgPool,
    limit: i64,
    offset: i64,
) -> Result<Vec<LlmUsageEntry>, sqlx::Error> {
    let rows = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            String,
            i32,
            i32,
            f64,
            chrono::DateTime<chrono::Utc>,
        ),
    >(
        "SELECT id, provider, model, input_tokens, output_tokens, cost_usd, created_at 
         FROM llm_usage ORDER BY created_at DESC LIMIT $1 OFFSET $2",
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| LlmUsageEntry {
            id: r.0.to_string(),
            provider: r.1,
            model: r.2,
            input_tokens: r.3,
            output_tokens: r.4,
            cost_usd: r.5,
            timestamp: r.6.to_rfc3339(),
        })
        .collect())
}

// ============================================================
// API Handlers
// ============================================================

pub async fn get_status(State(state): State<Arc<ApiState>>) -> Json<SystemStatus> {
    // Check database connection
    let db_connected = sqlx::query("SELECT 1")
        .fetch_optional(&state.db_pool)
        .await
        .is_ok();

    Json(SystemStatus {
        version: env!("CARGO_PKG_VERSION").to_string(),
        status: "running".to_string(),
        uptime_seconds: state.started_at.elapsed().as_secs(),
        database_connected: db_connected,
        workers_active: 0,
    })
}

pub async fn get_scheduler_stats(
    State(state): State<Arc<ApiState>>,
) -> Json<SchedulerStatsResponse> {
    let stats = state.scheduler.stats().await;
    let by_state = stats
        .by_state
        .into_iter()
        .map(|(k, v)| (format!("{:?}", k), v))
        .collect();

    Json(SchedulerStatsResponse {
        total_tasks: stats.total_tasks,
        by_state,
        retry_error_counts: stats.retry_error_counts,
        queue_depth: stats.queue_depth,
    })
}

pub async fn get_projects(
    State(state): State<Arc<ApiState>>,
    Query(params): Query<PaginationParams>,
) -> Json<Vec<ProjectResponse>> {
    let limit = params.limit.unwrap_or(50);
    let offset = params.offset.unwrap_or(0);

    match fetch_projects(&state.db_pool, limit, offset).await {
        Ok(projects) => Json(projects),
        Err(e) => {
            tracing::error!("Failed to fetch projects: {}", e);
            Json(vec![])
        }
    }
}

pub async fn create_project(
    State(state): State<Arc<ApiState>>,
    Json(req): Json<CreateProjectRequest>,
) -> (StatusCode, Json<ProjectResponse>) {
    match create_project_db(
        &state.db_pool,
        &req.name,
        req.description.as_deref(),
        req.git_repo_url.as_deref(),
        req.language.as_deref(),
        req.framework.as_deref(),
    )
    .await
    {
        Ok(project) => (StatusCode::CREATED, Json(project)),
        Err(e) => {
            tracing::error!("Failed to create project: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ProjectResponse {
                    id: String::new(),
                    name: String::new(),
                    description: String::new(),
                    git_repo_url: None,
                    language: None,
                    framework: None,
                    status: "error".to_string(),
                    priority: "medium".to_string(),
                    created_at: Utc::now().to_rfc3339(),
                    updated_at: Utc::now().to_rfc3339(),
                }),
            )
        }
    }
}

pub async fn get_tasks(
    State(state): State<Arc<ApiState>>,
    Query(params): Query<PaginationParams>,
) -> Json<Vec<TaskResponse>> {
    let limit = params.limit.unwrap_or(50);
    let offset = params.offset.unwrap_or(0);

    match fetch_tasks(&state.db_pool, params.project_id.as_deref(), limit, offset).await {
        Ok(tasks) => Json(tasks),
        Err(e) => {
            tracing::error!("Failed to fetch tasks: {}", e);
            Json(vec![])
        }
    }
}

pub async fn create_task(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Json(req): Json<CreateTaskRequest>,
) -> (StatusCode, Json<TaskResponse>) {
    let idempotency_key = parse_idempotency_key(&headers);

    let project_id = match Uuid::parse_str(&req.project_id) {
        Ok(v) => v,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(TaskResponse {
                    id: String::new(),
                    project_id: String::new(),
                    project_name: None,
                    title: String::new(),
                    description: "invalid project_id; expected UUID".to_string(),
                    task_type: "generic".to_string(),
                    status: "error".to_string(),
                    priority: "medium".to_string(),
                    created_at: Utc::now().to_rfc3339(),
                    updated_at: Utc::now().to_rfc3339(),
                }),
            );
        }
    };

    let task_id = Uuid::new_v4();
    if let Some(key) = idempotency_key.as_deref() {
        if let Some(existing_id) = state.scheduler.get_task_id_by_request_key(key).await {
            if let Ok(Some(existing_task)) = fetch_task_by_id(&state.db_pool, existing_id).await {
                return (StatusCode::OK, Json(existing_task));
            }

            tracing::warn!(
                task_id = %existing_id,
                "Idempotency key exists but its task row could not be loaded"
            );
        }
    }

    let mut scheduled_task = TaskBuilder::new(project_id, req.title.clone())
        .description(req.description.clone().unwrap_or_default())
        .priority(parse_priority(req.priority.as_deref()))
        .build();
    scheduled_task.id = task_id;
    scheduled_task.task_type = req.task_type.unwrap_or_else(|| "generic".to_string());
    scheduled_task.test_commands = req.test_commands;
    scheduled_task.acceptance_criteria = req.acceptance_criteria;

    let persisted = match idempotency_key {
        Some(key) => {
            state
                .scheduler
                .add_task_idempotent_durable(scheduled_task, key)
                .await
        }
        None => state
            .scheduler
            .add_task_durable(scheduled_task)
            .await
            .map(|()| task_id),
    };

    match persisted {
        Ok(persisted_id) => match fetch_task_by_id(&state.db_pool, persisted_id).await {
            Ok(Some(task)) => (
                if persisted_id == task_id {
                    StatusCode::CREATED
                } else {
                    StatusCode::OK
                },
                Json(task),
            ),
            Ok(None) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(error_task_response("persisted task could not be loaded")),
            ),
            Err(error) => {
                tracing::error!(%error, "Failed to load persisted task");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(error_task_response("failed to load persisted task")),
                )
            }
        },
        Err(e) => {
            tracing::error!("Failed to create task: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(error_task_response("failed to persist task")),
            )
        }
    }
}

pub async fn get_deployments(
    State(state): State<Arc<ApiState>>,
    Query(params): Query<PaginationParams>,
) -> Json<Vec<DeploymentResponse>> {
    let limit = params.limit.unwrap_or(50);
    let offset = params.offset.unwrap_or(0);

    match fetch_deployments(&state.db_pool, limit, offset).await {
        Ok(deployments) => Json(deployments),
        Err(e) => {
            tracing::error!("Failed to fetch deployments: {}", e);
            Json(vec![])
        }
    }
}

pub async fn get_logs(
    State(state): State<Arc<ApiState>>,
    Query(params): Query<LogQuery>,
) -> Result<Json<Vec<LogEntry>>, (StatusCode, Json<serde_json::Value>)> {
    let rows: Vec<(Uuid, chrono::DateTime<Utc>, String, String)> = sqlx::query_as(
        "SELECT id,timestamp,CASE outcome::text WHEN 'failure' THEN 'error' WHEN 'blocked' THEN 'warn' ELSE 'info' END AS level, concat_ws(' ',COALESCE(action->>'name','audit action'),resource->>'target',actor->>'id',outcome::text) FROM audit_log WHERE ($1::text IS NULL OR CASE outcome::text WHEN 'failure' THEN 'error' WHEN 'blocked' THEN 'warn' ELSE 'info' END=$1) AND ($2::text IS NULL OR $2='audit') ORDER BY timestamp DESC,id LIMIT $3 OFFSET $4"
    ).bind(params.level).bind(params.source).bind(params.limit.unwrap_or(100).clamp(1,500))
        .bind(params.offset.unwrap_or(0).max(0)).fetch_all(&state.db_pool).await
        .map_err(|_| project_state_error(StatusCode::INTERNAL_SERVER_ERROR, "Failed to read audit logs"))?;
    Ok(Json(
        rows.into_iter()
            .map(|(id, timestamp, level, message)| LogEntry {
                id: id.to_string(),
                timestamp: timestamp.to_rfc3339(),
                level,
                message,
                source: "audit".into(),
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
pub struct LogQuery {
    pub level: Option<String>,
    pub source: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

pub async fn get_llm_usage(
    State(state): State<Arc<ApiState>>,
    Query(params): Query<PaginationParams>,
) -> Json<Vec<LlmUsageEntry>> {
    let limit = params.limit.unwrap_or(50);
    let offset = params.offset.unwrap_or(0);

    match fetch_llm_usage(&state.db_pool, limit, offset).await {
        Ok(usage) => Json(usage),
        Err(e) => {
            tracing::error!("Failed to fetch LLM usage: {}", e);
            Json(vec![])
        }
    }
}

pub async fn get_config(State(state): State<Arc<ApiState>>) -> Json<serde_json::Value> {
    Json(state.runtime_config.clone())
}

pub async fn update_config(
    State(_state): State<Arc<ApiState>>,
    Json(_config): Json<serde_json::Value>,
) -> (StatusCode, Json<serde_json::Value>) {
    project_state_error(
        StatusCode::NOT_IMPLEMENTED,
        "Runtime configuration is read-only; edit the configuration file and restart Argus",
    )
}

pub fn safe_config(config: &argus_core::config::ArgusConfig) -> serde_json::Value {
    fn redact(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(map) => {
                for (key, value) in map {
                    if [
                        "secret",
                        "token",
                        "password",
                        "api_key",
                        "master_key",
                        "credential",
                        "url",
                        "webhook",
                    ]
                    .iter()
                    .any(|s| key.contains(s))
                    {
                        *value = serde_json::Value::String("[redacted]".into());
                    } else {
                        redact(value);
                    }
                }
            }
            serde_json::Value::Array(items) => items.iter_mut().for_each(redact),
            _ => {}
        }
    }
    let mut value = serde_json::to_value(config).unwrap_or(serde_json::Value::Null);
    redact(&mut value);
    value
}

fn evaluation_error(error: argus_core::types::ArgusError) -> (StatusCode, Json<serde_json::Value>) {
    match error {
        argus_core::types::ArgusError::NotFound(message) => {
            project_state_error(StatusCode::NOT_FOUND, message)
        }
        argus_core::types::ArgusError::Validation(message) => {
            project_state_error(StatusCode::CONFLICT, message)
        }
        _ => project_state_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Evaluation could not be persisted or loaded",
        ),
    }
}

pub async fn link_evaluation_task(
    State(state): State<Arc<ApiState>>,
    Path((project, work_item, task)): Path<(String, String, String)>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let project_id = parse_uuid_field(&project, "project_id")?;
    let work_item_id = parse_uuid_field(&work_item, "work_item_id")?;
    let task_id = parse_uuid_field(&task, "task_id")?;
    let item = state
        .project_state
        .get_work_item(work_item_id)
        .await
        .map_err(evaluation_error)?;
    if item.is_none_or(|w| w.project_id != project_id) {
        return Err(project_state_error(
            StatusCode::NOT_FOUND,
            "Work item not found in project",
        ));
    }
    state
        .project_state
        .repository()
        .link_work_item_task(work_item_id, task_id)
        .await
        .map_err(evaluation_error)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn evaluate_work_item(
    State(state): State<Arc<ApiState>>,
    Path((project, work_item)): Path<(String, String)>,
) -> Result<Json<argus_core::evaluation::EvaluationReport>, (StatusCode, Json<serde_json::Value>)> {
    let report = state
        .evaluation
        .evaluate(
            parse_uuid_field(&project, "project_id")?,
            parse_uuid_field(&work_item, "work_item_id")?,
        )
        .await
        .map_err(evaluation_error)?;
    Ok(Json(report))
}

pub async fn list_evaluations(
    State(state): State<Arc<ApiState>>,
    Path((project, work_item)): Path<(String, String)>,
) -> Result<
    Json<Vec<argus_core::evaluation::EvaluationReport>>,
    (StatusCode, Json<serde_json::Value>),
> {
    Ok(Json(
        state
            .evaluation
            .list(
                parse_uuid_field(&project, "project_id")?,
                parse_uuid_field(&work_item, "work_item_id")?,
            )
            .await
            .map_err(evaluation_error)?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptWorkItemRequest {
    pub evaluation_id: Uuid,
}

pub async fn accept_work_item(
    State(state): State<Arc<ApiState>>,
    axum::extract::Extension(actor): axum::extract::Extension<crate::http_security::HttpActor>,
    Path((project, work_item)): Path<(String, String)>,
    Json(request): Json<AcceptWorkItemRequest>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    state
        .evaluation
        .accept(
            parse_uuid_field(&project, "project_id")?,
            parse_uuid_field(&work_item, "work_item_id")?,
            request.evaluation_id,
            &actor.name,
        )
        .await
        .map_err(evaluation_error)?;
    Ok(StatusCode::NO_CONTENT)
}

// ============================================================
// V0.39 Project State handlers
// ============================================================

pub async fn get_project_snapshot(
    State(state): State<Arc<ApiState>>,
    Path(project_id): Path<String>,
) -> Result<Json<ProjectSnapshot>, (StatusCode, Json<serde_json::Value>)> {
    let project_id = parse_uuid_field(&project_id, "project_id")?;
    state
        .project_state
        .get_project_snapshot(project_id)
        .await
        .map(Json)
        .map_err(project_state_not_found_or_internal)
}

pub async fn get_project_state(
    State(state): State<Arc<ApiState>>,
    Path(project_id): Path<String>,
) -> Result<Json<ProjectStateResponse>, (StatusCode, Json<serde_json::Value>)> {
    let project_id = parse_uuid_field(&project_id, "project_id")?;
    let snapshot = state
        .project_state
        .get_project_snapshot(project_id)
        .await
        .map_err(project_state_not_found_or_internal)?;
    let status_text = state
        .project_state
        .render_status(project_id)
        .await
        .map_err(project_state_not_found_or_internal)?;
    Ok(Json(ProjectStateResponse {
        snapshot,
        status_text,
    }))
}

/// `GET /api/projects/:id/roadmap` — full deterministic roadmap + progress.
pub async fn get_project_roadmap(
    State(state): State<Arc<ApiState>>,
    Path(project_id): Path<String>,
) -> Result<Json<argus_core::project_state::RoadmapProgress>, (StatusCode, Json<serde_json::Value>)>
{
    let project_id = parse_uuid_field(&project_id, "project_id")?;
    state
        .project_state
        .get_roadmap_progress(project_id)
        .await
        .map(Json)
        .map_err(project_state_not_found_or_internal)
}

/// `GET /api/projects/:id/progress` — compact deterministic progress report.
pub async fn get_project_progress(
    State(state): State<Arc<ApiState>>,
    Path(project_id): Path<String>,
) -> Result<Json<argus_core::project_state::ProgressReport>, (StatusCode, Json<serde_json::Value>)>
{
    let project_id = parse_uuid_field(&project_id, "project_id")?;
    state
        .project_state
        .get_progress_report(project_id)
        .await
        .map(Json)
        .map_err(project_state_not_found_or_internal)
}

/// `GET /api/projects/:id/progress/diff` — current vs. latest recorded snapshot.
pub async fn get_project_progress_diff(
    State(state): State<Arc<ApiState>>,
    Path(project_id): Path<String>,
) -> Result<Json<argus_core::project_state::ProgressDiff>, (StatusCode, Json<serde_json::Value>)> {
    let project_id = parse_uuid_field(&project_id, "project_id")?;
    state
        .project_state
        .get_progress_diff(project_id)
        .await
        .map(Json)
        .map_err(project_state_not_found_or_internal)
}

/// `POST /api/projects/:id/progress/record` — record a progress baseline.
pub async fn record_project_progress(
    State(state): State<Arc<ApiState>>,
    Path(project_id): Path<String>,
) -> Result<Json<argus_core::project_state::RoadmapProgress>, (StatusCode, Json<serde_json::Value>)>
{
    let project_id = parse_uuid_field(&project_id, "project_id")?;
    state
        .project_state
        .record_progress_snapshot(project_id)
        .await
        .map(Json)
        .map_err(project_state_not_found_or_internal)
}

pub async fn create_milestone_state(
    State(state): State<Arc<ApiState>>,
    Path(project_id): Path<String>,
    Json(req): Json<CreateMilestoneRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, Json<serde_json::Value>)> {
    let project_id = parse_uuid_field(&project_id, "project_id")?;
    let parent_id = parse_optional_uuid(req.parent_id.as_deref(), "parent_id")?;
    let record = state
        .project_state
        .create_milestone(NewMilestone {
            project_id,
            title: req.title,
            description: req.description,
            sequence: req.sequence,
            parent_id,
            definition_of_done: req.definition_of_done,
            external_key: req.external_key,
            weight: req.weight.unwrap_or(1),
        })
        .await
        .map_err(project_state_not_found_or_internal)?;
    Ok((StatusCode::CREATED, Json(serde_json::json!(record))))
}

pub async fn create_work_item_state(
    State(state): State<Arc<ApiState>>,
    Path(project_id): Path<String>,
    Json(req): Json<CreateWorkItemRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, Json<serde_json::Value>)> {
    let project_id = parse_uuid_field(&project_id, "project_id")?;
    let milestone_id = parse_optional_uuid(req.milestone_id.as_deref(), "milestone_id")?;
    let phase_id = parse_optional_uuid(req.phase_id.as_deref(), "phase_id")?;
    let record = state
        .project_state
        .create_work_item(NewWorkItem {
            project_id,
            milestone_id,
            phase_id,
            title: req.title,
            description: req.description.unwrap_or_default(),
            priority: parse_priority(req.priority.as_deref()),
            definition_of_done: req.definition_of_done,
            source: Some("api".to_string()),
            external_key: req.external_key,
            weight: req.weight.unwrap_or(1),
        })
        .await
        .map_err(project_state_not_found_or_internal)?;
    Ok((StatusCode::CREATED, Json(serde_json::json!(record))))
}

pub async fn record_decision_state(
    State(state): State<Arc<ApiState>>,
    Path(project_id): Path<String>,
    Json(req): Json<CreateDecisionRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, Json<serde_json::Value>)> {
    let project_id = parse_uuid_field(&project_id, "project_id")?;
    let status = req
        .status
        .as_deref()
        .and_then(argus_core::project_state::DecisionStatus::parse)
        .unwrap_or(argus_core::project_state::DecisionStatus::Accepted);
    let record = state
        .project_state
        .record_decision(NewDecision {
            project_id,
            title: req.title,
            context: req.context,
            decision: req.decision,
            reason: req.reason,
            status,
            source: Some("api".to_string()),
        })
        .await
        .map_err(project_state_not_found_or_internal)?;
    Ok((StatusCode::CREATED, Json(serde_json::json!(record))))
}

pub async fn create_risk_state(
    State(state): State<Arc<ApiState>>,
    Path(project_id): Path<String>,
    Json(req): Json<CreateRiskRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, Json<serde_json::Value>)> {
    let project_id = parse_uuid_field(&project_id, "project_id")?;
    let milestone_id = parse_optional_uuid(req.milestone_id.as_deref(), "milestone_id")?;
    let severity = req
        .severity
        .as_deref()
        .and_then(RiskSeverity::parse)
        .unwrap_or(RiskSeverity::Medium);
    let likelihood = req
        .likelihood
        .as_deref()
        .and_then(RiskLikelihood::parse)
        .unwrap_or(RiskLikelihood::Medium);
    let record = state
        .project_state
        .create_risk(NewRisk {
            project_id,
            milestone_id,
            title: req.title,
            description: req.description,
            severity,
            likelihood,
            mitigation: req.mitigation,
            owner: req.owner,
        })
        .await
        .map_err(project_state_not_found_or_internal)?;
    Ok((StatusCode::CREATED, Json(serde_json::json!(record))))
}

pub async fn create_blocker_state(
    State(state): State<Arc<ApiState>>,
    Path(project_id): Path<String>,
    Json(req): Json<CreateBlockerRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, Json<serde_json::Value>)> {
    let project_id = parse_uuid_field(&project_id, "project_id")?;
    let work_item_id = parse_optional_uuid(req.work_item_id.as_deref(), "work_item_id")?;
    let record = state
        .project_state
        .create_blocker(NewBlocker {
            project_id,
            work_item_id,
            title: req.title,
            description: req.description,
        })
        .await
        .map_err(project_state_not_found_or_internal)?;
    Ok((StatusCode::CREATED, Json(serde_json::json!(record))))
}

pub async fn create_improvement_state(
    State(state): State<Arc<ApiState>>,
    Path(project_id): Path<String>,
    Json(req): Json<CreateImprovementRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, Json<serde_json::Value>)> {
    let project_id = parse_uuid_field(&project_id, "project_id")?;
    let priority = req
        .priority
        .as_deref()
        .and_then(ImprovementPriority::parse)
        .unwrap_or(ImprovementPriority::Medium);
    let status = req
        .status
        .as_deref()
        .and_then(ImprovementStatus::parse)
        .unwrap_or(ImprovementStatus::Deferred);
    let record = state
        .project_state
        .create_improvement(NewImprovement {
            project_id,
            external_key: req.external_key,
            title: req.title,
            description: req.description,
            priority,
            status,
            source: req.source,
            why_deferred: req.why_deferred,
            required_before: req.required_before,
        })
        .await
        .map_err(project_state_not_found_or_internal)?;
    Ok((StatusCode::CREATED, Json(serde_json::json!(record))))
}

pub async fn record_activity_state(
    State(state): State<Arc<ApiState>>,
    Path(project_id): Path<String>,
    Json(req): Json<RecordActivityRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, Json<serde_json::Value>)> {
    let project_id = parse_uuid_field(&project_id, "project_id")?;
    let activity_type = ActivityType::parse(&req.activity_type).ok_or_else(|| {
        project_state_error(
            StatusCode::BAD_REQUEST,
            format!("unknown activity_type '{}'", req.activity_type),
        )
    })?;
    let entity_id = parse_optional_uuid(req.entity_id.as_deref(), "entity_id")?;
    let record = state
        .project_state
        .record_activity(NewActivity {
            project_id,
            activity_type,
            entity_type: req.entity_type.unwrap_or_else(|| "project".to_string()),
            entity_id,
            summary: req.summary,
            metadata: serde_json::json!({}),
        })
        .await
        .map_err(project_state_not_found_or_internal)?;
    Ok((StatusCode::CREATED, Json(serde_json::json!(record))))
}

/// `POST /api/projects/:id/context/compile[?format=markdown]` — deterministic
/// role/purpose-aware Context Packet. No model is called.
pub async fn compile_project_context(
    State(state): State<Arc<ApiState>>,
    Path(project_id): Path<String>,
    Query(query): Query<ContextQuery>,
    Json(req): Json<CompileContextRequest>,
) -> Result<Response, (StatusCode, Json<serde_json::Value>)> {
    let project_id = parse_uuid_field(&project_id, "project_id")?;
    let role = ContextRole::parse(&req.role).ok_or_else(|| {
        project_state_error(
            StatusCode::BAD_REQUEST,
            format!("unknown role '{}'", req.role),
        )
    })?;
    let purpose = ContextPurpose::parse(&req.purpose).ok_or_else(|| {
        project_state_error(
            StatusCode::BAD_REQUEST,
            format!("unknown purpose '{}'", req.purpose),
        )
    })?;

    let mut request = ContextRequest::new(project_id, role, purpose);
    request.work_item_id = parse_optional_uuid(req.work_item_id.as_deref(), "work_item_id")?;
    request.task_id = parse_optional_uuid(req.task_id.as_deref(), "task_id")?;
    request.execution_id = parse_optional_uuid(req.execution_id.as_deref(), "execution_id")?;
    request.max_items = req.max_items;
    request.max_bytes = req.max_bytes;
    request.estimated_token_budget = req.estimated_token_budget;
    if let Some(sections) = &req.requested_sections {
        let mut parsed = Vec::with_capacity(sections.len());
        for section in sections {
            parsed.push(ContextSection::parse(section).ok_or_else(|| {
                project_state_error(
                    StatusCode::BAD_REQUEST,
                    format!("unknown section '{section}'"),
                )
            })?);
        }
        request.requested_sections = Some(parsed);
    }
    if let Some(v) = req.include_recent_activity {
        request.include_recent_activity = v;
    }
    if let Some(v) = req.include_artifacts {
        request.include_artifacts = v;
    }

    let packet = state
        .project_state
        .compile_context(&request)
        .await
        .map_err(project_state_not_found_or_internal)?;

    let format = query
        .format
        .as_deref()
        .and_then(ContextFormat::parse)
        .unwrap_or(ContextFormat::Json);
    Ok(match format {
        ContextFormat::Markdown => (
            StatusCode::OK,
            [("content-type", "text/markdown; charset=utf-8")],
            render_markdown(&packet),
        )
            .into_response(),
        ContextFormat::Json => Json(packet).into_response(),
    })
}

/// `POST /api/projects/:id/intelligence/classify` — deterministic TaskProfile
/// + RoutingDecision. No model is called.
pub async fn classify_task(
    State(state): State<Arc<ApiState>>,
    Path(project_id): Path<String>,
    Json(req): Json<ClassifyTaskRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, Json<serde_json::Value>)> {
    let project_id = parse_uuid_field(&project_id, "project_id")?;
    let work_item_id = parse_uuid_field(&req.work_item_id, "work_item_id")?;
    let role = ContextRole::parse(&req.role).ok_or_else(|| {
        project_state_error(
            StatusCode::BAD_REQUEST,
            format!("unknown role '{}'", req.role),
        )
    })?;
    let purpose = ContextPurpose::parse(&req.purpose).ok_or_else(|| {
        project_state_error(
            StatusCode::BAD_REQUEST,
            format!("unknown purpose '{}'", req.purpose),
        )
    })?;
    let (profile, decision) = state
        .task_intelligence
        .classify_work_item(project_id, work_item_id, role, purpose)
        .await
        .map_err(project_state_not_found_or_internal)?;
    Ok((
        StatusCode::OK,
        Json(serde_json::json!({
            "profile": profile,
            "routing_decision": decision,
        })),
    ))
}

/// `POST /api/routing/:routing_id/outcome`
pub async fn record_routing_outcome(
    State(state): State<Arc<ApiState>>,
    Path(routing_id): Path<String>,
    Json(req): Json<RoutingOutcomeRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, Json<serde_json::Value>)> {
    let routing_id = parse_uuid_field(&routing_id, "routing_id")?;
    let failure_class = match req.failure_class.as_deref() {
        Some(s) => Some(FailureClass::parse(s).ok_or_else(|| {
            project_state_error(
                StatusCode::BAD_REQUEST,
                format!("unknown failure_class '{s}'"),
            )
        })?),
        None => None,
    };
    let artifact_id = parse_optional_uuid(req.artifact_id.as_deref(), "artifact_id")?;
    let outcome = state
        .task_intelligence
        .record_outcome(
            routing_id,
            OutcomeInput {
                success: req.success,
                failure_class,
                attempts: req.attempts.unwrap_or(1),
                duration_ms: req.duration_ms,
                artifact_id,
            },
        )
        .await
        .map_err(project_state_not_found_or_internal)?;
    Ok((StatusCode::CREATED, Json(serde_json::json!(outcome))))
}

/// `POST /api/routing/:routing_id/failure`
pub async fn record_routing_failure(
    State(state): State<Arc<ApiState>>,
    Path(routing_id): Path<String>,
    Json(req): Json<RoutingFailureRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, Json<serde_json::Value>)> {
    let routing_id = parse_uuid_field(&routing_id, "routing_id")?;
    let failure_class = match req.failure_class.as_deref() {
        Some(s) => Some(FailureClass::parse(s).ok_or_else(|| {
            project_state_error(
                StatusCode::BAD_REQUEST,
                format!("unknown failure_class '{s}'"),
            )
        })?),
        None => None,
    };
    let error_type = match req.error_type.as_deref() {
        Some(s) => Some(
            serde_json::from_value::<TaskErrorType>(serde_json::Value::String(s.to_string()))
                .map_err(|_| {
                    project_state_error(
                        StatusCode::BAD_REQUEST,
                        format!("unknown error_type '{s}'"),
                    )
                })?,
        ),
        None => None,
    };
    let decision = state
        .task_intelligence
        .record_failure(
            routing_id,
            FailureInput {
                failure_class,
                error_type,
                attempts: req.attempts,
                context_truncated: req.context_truncated.unwrap_or(false),
            },
        )
        .await
        .map_err(project_state_not_found_or_internal)?;
    Ok((StatusCode::CREATED, Json(serde_json::json!(decision))))
}

/// `GET /api/projects/:id/routing/stats`
pub async fn get_routing_stats(
    State(state): State<Arc<ApiState>>,
    Path(project_id): Path<String>,
) -> Result<Json<Vec<TaskKindTierStats>>, (StatusCode, Json<serde_json::Value>)> {
    let project_id = parse_uuid_field(&project_id, "project_id")?;
    state
        .task_intelligence
        .routing_stats(project_id)
        .await
        .map(Json)
        .map_err(project_state_not_found_or_internal)
}

/// `POST /api/projects/:id/planning/requests`
pub async fn create_planning_request(
    State(state): State<Arc<ApiState>>,
    Path(project_id): Path<String>,
    Json(req): Json<CreatePlanningRequestBody>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, Json<serde_json::Value>)> {
    let project_id = parse_uuid_field(&project_id, "project_id")?;
    let planning_kind = PlanningKind::parse(&req.planning_kind).ok_or_else(|| {
        project_state_error(
            StatusCode::BAD_REQUEST,
            format!("unknown planning_kind '{}'", req.planning_kind),
        )
    })?;
    let role = ContextRole::parse(&req.role).ok_or_else(|| {
        project_state_error(
            StatusCode::BAD_REQUEST,
            format!("unknown role '{}'", req.role),
        )
    })?;
    let purpose = ContextPurpose::parse(&req.purpose).ok_or_else(|| {
        project_state_error(
            StatusCode::BAD_REQUEST,
            format!("unknown purpose '{}'", req.purpose),
        )
    })?;
    let work_item_id = parse_optional_uuid(req.work_item_id.as_deref(), "work_item_id")?;
    let mut requested_outputs = Vec::new();
    for output in req.requested_outputs.unwrap_or_default() {
        requested_outputs.push(RequestedOutput::parse(&output).ok_or_else(|| {
            project_state_error(
                StatusCode::BAD_REQUEST,
                format!("unknown requested_output '{output}'"),
            )
        })?);
    }

    let request = state
        .planner
        .create_planning_request(CreatePlanningRequestInput {
            project_id,
            planning_kind,
            role,
            purpose,
            objective: req.objective,
            constraints: req.constraints.unwrap_or_default(),
            requested_outputs,
            work_item_id,
        })
        .await
        .map_err(project_state_not_found_or_internal)?;
    Ok((StatusCode::CREATED, Json(serde_json::json!(request))))
}

/// `GET /api/planning/requests/:id/packet[?format=markdown|json]`
pub async fn get_planning_packet(
    State(state): State<Arc<ApiState>>,
    Path(request_id): Path<String>,
    Query(query): Query<ContextQuery>,
) -> Result<Response, (StatusCode, Json<serde_json::Value>)> {
    let request_id = parse_uuid_field(&request_id, "request_id")?;
    let packet = state
        .planner
        .export_packet(request_id)
        .await
        .map_err(project_state_not_found_or_internal)?;
    let format = query
        .format
        .as_deref()
        .and_then(ContextFormat::parse)
        .unwrap_or(ContextFormat::Markdown);
    Ok(match format {
        ContextFormat::Json => (
            StatusCode::OK,
            [("content-type", "application/json; charset=utf-8")],
            packet,
        )
            .into_response(),
        ContextFormat::Markdown => (
            StatusCode::OK,
            [("content-type", "text/markdown; charset=utf-8")],
            packet,
        )
            .into_response(),
    })
}

/// `POST /api/planning/results/import` — body is an untrusted `PlanningResult`.
pub async fn import_planning_result(
    State(state): State<Arc<ApiState>>,
    Json(result): Json<PlanningResult>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, Json<serde_json::Value>)> {
    let outcome = state
        .planner
        .import_result(result)
        .await
        .map_err(project_state_not_found_or_internal)?;
    Ok((StatusCode::CREATED, Json(serde_json::json!(outcome))))
}

pub async fn get_planning_proposal(
    State(state): State<Arc<ApiState>>,
    Path(proposal_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let proposal_id = parse_uuid_field(&proposal_id, "proposal_id")?;
    state
        .planner
        .get_proposal(proposal_id)
        .await
        .map(|p| Json(serde_json::json!(p)))
        .map_err(project_state_not_found_or_internal)
}

pub async fn get_planning_proposal_diff(
    State(state): State<Arc<ApiState>>,
    Path(proposal_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let proposal_id = parse_uuid_field(&proposal_id, "proposal_id")?;
    state
        .planner
        .get_diff(proposal_id)
        .await
        .map(|d| Json(serde_json::json!(d)))
        .map_err(project_state_not_found_or_internal)
}

pub async fn approve_planning_proposal(
    State(state): State<Arc<ApiState>>,
    Path(proposal_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let proposal_id = parse_uuid_field(&proposal_id, "proposal_id")?;
    state
        .planner
        .approve_proposal(proposal_id)
        .await
        .map(|p| Json(serde_json::json!(p)))
        .map_err(project_state_not_found_or_internal)
}

pub async fn reject_planning_proposal(
    State(state): State<Arc<ApiState>>,
    Path(proposal_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let proposal_id = parse_uuid_field(&proposal_id, "proposal_id")?;
    state
        .planner
        .reject_proposal(proposal_id)
        .await
        .map(|p| Json(serde_json::json!(p)))
        .map_err(project_state_not_found_or_internal)
}

pub async fn apply_planning_proposal(
    State(state): State<Arc<ApiState>>,
    Path(proposal_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let proposal_id = parse_uuid_field(&proposal_id, "proposal_id")?;
    state
        .planner
        .apply_proposal(proposal_id)
        .await
        .map(|o| Json(serde_json::json!(o)))
        .map_err(project_state_not_found_or_internal)
}

/// `GET /api/providers`
pub async fn list_providers(State(state): State<Arc<ApiState>>) -> Json<serde_json::Value> {
    let registry = state.provider.registry().await;
    Json(serde_json::json!(registry.providers()))
}

/// `GET /api/providers/status`
pub async fn providers_status(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let discovery = state.provider.discover_all().await;
    let snapshot = state
        .provider
        .state()
        .await
        .map_err(project_state_not_found_or_internal)?;
    Ok(Json(serde_json::json!({
        "discovery": discovery,
        "health": snapshot.health,
    })))
}

/// `GET /api/providers/:id/models`
pub async fn list_provider_models(
    State(state): State<Arc<ApiState>>,
    Path(provider_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let provider = RuntimeProvider::parse(&provider_id).ok_or_else(|| {
        project_state_error(
            StatusCode::BAD_REQUEST,
            format!("unknown provider_id '{provider_id}'"),
        )
    })?;
    let registry = state.provider.registry().await;
    let models: Vec<_> = registry.models_for(provider).cloned().collect();
    Ok(Json(serde_json::json!(models)))
}

/// `GET /api/auth-profiles`
pub async fn list_auth_profiles(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let profiles = state
        .provider
        .list_auth_profiles()
        .await
        .map_err(project_state_not_found_or_internal)?;
    Ok(Json(serde_json::json!(profiles)))
}

/// `POST /api/auth-profiles` — register a credential *locator* only.
pub async fn register_auth_profile(
    State(state): State<Arc<ApiState>>,
    Json(req): Json<RegisterAuthProfileRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, Json<serde_json::Value>)> {
    let provider_id = RuntimeProvider::parse(&req.provider_id).ok_or_else(|| {
        project_state_error(
            StatusCode::BAD_REQUEST,
            format!("unknown provider_id '{}'", req.provider_id),
        )
    })?;
    let auth_kind = AuthKind::parse(&req.auth_kind).ok_or_else(|| {
        project_state_error(
            StatusCode::BAD_REQUEST,
            format!("unknown auth_kind '{}'", req.auth_kind),
        )
    })?;
    let profile = state
        .provider
        .ensure_auth_profile(NewAuthProfile {
            provider_id,
            auth_kind,
            account_label: req.account_label,
            workspace_label: None,
            status: AuthStatus::Unknown,
            secret_reference: req.secret_reference.map(SecretReference::new),
            credential_source: req.credential_source,
        })
        .await
        .map_err(project_state_not_found_or_internal)?;
    Ok((StatusCode::CREATED, Json(serde_json::json!(profile))))
}

/// `GET /api/auth-profiles/:id/quota`
pub async fn get_auth_quota(
    State(state): State<Arc<ApiState>>,
    Path(auth_profile_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let id = parse_uuid_field(&auth_profile_id, "auth_profile_id")?;
    let profiles = state
        .provider
        .list_auth_profiles()
        .await
        .map_err(project_state_not_found_or_internal)?;
    let profile = profiles
        .into_iter()
        .find(|p| p.auth_profile_id == id)
        .ok_or_else(|| project_state_error(StatusCode::NOT_FOUND, "auth profile not found"))?;
    let quota = state
        .provider
        .latest_quota(profile.provider_id, Some(id))
        .await
        .map_err(project_state_not_found_or_internal)?;
    Ok(Json(serde_json::json!(quota)))
}

/// `POST /api/auth-profiles/:id/quota/refresh`
pub async fn refresh_auth_quota(
    State(state): State<Arc<ApiState>>,
    Path(auth_profile_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let id = parse_uuid_field(&auth_profile_id, "auth_profile_id")?;
    let profiles = state
        .provider
        .list_auth_profiles()
        .await
        .map_err(project_state_not_found_or_internal)?;
    let profile = profiles
        .into_iter()
        .find(|p| p.auth_profile_id == id)
        .ok_or_else(|| project_state_error(StatusCode::NOT_FOUND, "auth profile not found"))?;
    let quota = state
        .provider
        .refresh_quota(profile.provider_id, Some(id))
        .await
        .map_err(project_state_not_found_or_internal)?;
    Ok(Json(serde_json::json!(quota)))
}

/// `POST /api/projects/:id/provider-routing/select`
pub async fn select_provider_route(
    State(state): State<Arc<ApiState>>,
    Path(project_id): Path<String>,
    Json(req): Json<ProviderRoutingSelectRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let project_id = parse_uuid_field(&project_id, "project_id")?;
    let routing_decision_id = parse_uuid_field(&req.routing_decision_id, "routing_decision_id")?;
    let role = ContextRole::parse(&req.role).ok_or_else(|| {
        project_state_error(
            StatusCode::BAD_REQUEST,
            format!("unknown role '{}'", req.role),
        )
    })?;
    let purpose = ContextPurpose::parse(&req.purpose).ok_or_else(|| {
        project_state_error(
            StatusCode::BAD_REQUEST,
            format!("unknown purpose '{}'", req.purpose),
        )
    })?;
    let decision = state
        .task_intelligence
        .get_routing_decision(routing_decision_id)
        .await
        .map_err(project_state_not_found_or_internal)?
        .ok_or_else(|| project_state_error(StatusCode::NOT_FOUND, "routing decision not found"))?;
    let policy = state
        .provider
        .get_policy(project_id)
        .await
        .map_err(project_state_not_found_or_internal)?;
    let outcome = state
        .provider
        .route(ProviderRoutingInput {
            project_id,
            routing_decision_id: Some(routing_decision_id),
            task_id: decision.task_id,
            role: role.as_str().to_string(),
            purpose: purpose.as_str().to_string(),
            required_tier: decision.required_tier,
            required_capabilities: decision.required_capabilities.clone(),
            reasoning_effort: decision.reasoning_effort,
            estimated_cost_usd: req.estimated_cost_usd,
            policy: policy.to_policy(),
        })
        .await
        .map_err(project_state_not_found_or_internal)?;
    Ok(Json(serde_json::json!(outcome)))
}

/// `GET /api/projects/:id/provider-routing/policy`
pub async fn get_provider_policy(
    State(state): State<Arc<ApiState>>,
    Path(project_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let project_id = parse_uuid_field(&project_id, "project_id")?;
    let policy = state
        .provider
        .get_policy(project_id)
        .await
        .map_err(project_state_not_found_or_internal)?;
    Ok(Json(serde_json::json!(policy)))
}

/// `PUT /api/projects/:id/provider-routing/policy`
pub async fn upsert_provider_policy(
    State(state): State<Arc<ApiState>>,
    Path(project_id): Path<String>,
    Json(req): Json<UpsertProviderPolicyRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let project_id = parse_uuid_field(&project_id, "project_id")?;
    let parse_providers = |list: Option<Vec<String>>| -> Result<Vec<RuntimeProvider>, _> {
        let mut out = Vec::new();
        for item in list.unwrap_or_default() {
            out.push(RuntimeProvider::parse(&item).ok_or_else(|| {
                project_state_error(
                    StatusCode::BAD_REQUEST,
                    format!("unknown provider '{item}'"),
                )
            })?);
        }
        Ok(out)
    };
    let allowed = parse_providers(req.allowed_providers)?;
    let denied = parse_providers(req.denied_providers)?;
    let api_fallback = match req.api_fallback.as_deref() {
        Some(s) => FallbackPolicy::parse(s).ok_or_else(|| {
            project_state_error(
                StatusCode::BAD_REQUEST,
                format!("unknown api_fallback '{s}'"),
            )
        })?,
        None => FallbackPolicy::AutomaticWithinBudget,
    };
    let max_tier = match req.max_tier.as_deref() {
        Some(s) => Some(ModelTier::parse(s).ok_or_else(|| {
            project_state_error(StatusCode::BAD_REQUEST, format!("unknown max_tier '{s}'"))
        })?),
        None => None,
    };
    let preferred_runtime = match req.preferred_runtime.as_deref() {
        Some(s) => Some(RuntimeProvider::parse(s).ok_or_else(|| {
            project_state_error(
                StatusCode::BAD_REQUEST,
                format!("unknown preferred_runtime '{s}'"),
            )
        })?),
        None => None,
    };
    // Model ids are dynamic data (§48/§49), so they are not validated against a
    // fixed catalog — only empties are dropped. A denied unknown id is harmless.
    let clean_models = |list: Option<Vec<String>>| -> Vec<String> {
        list.unwrap_or_default()
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    };
    let policy = ProjectProviderPolicy {
        project_id,
        allowed_providers: allowed,
        denied_providers: denied,
        subscription_first: req.subscription_first.unwrap_or(true),
        api_fallback,
        max_tier,
        preferred_runtime,
        allowed_models: clean_models(req.allowed_models),
        denied_models: clean_models(req.denied_models),
    };
    let stored = state
        .provider
        .upsert_policy(policy)
        .await
        .map_err(project_state_not_found_or_internal)?;
    Ok(Json(serde_json::json!(stored)))
}

/// `GET /api/projects/:id/provider-routing/selections`
pub async fn list_provider_selections(
    State(state): State<Arc<ApiState>>,
    Path(project_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let project_id = parse_uuid_field(&project_id, "project_id")?;
    let selections = state
        .provider
        .list_selections(project_id, 50)
        .await
        .map_err(project_state_not_found_or_internal)?;
    Ok(Json(serde_json::json!(selections)))
}

fn project_state_not_found_or_internal(
    error: argus_core::types::ArgusError,
) -> (StatusCode, Json<serde_json::Value>) {
    let status = match error {
        argus_core::types::ArgusError::NotFound(_) => StatusCode::NOT_FOUND,
        argus_core::types::ArgusError::Validation(_) => StatusCode::BAD_REQUEST,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    project_state_error(status, error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Json;
    use axum::Router;
    use axum::body::Body;
    use axum::extract::State;
    use axum::http::HeaderValue;
    use axum::http::Request;
    use axum::routing::{get, post};
    use serde_json::json;
    use sqlx::postgres::PgConnectOptions;
    use sqlx::postgres::PgPoolOptions;
    use std::time::Duration;
    use tower::util::ServiceExt;

    /// A scheduler whose durable writes really reach Postgres. `create_task`
    /// persists through the scheduler, so a test asserting one `tasks` row must
    /// not run it against the in-memory default, which has no repository.
    fn durable_scheduler(pool: sqlx::PgPool) -> Arc<TaskScheduler> {
        Arc::new(TaskScheduler::new_with_repository(
            Arc::new(argus_core::task_repository::PostgresTaskRepository::new(
                pool,
            )),
            None,
        ))
    }

    #[test]
    fn test_parse_priority_variants() {
        assert!(matches!(
            parse_priority(Some("critical")),
            Priority::Critical
        ));
        assert!(matches!(parse_priority(Some("urgent")), Priority::Urgent));
        assert!(matches!(parse_priority(Some("high")), Priority::High));
        assert!(matches!(parse_priority(Some("low")), Priority::Low));
        assert!(matches!(
            parse_priority(Some("background")),
            Priority::Background
        ));
        assert!(matches!(parse_priority(Some("unknown")), Priority::Normal));
        assert!(matches!(parse_priority(None), Priority::Normal));
    }

    #[test]
    fn test_parse_idempotency_key_variants() {
        let mut headers = HeaderMap::new();
        headers.insert("idempotency-key", HeaderValue::from_static("abc-123"));
        assert_eq!(parse_idempotency_key(&headers), Some("abc-123".to_string()));

        let mut headers = HeaderMap::new();
        headers.insert("x-idempotency-key", HeaderValue::from_static("xyz-789"));
        assert_eq!(parse_idempotency_key(&headers), Some("xyz-789".to_string()));

        let mut headers = HeaderMap::new();
        headers.insert("idempotency-key", HeaderValue::from_static("   "));
        assert_eq!(parse_idempotency_key(&headers), None);
    }

    #[tokio::test]
    #[ignore = "requires postgres and migrations"]
    async fn test_create_task_idempotency_integration() {
        let database_url = std::env::var("ARGUS_DATABASE_URL")
            .or_else(|_| std::env::var("DATABASE_URL"))
            .expect("set ARGUS_DATABASE_URL or DATABASE_URL for integration test");

        let pool = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&database_url)
            .await
            .expect("connect postgres");

        sqlx::migrate!("../../migrations")
            .run(&pool)
            .await
            .expect("run migrations");

        let scheduler = durable_scheduler(pool.clone());
        let state = Arc::new(ApiState::new(pool.clone(), scheduler));

        let project_id = Uuid::new_v4();
        let project_name = format!("idempotency-test-{}", &project_id.to_string()[..8]);
        let git_local_path = format!("/tmp/argus/projects/{project_name}");
        sqlx::query(
            "INSERT INTO projects (id, name, description, git_local_path, active, priority) VALUES ($1, $2, $3, $4, true, 'medium')",
        )
        .bind(project_id)
        .bind(&project_name)
        .bind("integration test")
        .bind(git_local_path)
        .execute(&pool)
        .await
        .expect("insert project");

        let request_1 = CreateTaskRequest {
            project_id: project_id.to_string(),
            title: "idempotent task".to_string(),
            description: Some("ensure one row only".to_string()),
            task_type: Some("generic".to_string()),
            priority: Some("high".to_string()),
            test_commands: vec![],
            acceptance_criteria: vec![],
        };

        let request_2 = CreateTaskRequest {
            project_id: project_id.to_string(),
            title: "idempotent task duplicate submit".to_string(),
            description: Some("same key".to_string()),
            task_type: Some("generic".to_string()),
            priority: Some("high".to_string()),
            test_commands: vec![],
            acceptance_criteria: vec![],
        };

        let mut headers = HeaderMap::new();
        headers.insert("idempotency-key", HeaderValue::from_static("it-key-001"));

        let (status_1, body_1) =
            create_task(State(state.clone()), headers.clone(), Json(request_1)).await;
        let (status_2, body_2) = create_task(State(state), headers, Json(request_2)).await;

        assert_eq!(status_1, StatusCode::CREATED);
        assert_eq!(status_2, StatusCode::OK);
        assert_eq!(body_1.id, body_2.id);

        let task_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM tasks WHERE project_id = $1")
            .bind(project_id)
            .fetch_one(&pool)
            .await
            .expect("count tasks");
        assert_eq!(task_count.0, 1);

        let _ = sqlx::query("DELETE FROM tasks WHERE project_id = $1")
            .bind(project_id)
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM projects WHERE id = $1")
            .bind(project_id)
            .execute(&pool)
            .await;
    }

    #[tokio::test]
    async fn test_get_scheduler_stats_response_shape() {
        let pool = PgPoolOptions::new().connect_lazy_with(PgConnectOptions::new());
        let scheduler = Arc::new(TaskScheduler::new());
        let state = Arc::new(ApiState::new(pool, scheduler.clone()));

        let project_id = Uuid::new_v4();
        let task = TaskBuilder::new(project_id, "stats task").build();
        scheduler.add_task(task).await;

        let resp = get_scheduler_stats(State(state)).await;
        assert!(resp.total_tasks >= 1);
        assert!(resp.by_state.contains_key("Ready") || resp.by_state.contains_key("Pending"));
        assert!(resp.retry_error_counts.is_empty());
    }

    #[tokio::test]
    #[ignore = "requires postgres and migrations"]
    async fn test_create_task_idempotency_http_integration() {
        let database_url = std::env::var("ARGUS_DATABASE_URL")
            .or_else(|_| std::env::var("DATABASE_URL"))
            .expect("set ARGUS_DATABASE_URL or DATABASE_URL for integration test");

        let pool = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&database_url)
            .await
            .expect("connect postgres");

        sqlx::migrate!("../../migrations")
            .run(&pool)
            .await
            .expect("run migrations");

        let scheduler = durable_scheduler(pool.clone());
        let state = Arc::new(ApiState::new(pool.clone(), scheduler));
        let app = Router::new()
            .route("/api/tasks", post(create_task))
            .route("/api/scheduler/stats", get(get_scheduler_stats))
            .with_state(state);

        let project_id = Uuid::new_v4();
        let project_name = format!("idempotency-http-test-{}", &project_id.to_string()[..8]);
        let git_local_path = format!("/tmp/argus/projects/{project_name}");
        sqlx::query(
            "INSERT INTO projects (id, name, description, git_local_path, active, priority) VALUES ($1, $2, $3, $4, true, 'medium')",
        )
        .bind(project_id)
        .bind(&project_name)
        .bind("integration test")
        .bind(git_local_path)
        .execute(&pool)
        .await
        .expect("insert project");

        let payload_1 = json!({
            "project_id": project_id.to_string(),
            "title": "http idempotent task",
            "description": "ensure one row only",
            "task_type": "generic",
            "priority": "high"
        });
        let payload_2 = json!({
            "project_id": project_id.to_string(),
            "title": "http idempotent task duplicate",
            "description": "same key",
            "task_type": "generic",
            "priority": "high"
        });

        let request_1 = Request::builder()
            .method("POST")
            .uri("/api/tasks")
            .header("content-type", "application/json")
            .header("idempotency-key", "it-http-key-001")
            .body(Body::from(payload_1.to_string()))
            .expect("build request 1");

        let request_2 = Request::builder()
            .method("POST")
            .uri("/api/tasks")
            .header("content-type", "application/json")
            .header("idempotency-key", "it-http-key-001")
            .body(Body::from(payload_2.to_string()))
            .expect("build request 2");

        let response_1 = app
            .clone()
            .oneshot(request_1)
            .await
            .expect("send request 1");
        let response_2 = app.oneshot(request_2).await.expect("send request 2");

        assert_eq!(response_1.status(), StatusCode::CREATED);
        assert_eq!(response_2.status(), StatusCode::OK);

        let task_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM tasks WHERE project_id = $1")
            .bind(project_id)
            .fetch_one(&pool)
            .await
            .expect("count tasks");
        assert_eq!(task_count.0, 1);

        let _ = sqlx::query("DELETE FROM tasks WHERE project_id = $1")
            .bind(project_id)
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM projects WHERE id = $1")
            .bind(project_id)
            .execute(&pool)
            .await;
    }

    #[tokio::test]
    #[ignore = "requires postgres and migrations"]
    async fn test_project_state_api_exposes_snapshot_and_status_text() {
        let database_url = std::env::var("ARGUS_DATABASE_URL")
            .or_else(|_| std::env::var("DATABASE_URL"))
            .expect("set ARGUS_DATABASE_URL or DATABASE_URL for integration test");

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

        let state = Arc::new(ApiState::new(pool.clone(), Arc::new(TaskScheduler::new())));

        let slug = format!("api-ps-{}", &Uuid::new_v4().to_string()[..8]);
        let project = state
            .project_state
            .create_project_state(argus_core::project_state::NewProject {
                name: format!("API {slug}"),
                slug: slug.clone(),
                description: "api project state test".to_string(),
                priority: Priority::High,
            })
            .await
            .expect("create project");
        let milestone = state
            .project_state
            .create_milestone(NewMilestone {
                project_id: project.project_id,
                title: "API Milestone".to_string(),
                description: None,
                sequence: None,
                parent_id: None,
                definition_of_done: None,

                external_key: None,
                weight: 1,
            })
            .await
            .expect("create milestone");
        state
            .project_state
            .set_milestone_status(
                milestone.milestone_id,
                argus_core::project_state::MilestoneStatus::Active,
            )
            .await
            .expect("activate milestone");

        let app = Router::new()
            .route("/api/projects/{project_id}/state", get(get_project_state))
            .route(
                "/api/projects/{project_id}/snapshot",
                get(get_project_snapshot),
            )
            .with_state(state);

        let request = Request::builder()
            .method("GET")
            .uri(format!("/api/projects/{}/state", project.project_id))
            .body(Body::empty())
            .expect("build request");
        let response = app.clone().oneshot(request).await.expect("send request");
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json body");
        assert_eq!(
            body["snapshot"]["current_milestone"]["title"],
            "API Milestone"
        );
        assert!(
            body["status_text"]
                .as_str()
                .unwrap()
                .contains("API Milestone")
        );

        // Unknown project -> 404, not a 500 or a fabricated snapshot.
        let missing = Request::builder()
            .method("GET")
            .uri(format!("/api/projects/{}/state", Uuid::new_v4()))
            .body(Body::empty())
            .expect("build missing request");
        let missing_response = app.oneshot(missing).await.expect("send missing request");
        assert_eq!(missing_response.status(), StatusCode::NOT_FOUND);

        let _ = sqlx::query("DELETE FROM projects WHERE id = $1")
            .bind(project.project_id)
            .execute(&pool)
            .await;
    }

    #[tokio::test]
    #[ignore = "requires postgres and migrations"]
    async fn test_project_roadmap_and_progress_api() {
        let database_url = std::env::var("ARGUS_DATABASE_URL")
            .or_else(|_| std::env::var("DATABASE_URL"))
            .expect("set ARGUS_DATABASE_URL or DATABASE_URL for integration test");

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

        let state = Arc::new(ApiState::new(pool.clone(), Arc::new(TaskScheduler::new())));

        let slug = format!("api-rp-{}", &Uuid::new_v4().to_string()[..8]);
        let project = state
            .project_state
            .create_project_state(argus_core::project_state::NewProject {
                name: format!("API RP {slug}"),
                slug,
                description: "api roadmap test".to_string(),
                priority: Priority::Normal,
            })
            .await
            .expect("create project");
        let milestone = state
            .project_state
            .create_milestone(NewMilestone {
                project_id: project.project_id,
                title: "API RP Milestone".to_string(),
                description: None,
                sequence: None,
                parent_id: None,
                definition_of_done: None,
                external_key: None,
                weight: 1,
            })
            .await
            .expect("create milestone");
        state
            .project_state
            .set_milestone_status(
                milestone.milestone_id,
                argus_core::project_state::MilestoneStatus::Active,
            )
            .await
            .expect("activate milestone");
        state
            .project_state
            .create_work_item(NewWorkItem {
                project_id: project.project_id,
                milestone_id: Some(milestone.milestone_id),
                phase_id: None,
                title: "API RP Work".to_string(),
                description: "planned".to_string(),
                priority: Priority::Normal,
                definition_of_done: None,
                source: None,
                external_key: None,
                weight: 1,
            })
            .await
            .expect("create work item");

        let app = Router::new()
            .route(
                "/api/projects/{project_id}/roadmap",
                get(get_project_roadmap),
            )
            .route(
                "/api/projects/{project_id}/progress",
                get(get_project_progress),
            )
            .route(
                "/api/projects/{project_id}/progress/diff",
                get(get_project_progress_diff),
            )
            .route(
                "/api/projects/{project_id}/progress/record",
                post(record_project_progress),
            )
            .with_state(state);

        let get_json = |path: String| {
            Request::builder()
                .method("GET")
                .uri(path)
                .body(Body::empty())
                .expect("build request")
        };

        let roadmap = app
            .clone()
            .oneshot(get_json(format!(
                "/api/projects/{}/roadmap",
                project.project_id
            )))
            .await
            .expect("roadmap request");
        assert_eq!(roadmap.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(roadmap.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["current_milestone"]["title"], "API RP Milestone");

        let progress = app
            .clone()
            .oneshot(get_json(format!(
                "/api/projects/{}/progress",
                project.project_id
            )))
            .await
            .expect("progress request");
        assert_eq!(progress.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(progress.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["completion_percent"], 0.0);

        let record = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/projects/{}/progress/record",
                        project.project_id
                    ))
                    .body(Body::empty())
                    .expect("build record request"),
            )
            .await
            .expect("record request");
        assert_eq!(record.status(), StatusCode::OK);

        let diff = app
            .oneshot(get_json(format!(
                "/api/projects/{}/progress/diff",
                project.project_id
            )))
            .await
            .expect("diff request");
        assert_eq!(diff.status(), StatusCode::OK);

        let _ = sqlx::query("DELETE FROM projects WHERE id = $1")
            .bind(project.project_id)
            .execute(&pool)
            .await;
    }

    #[tokio::test]
    #[ignore = "requires postgres and migrations"]
    async fn test_context_compile_api_json_and_markdown() {
        let database_url = std::env::var("ARGUS_DATABASE_URL")
            .or_else(|_| std::env::var("DATABASE_URL"))
            .expect("set ARGUS_DATABASE_URL or DATABASE_URL for integration test");

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

        let state = Arc::new(ApiState::new(pool.clone(), Arc::new(TaskScheduler::new())));
        let slug = format!("api-ctx-{}", &Uuid::new_v4().to_string()[..8]);
        let project = state
            .project_state
            .create_project_state(argus_core::project_state::NewProject {
                name: format!("API CTX {slug}"),
                slug,
                description: "api context test".to_string(),
                priority: Priority::Normal,
            })
            .await
            .expect("create project");
        let milestone = state
            .project_state
            .create_milestone(NewMilestone {
                project_id: project.project_id,
                title: "API CTX Milestone".to_string(),
                description: None,
                sequence: None,
                parent_id: None,
                definition_of_done: None,
                external_key: None,
                weight: 1,
            })
            .await
            .expect("create milestone");
        state
            .project_state
            .set_milestone_status(
                milestone.milestone_id,
                argus_core::project_state::MilestoneStatus::Active,
            )
            .await
            .expect("activate milestone");

        let app = Router::new()
            .route(
                "/api/projects/{project_id}/context/compile",
                post(compile_project_context),
            )
            .with_state(state);

        let payload = json!({ "role": "project_manager", "purpose": "planning" });
        let request = Request::builder()
            .method("POST")
            .uri(format!(
                "/api/projects/{}/context/compile",
                project.project_id
            ))
            .header("content-type", "application/json")
            .body(Body::from(payload.to_string()))
            .expect("build request");
        let response = app.clone().oneshot(request).await.expect("send request");
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["role"], "project_manager");
        assert_eq!(body["compiler_version"], "context-compiler-v1");
        assert!(
            body["context_hash"]
                .as_str()
                .unwrap()
                .starts_with("sha256:")
        );

        let md_request = Request::builder()
            .method("POST")
            .uri(format!(
                "/api/projects/{}/context/compile?format=markdown",
                project.project_id
            ))
            .header("content-type", "application/json")
            .body(Body::from(payload.to_string()))
            .expect("build markdown request");
        let md_response = app
            .oneshot(md_request)
            .await
            .expect("send markdown request");
        assert_eq!(md_response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(md_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("# ARGUS Context Packet"));
        assert!(text.contains("Project: API CTX"));

        let _ = sqlx::query("DELETE FROM projects WHERE id = $1")
            .bind(project.project_id)
            .execute(&pool)
            .await;
    }

    #[tokio::test]
    #[ignore = "requires postgres and migrations"]
    async fn test_task_intelligence_api() {
        let database_url = std::env::var("ARGUS_DATABASE_URL")
            .or_else(|_| std::env::var("DATABASE_URL"))
            .expect("set ARGUS_DATABASE_URL or DATABASE_URL for integration test");

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

        let state = Arc::new(ApiState::new(pool.clone(), Arc::new(TaskScheduler::new())));
        let slug = format!("api-ti-{}", &Uuid::new_v4().to_string()[..8]);
        let project = state
            .project_state
            .create_project_state(argus_core::project_state::NewProject {
                name: format!("API TI {slug}"),
                slug,
                description: "api task intelligence test".to_string(),
                priority: Priority::Normal,
            })
            .await
            .expect("create project");
        let milestone = state
            .project_state
            .create_milestone(NewMilestone {
                project_id: project.project_id,
                title: "API TI Milestone".to_string(),
                description: None,
                sequence: None,
                parent_id: None,
                definition_of_done: None,
                external_key: None,
                weight: 1,
            })
            .await
            .expect("create milestone");
        let work_item = state
            .project_state
            .create_work_item(NewWorkItem {
                project_id: project.project_id,
                milestone_id: Some(milestone.milestone_id),
                phase_id: None,
                title: "Modify authentication and secret permission boundaries".to_string(),
                description: "Tighten auth checks.".to_string(),
                priority: Priority::Normal,
                definition_of_done: Some("auth tightened".to_string()),
                source: None,
                external_key: None,
                weight: 1,
            })
            .await
            .expect("create work item");

        let app = Router::new()
            .route(
                "/api/projects/{project_id}/intelligence/classify",
                post(classify_task),
            )
            .route(
                "/api/projects/{project_id}/routing/stats",
                get(get_routing_stats),
            )
            .route(
                "/api/routing/{routing_id}/outcome",
                post(record_routing_outcome),
            )
            .route(
                "/api/routing/{routing_id}/failure",
                post(record_routing_failure),
            )
            .with_state(state);

        let classify_body = json!({
            "work_item_id": work_item.work_item_id.to_string(),
            "role": "developer",
            "purpose": "execution",
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/projects/{}/intelligence/classify",
                        project.project_id
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(classify_body.to_string()))
                    .unwrap(),
            )
            .await
            .expect("classify request");
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["routing_decision"]["role"], "developer");
        assert!(
            body["profile"]["recommended_tier"].as_str().unwrap() >= "T2",
            "auth task should be T2+: {}",
            body["profile"]["recommended_tier"]
        );
        assert!(!body["profile"]["reasons"].as_array().unwrap().is_empty());
        let routing_id = body["routing_decision"]["routing_id"]
            .as_str()
            .unwrap()
            .to_string();

        let outcome = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/routing/{routing_id}/outcome"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"success": true, "attempts": 1}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .expect("outcome request");
        assert_eq!(outcome.status(), StatusCode::CREATED);

        let failure = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/routing/{routing_id}/failure"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"failure_class": "environment_failure", "attempts": 5}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .expect("failure request");
        assert_eq!(failure.status(), StatusCode::CREATED);
        let bytes = axum::body::to_bytes(failure.into_body(), usize::MAX)
            .await
            .unwrap();
        let failure_body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(failure_body["action"], "repair_environment");
        assert_eq!(failure_body["to_tier"], serde_json::Value::Null);

        let stats = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!(
                        "/api/projects/{}/routing/stats",
                        project.project_id
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("stats request");
        assert_eq!(stats.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(stats.into_body(), usize::MAX)
            .await
            .unwrap();
        let stats_body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(stats_body.as_array().unwrap().len(), 1);

        let _ = sqlx::query("DELETE FROM projects WHERE id = $1")
            .bind(project.project_id)
            .execute(&pool)
            .await;
    }

    #[tokio::test]
    #[ignore = "requires postgres and migrations"]
    async fn test_planner_harness_api() {
        let database_url = std::env::var("ARGUS_DATABASE_URL")
            .or_else(|_| std::env::var("DATABASE_URL"))
            .expect("set ARGUS_DATABASE_URL or DATABASE_URL for integration test");

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

        let state = Arc::new(ApiState::new(pool.clone(), Arc::new(TaskScheduler::new())));
        let slug = format!("api-plan-{}", &Uuid::new_v4().to_string()[..8]);
        let project = state
            .project_state
            .create_project_state(argus_core::project_state::NewProject {
                name: format!("API PLAN {slug}"),
                slug,
                description: "api planner test".to_string(),
                priority: Priority::Normal,
            })
            .await
            .expect("create project");
        let milestone = state
            .project_state
            .create_milestone(NewMilestone {
                project_id: project.project_id,
                title: "API Plan Milestone".to_string(),
                description: None,
                sequence: None,
                parent_id: None,
                definition_of_done: None,
                external_key: None,
                weight: 1,
            })
            .await
            .expect("create milestone");

        let app = Router::new()
            .route(
                "/api/projects/{project_id}/planning/requests",
                post(create_planning_request),
            )
            .route(
                "/api/planning/requests/{request_id}/packet",
                get(get_planning_packet),
            )
            .route("/api/planning/results/import", post(import_planning_result))
            .route(
                "/api/planning/proposals/{proposal_id}",
                get(get_planning_proposal),
            )
            .route(
                "/api/planning/proposals/{proposal_id}/approve",
                post(approve_planning_proposal),
            )
            .route(
                "/api/planning/proposals/{proposal_id}/apply",
                post(apply_planning_proposal),
            )
            .with_state(state.clone());

        let create_body = json!({
            "planning_kind": "phase_planning",
            "role": "project_manager",
            "purpose": "planning",
            "objective": "Plan via API",
            "requested_outputs": ["roadmap_changes", "work_items"],
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/projects/{}/planning/requests",
                        project.project_id
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(create_body.to_string()))
                    .unwrap(),
            )
            .await
            .expect("create request");
        assert_eq!(response.status(), StatusCode::CREATED);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let request: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let request_id = request["planning_request_id"].as_str().unwrap().to_string();
        let context_hash = request["context_hash"].as_str().unwrap().to_string();
        let request_hash = request["request_hash"].as_str().unwrap().to_string();

        let packet = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/planning/requests/{request_id}/packet"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("packet request");
        assert_eq!(packet.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(packet.into_body(), usize::MAX)
            .await
            .unwrap();
        let packet_text = String::from_utf8_lossy(&bytes);
        assert!(packet_text.contains("planner-protocol-v1"));
        assert!(packet_text.contains("OUTPUT REQUIREMENT"));

        // A manual planner result (would come from a web model).
        let result = json!({
            "planning_result_id": Uuid::new_v4().to_string(),
            "planning_request_id": request_id,
            "status": "proposed",
            "summary": "api plan",
            "proposal": {
                "work_item_changes": [{
                    "operation": "create",
                    "title": "API planned work item",
                    "description": "created by the API test",
                    "definition_of_done": "exists",
                    "milestone_id": milestone.milestone_id.to_string(),
                    "priority": "high",
                    "dependencies": [],
                    "rationale": "api"
                }]
            },
            "assumptions": [],
            "clarifications": [],
            "warnings": [],
            "planner_metadata": {"planner_type": "manual_web"},
            "protocol_version": "planner-protocol-v1",
            "context_hash": context_hash,
            "request_hash": request_hash,
            "result_hash": "",
            "created_at": Utc::now().to_rfc3339(),
        });
        let import = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/planning/results/import")
                    .header("content-type", "application/json")
                    .body(Body::from(result.to_string()))
                    .unwrap(),
            )
            .await
            .expect("import request");
        assert_eq!(import.status(), StatusCode::CREATED);
        let bytes = axum::body::to_bytes(import.into_body(), usize::MAX)
            .await
            .unwrap();
        let outcome: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(outcome["proposal"]["status"], "validated");
        let proposal_id = outcome["proposal"]["proposal_id"]
            .as_str()
            .unwrap()
            .to_string();

        for action in ["approve", "apply"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(format!("/api/planning/proposals/{proposal_id}/{action}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .expect("action request");
            assert_eq!(response.status(), StatusCode::OK, "action {action}");
        }

        let work_items = state
            .project_state
            .list_work_items(project.project_id)
            .await
            .unwrap();
        assert!(
            work_items
                .iter()
                .any(|w| w.title == "API planned work item")
        );

        let _ = sqlx::query("DELETE FROM projects WHERE id = $1")
            .bind(project.project_id)
            .execute(&pool)
            .await;
    }
}

#[cfg(test)]
mod operational_tests {
    use super::*;

    #[tokio::test]
    async fn config_reports_real_redacted_values_and_rejects_online_updates() {
        let mut config = argus_core::config::ArgusConfig::default();
        config.server.port = 4321;
        config.database.url = "postgres://user:credential-for-config-test@localhost/argus".into();
        let snapshot = safe_config(&config);
        assert_eq!(snapshot["server"]["port"], 4321);
        assert_eq!(snapshot["database"]["url"], "[redacted]");
        assert!(!snapshot.to_string().contains("credential-for-config-test"));
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://localhost/argus")
            .unwrap();
        let state = Arc::new(ApiState::new(pool, Arc::new(TaskScheduler::new())));
        let (status, _) = update_config(
            State(state),
            Json(serde_json::json!({"server":{"port":9999}})),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    }
}
