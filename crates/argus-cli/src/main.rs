//! Argus CLI — entry point for the Argus autonomous agent.
//!
//! Commands:
//! - `argus run`      — Start the Argus daemon
//! - `argus migrate`  — Run database migrations
//! - `argus status`   — Show system status (future)
//! - `argus version`  — Print version info

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::{io::Write, str::FromStr};

use anyhow::{Context, Result, anyhow, bail};
use axum::body::Bytes;
use axum::extract::{Extension, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{Next, from_fn, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgPoolOptions;
use tracing::{info, warn};
use uuid::Uuid;

use argus_core::accounting_state::RuntimeAccountingState;
use argus_core::brain::BrainEngine;
use argus_core::budget_service::{BudgetPolicy, BudgetService};
use argus_core::config::ArgusConfig;
use argus_core::event_bus::EventBus;
use argus_core::execution_loop::{ExecutionLoop, ExecutionLoopConfig};
use argus_core::intent::Intent;
use argus_core::spend_repository::PostgresSpendRepository;
use argus_core::task_repository::PostgresTaskRepository;
use argus_core::task_scheduler::TaskScheduler;
use argus_core::task_types::{TaskBuilder, TaskOutput, TaskState};
use argus_core::types::{AuditOutcome, Role};
use argus_llm::ProviderRouter;
use argus_llm::budget::BudgetTracker;
use argus_llm::config::ProvidersConfig;
use argus_llm::types::{ChatMessage, LlmRequest, ModelClass};
use argus_memory::{EmbeddingEngine, MemoryStore, NewMemory};
use argus_security::audit::AuditLogger;
use argus_security::rbac::{Permission, RbacPolicyEngine};
use argus_social::adapters::feishu::FeishuAdapter;
use argus_social::adapters::google_chat::GoogleChatAdapter;
use argus_social::adapters::qq::QqAdapter;
use argus_social::adapters::telegram::TelegramAdapter;
use argus_social::adapters::web::WebDashboardAdapter;
use argus_social::adapters::whatsapp::WhatsAppAdapter;
use argus_social::conversation::ConversationManager;
use argus_social::gateway::SocialGateway;
use argus_social::types::{Platform, SocialMessage};

mod api;
mod http_security;

// ============================================================
// CLI definition
// ============================================================

#[derive(Parser)]
#[command(
    name = "argus",
    about = "Argus — Self-Governing Autonomous AI Agent Terminal",
    version,
    long_about = None,
)]
struct Cli {
    /// Path to the configuration file.
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the Argus daemon.
    Run,

    /// Run pending database migrations.
    Migrate,

    /// Show system status.
    Status,

    /// Print version information.
    Version,

    /// Inspect and bootstrap structured Project State (V0.39).
    Project {
        #[command(subcommand)]
        command: ProjectCommand,
    },

    /// Inspect runtime providers (V0.44).
    Provider {
        #[command(subcommand)]
        command: ProviderCommand,
    },

    /// Inspect and register auth profiles (V0.44).
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },

    /// Inspect and refresh quota (V0.44).
    Quota {
        #[command(subcommand)]
        command: QuotaCommand,
    },

    /// Select a concrete execution target for a routing decision (V0.44).
    Route {
        /// Routing decision UUID (from V0.42).
        routing_decision: String,
        #[arg(long, default_value = "developer")]
        role: String,
        #[arg(long, default_value = "execution")]
        purpose: String,
        #[arg(long)]
        estimated_cost_usd: Option<f64>,
    },

    /// Run the P6 single-project MVP flow.
    Mvp {
        /// Prompt for the MVP scenario.
        #[arg(long, default_value = "创建一个Rust的Hello World项目，加上单元测试")]
        prompt: String,

        /// Workspace directory for the MVP run.
        #[arg(long, default_value = "/tmp/argus-mvp")]
        workspace: PathBuf,
    },

    #[command(hide = true)]
    SchedulerCrashProbe {
        #[arg(long)]
        phase: String,
        #[arg(long)]
        task_id: Option<String>,
    },
}

#[derive(Subcommand)]
enum ProviderCommand {
    /// List configured providers.
    List,
    /// Show discovery/health for CLI providers.
    Status,
    /// Discover installed CLI runtimes (binary/version only).
    Discover,
}

#[derive(Subcommand)]
enum AuthCommand {
    /// List auth profiles (credential locators only).
    List,
    /// Show one auth profile.
    Status {
        /// Auth profile UUID.
        profile: String,
    },
    /// Register an auth profile (stores a locator, never a value).
    Register {
        #[arg(long)]
        provider: String,
        #[arg(long)]
        kind: String,
        #[arg(long)]
        account: Option<String>,
        /// Credential locator, e.g. `env:OPENAI_API_KEY` or `cli:claude`.
        #[arg(long)]
        secret_reference: Option<String>,
    },
}

#[derive(Subcommand)]
enum QuotaCommand {
    /// Show the latest known quota for an auth profile.
    Show {
        /// Auth profile UUID.
        profile: String,
    },
    /// Refresh quota (best-effort; unknown is recorded as unknown).
    Refresh {
        /// Auth profile UUID.
        profile: String,
    },
}

#[derive(Subcommand)]
enum ProjectCommand {
    /// Render the deterministic project status view (no LLM).
    Status {
        /// Project slug (e.g. `argus`) or UUID.
        project: String,
    },
    /// Print compact deterministic progress (V0.40).
    Progress {
        /// Project slug (e.g. `argus`) or UUID.
        project: String,
    },
    /// Compile a deterministic role/purpose Context Packet (V0.41).
    Context {
        /// Project slug (e.g. `argus`) or UUID.
        project: String,
        /// Logical role: chief, project-manager, developer, tester, reviewer,
        /// researcher, integrator.
        #[arg(long, default_value = "project-manager")]
        role: String,
        /// Purpose: planning, execution, review, debugging, research,
        /// integration, status-review.
        #[arg(long, default_value = "planning")]
        purpose: String,
        /// Output format: markdown or json.
        #[arg(long, default_value = "markdown")]
        format: String,
    },
    /// Classify a WorkItem and route it to a capability tier (V0.42).
    Classify {
        /// Project slug (e.g. `argus`) or UUID.
        project: String,
        /// WorkItem UUID to classify.
        #[arg(long)]
        work_item: String,
        /// Logical role: developer, reviewer, tester, project-manager, chief,
        /// researcher, integrator.
        #[arg(long, default_value = "developer")]
        role: String,
        /// Purpose: execution, planning, review, debugging, research,
        /// integration, status-review.
        #[arg(long, default_value = "execution")]
        purpose: String,
    },
    /// Plan export/import/approve/apply (V0.43 planner protocol).
    Plan {
        #[command(subcommand)]
        command: PlanCommand,
    },
    /// Independently evaluate a WorkItem (V0.45).
    Evaluate { project: String, work_item: Uuid },
    /// List durable evaluation reports.
    Evaluations { project: String, work_item: Uuid },
    /// Explicitly accept a current passing evaluation (admin/owner only).
    Accept {
        project: String,
        work_item: Uuid,
        evaluation: Uuid,
    },
    /// Link an execution task to a WorkItem in the same project.
    LinkTask {
        project: String,
        work_item: Uuid,
        task: Uuid,
    },
    /// Bootstrap ARGUS's own Project State and roadmap; idempotent.
    Bootstrap,
}

#[derive(Subcommand)]
enum PlanCommand {
    /// Create a planning request and export a manual planner packet.
    Export {
        /// Project slug (e.g. `argus`) or UUID.
        project: String,
        /// Planning kind (e.g. phase-planning, architecture-planning).
        #[arg(long, default_value = "phase-planning")]
        kind: String,
        /// Logical role.
        #[arg(long, default_value = "project-manager")]
        role: String,
        /// Purpose.
        #[arg(long, default_value = "planning")]
        purpose: String,
        /// Objective the planner must plan toward.
        #[arg(long)]
        objective: String,
        /// Output format: markdown or json.
        #[arg(long, default_value = "markdown")]
        format: String,
        /// Optional focus WorkItem UUID.
        #[arg(long)]
        work_item: Option<String>,
    },
    /// Import a PlanningResult JSON produced by a web planner.
    Import {
        /// Path to the JSON file.
        file: PathBuf,
    },
    /// Show a proposal.
    Show {
        /// Proposal UUID.
        proposal: String,
    },
    /// Show a proposal's deterministic diff.
    Diff {
        /// Proposal UUID.
        proposal: String,
    },
    /// Approve a validated proposal.
    Approve {
        /// Proposal UUID.
        proposal: String,
    },
    /// Reject a proposal.
    Reject {
        /// Proposal UUID.
        proposal: String,
    },
    /// Apply an approved proposal (transactional, idempotent).
    Apply {
        /// Proposal UUID.
        proposal: String,
    },
}

// ============================================================
// Main
// ============================================================

fn main() -> Result<()> {
    // Load .env file if present (for ARGUS_DATABASE_URL, etc.)
    dotenvy::dotenv().ok();

    let cli = Cli::parse();

    match cli.command {
        Commands::Version => {
            println!(
                "Argus v{} ({})",
                env!("CARGO_PKG_VERSION"),
                env!("CARGO_PKG_DESCRIPTION"),
            );
            Ok(())
        }
        _ => {
            // All other commands need the async runtime
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .context("Failed to build tokio runtime")?
                .block_on(async_main(cli))
        }
    }
}

async fn async_main(cli: Cli) -> Result<()> {
    // Load configuration
    let config = match &cli.config {
        Some(path) => ArgusConfig::load(path)?,
        None => ArgusConfig::load_auto()?,
    };

    // Initialize tracing
    init_tracing(&config);

    let runtime_env = RuntimeEnvironment::detect();
    validate_startup_config(&config, &cli.command, runtime_env)?;

    let security = CliSecurity::from_config(&config);

    match cli.command {
        Commands::Run => {
            run_secured_command(
                &security,
                "run",
                &[Permission::ConfigureSystem],
                cmd_run(config),
            )
            .await
        }
        Commands::Migrate => {
            run_secured_command(
                &security,
                "migrate",
                &[Permission::ConfigureSystem],
                cmd_migrate(config),
            )
            .await
        }
        Commands::Status => {
            run_secured_command(
                &security,
                "status",
                &[Permission::ReadProject],
                cmd_status(config),
            )
            .await
        }
        Commands::Project { command } => {
            let permissions = project_permissions(&command);
            run_secured_command(
                &security,
                "project",
                &permissions,
                cmd_project(config, command),
            )
            .await
        }
        Commands::Provider { command } => {
            run_secured_command(
                &security,
                "provider",
                &[Permission::ReadProject],
                cmd_provider(config, command),
            )
            .await
        }
        Commands::Auth { command } => {
            run_secured_command(
                &security,
                "auth",
                &[Permission::ConfigureSystem],
                cmd_auth(config, command),
            )
            .await
        }
        Commands::Quota { command } => {
            run_secured_command(
                &security,
                "quota",
                &[Permission::ReadProject],
                cmd_quota(config, command),
            )
            .await
        }
        Commands::Route {
            routing_decision,
            role,
            purpose,
            estimated_cost_usd,
        } => {
            run_secured_command(
                &security,
                "route",
                &[Permission::ReadProject],
                cmd_route(config, routing_decision, role, purpose, estimated_cost_usd),
            )
            .await
        }
        Commands::Mvp { prompt, workspace } => {
            run_secured_command(
                &security,
                "mvp",
                &[Permission::WriteCode, Permission::ExecuteCommand],
                cmd_mvp(prompt, workspace),
            )
            .await
        }
        Commands::SchedulerCrashProbe { phase, task_id } => {
            run_secured_command(
                &security,
                "scheduler-crash-probe",
                &[Permission::ConfigureSystem],
                cmd_scheduler_crash_probe(config, &phase, task_id.as_deref()),
            )
            .await
        }
        Commands::Version => unreachable!(), // handled synchronously above
    }
}

// ============================================================
// Command implementations
// ============================================================

/// `argus run` — Start the Argus daemon.
async fn cmd_run(config: ArgusConfig) -> Result<()> {
    info!(version = env!("CARGO_PKG_VERSION"), "Starting Argus");
    let driver = argus_worker::agent::driver_for(
        config.execution.default_worker.worker_type(),
        std::env::var("ARGUS_AGENT_BINARY_OVERRIDE").ok(),
    );
    let version = argus_worker::agent::check_agent_startup(driver.as_ref())
        .await
        .map_err(anyhow::Error::msg)?;
    info!(executor = driver.executor_type(), binary = driver.binary(), %version, "Agent startup check passed");

    // Connect to database
    let pool = create_db_pool(&config).await?;
    info!("Connected to PostgreSQL");
    let http_security = Arc::new(http_security::HttpSecurity::from_env(pool.clone())?);

    // Run migrations if configured
    if config.database.run_migrations {
        info!("Running database migrations...");
        sqlx::migrate!("../../migrations")
            .run(&pool)
            .await
            .context("Failed to run database migrations")?;
        info!("Migrations complete");
    }

    // Create event bus
    let event_bus = argus_core::event_bus::shared_event_bus();
    info!(
        capacity = config.system.event_bus_capacity,
        "Event bus initialized"
    );

    // Publish system started event
    use argus_core::events::{ArgusEvent, EventSource};
    event_bus.publish(
        EventSource::System,
        ArgusEvent::SystemStarted {
            version: env!("CARGO_PKG_VERSION").to_string(),
            modules_loaded: vec![
                "core".to_string(),
                "event_bus".to_string(),
                "config".to_string(),
            ],
        },
    );

    let gateway = init_social_gateway(&config);
    info!("Social gateway initialized");

    let llm = init_llm_router(&event_bus).await?;
    if llm.is_some() {
        info!("LLM router initialized from providers config");
    } else {
        info!("LLM router not configured (providers config missing)");
    }

    // Initialize Brain Engine
    let brain = Arc::new(BrainEngine::default());
    let scheduler = TaskScheduler::new_with_repository(
        Arc::new(PostgresTaskRepository::new(pool.clone())),
        Some(event_bus.clone()),
    );

    // Restore persisted tasks with detailed reporting
    let recovery_report = scheduler
        .restore()
        .await
        .context("Failed to restore scheduler state")?;

    // Print startup recovery report
    print_recovery_report(&recovery_report);

    let scheduler = Arc::new(scheduler);

    // Initialize accounting state (tracks spend ledger health)
    let accounting_state = RuntimeAccountingState::new();
    info!("Accounting state initialized: HEALTHY");

    // Initialize spend tracking and budget enforcement
    let spend_repo: Arc<dyn argus_core::spend_repository::SpendRepository> =
        Arc::new(PostgresSpendRepository::new(pool.clone()));
    let budget_policy = BudgetPolicy {
        daily_budget_usd: config.budget.daily_budget_usd,
        max_per_task_usd: config.budget.max_per_task_usd,
        strict_mode: true,
    };
    let budget_service = Arc::new(
        BudgetService::new(Arc::clone(&spend_repo), budget_policy)
            .with_accounting_state(accounting_state.clone()),
    );
    info!("Spend tracking initialized with PostgresSpendRepository");

    // Initialize ExecutionLoop with real AgentTaskExecutor
    //
    // V0.38 Phase 3: two distinct roots.
    // - ARGUS_WORKSPACE_ROOT: canonical, non-worktree repository per project
    //   (`<root>/<project_id>`) — never used as Agent cwd once workspace
    //   isolation is attached below.
    // - ARGUS_WORKTREE_ROOT: isolated Git worktrees per execution
    //   (`<root>/<project_id>/<execution_id>`) — this is where the Agent runs.
    let workspace_root = PathBuf::from(
        std::env::var("ARGUS_WORKSPACE_ROOT").unwrap_or_else(|_| ".argus/workspace".to_string()),
    );
    let worktree_root = PathBuf::from(
        std::env::var("ARGUS_WORKTREE_ROOT").unwrap_or_else(|_| ".argus/worktrees".to_string()),
    );
    let executor = Arc::new(argus_worker::AgentTaskExecutor::new(
        Arc::from(driver),
        workspace_root.clone(),
    ));

    let workspace_config = argus_core::workspace_manager::WorkspaceConfig::new(&worktree_root);
    workspace_config
        .validate()
        .context("Failed to prepare Git worktree root")?;
    let workspace_manager = Arc::new(argus_core::workspace_manager::WorkspaceManager::new(
        workspace_config,
    ));
    let workspace_repo: Arc<dyn argus_core::workspace_repository::WorkspaceRepository> =
        Arc::new(argus_core::workspace_repository::PostgresWorkspaceRepository::new(pool.clone()));
    let workspace_guard: Arc<dyn argus_core::workspace_manager::WorkspaceGuard> =
        Arc::new(argus_git::worktree_guard::GitWorkspaceGuard::new());

    // V0.38 Phase 4: Git Checkpoint + Task Artifact. Always wired alongside
    // Workspace isolation in production — a task's verified worktree
    // changes only become Completed once they're a durable Git commit with
    // a persisted Artifact record (see `argus_core::checkpoint`).
    let checkpointer: Arc<dyn argus_core::checkpoint::GitCheckpointer> =
        Arc::new(argus_git::checkpoint::GitCommitCheckpointer::new());
    let artifact_repo: Arc<dyn argus_core::artifact_repository::ArtifactRepository> =
        Arc::new(argus_core::artifact_repository::PostgresArtifactRepository::new(pool.clone()));

    let loop_config = ExecutionLoopConfig::default();
    let execution_loop_handle = ExecutionLoop::new(Arc::clone(&scheduler), executor, loop_config)
        .with_budget_service(budget_service.clone())
        .with_spend_repo(spend_repo)
        .with_accounting_state(accounting_state.clone())
        .with_workspace(
            workspace_repo,
            workspace_manager,
            workspace_guard,
            workspace_root,
        )
        .with_checkpoint(checkpointer, artifact_repo)
        .with_evaluation(Arc::new(argus_core::evaluation::EvaluationHarness::new(
            pool.clone(),
        )))
        .spawn();
    info!(
        "ExecutionLoop started with AgentTaskExecutor, BudgetService, SpendRepository, AccountingState, Workspace isolation & Git Checkpoint"
    );

    let app_state = Arc::new(AppState {
        gateway,
        conversation: ConversationManager::new(),
        llm,
        default_model_class: ModelClass::Coding,
        qq_bot_secret: if config.social.qq.bot_secret.trim().is_empty() {
            None
        } else {
            Some(config.social.qq.bot_secret.clone())
        },
        db_pool: pool.clone(),
        brain,
    });

    // Create API state with database pool
    let mut api_state = api::ApiState::with_event_bus(pool.clone(), scheduler, event_bus.clone());
    api_state.runtime_config = api::safe_config(&config);
    let api_state = Arc::new(api_state);

    // Create API router with its own state
    let api_router = Router::new()
        .route("/api/status", get(api::get_status))
        .route("/api/scheduler/stats", get(api::get_scheduler_stats))
        .route(
            "/api/projects",
            get(api::get_projects).post(api::create_project),
        )
        // V0.39 Project State — read-mostly views over structured state.
        .route(
            "/api/projects/{project_id}/state",
            get(api::get_project_state),
        )
        .route(
            "/api/projects/{project_id}/snapshot",
            get(api::get_project_snapshot),
        )
        // V0.40 Roadmap & Progress Engine.
        .route(
            "/api/projects/{project_id}/roadmap",
            get(api::get_project_roadmap),
        )
        .route(
            "/api/projects/{project_id}/progress",
            get(api::get_project_progress),
        )
        .route(
            "/api/projects/{project_id}/progress/diff",
            get(api::get_project_progress_diff),
        )
        .route(
            "/api/projects/{project_id}/progress/record",
            post(api::record_project_progress),
        )
        // V0.41 Context Compiler (deterministic; no model call).
        .route(
            "/api/projects/{project_id}/context/compile",
            post(api::compile_project_context),
        )
        // V0.42 Task Intelligence / Model Routing (deterministic; no model call).
        .route(
            "/api/projects/{project_id}/intelligence/classify",
            post(api::classify_task),
        )
        .route(
            "/api/projects/{project_id}/routing/stats",
            get(api::get_routing_stats),
        )
        .route(
            "/api/routing/{routing_id}/outcome",
            post(api::record_routing_outcome),
        )
        .route(
            "/api/routing/{routing_id}/failure",
            post(api::record_routing_failure),
        )
        // V0.43 Planner Harness / Protocol (no model call; explicit apply).
        .route(
            "/api/projects/{project_id}/planning/requests",
            post(api::create_planning_request),
        )
        .route(
            "/api/planning/requests/{request_id}/packet",
            get(api::get_planning_packet),
        )
        .route(
            "/api/planning/results/import",
            post(api::import_planning_result),
        )
        .route(
            "/api/planning/proposals/{proposal_id}",
            get(api::get_planning_proposal),
        )
        .route(
            "/api/planning/proposals/{proposal_id}/diff",
            get(api::get_planning_proposal_diff),
        )
        .route(
            "/api/planning/proposals/{proposal_id}/approve",
            post(api::approve_planning_proposal),
        )
        .route(
            "/api/planning/proposals/{proposal_id}/reject",
            post(api::reject_planning_proposal),
        )
        .route(
            "/api/planning/proposals/{proposal_id}/apply",
            post(api::apply_planning_proposal),
        )
        // V0.44 Provider / Auth / Subscription / Quota.
        .route("/api/providers", get(api::list_providers))
        .route("/api/providers/status", get(api::providers_status))
        .route(
            "/api/providers/{provider_id}/models",
            get(api::list_provider_models),
        )
        .route(
            "/api/auth-profiles",
            get(api::list_auth_profiles).post(api::register_auth_profile),
        )
        .route(
            "/api/auth-profiles/{auth_profile_id}/quota",
            get(api::get_auth_quota),
        )
        .route(
            "/api/auth-profiles/{auth_profile_id}/quota/refresh",
            post(api::refresh_auth_quota),
        )
        .route(
            "/api/projects/{project_id}/provider-routing/select",
            post(api::select_provider_route),
        )
        .route(
            "/api/projects/{project_id}/provider-routing/policy",
            get(api::get_provider_policy).put(api::upsert_provider_policy),
        )
        .route(
            "/api/projects/{project_id}/provider-routing/selections",
            get(api::list_provider_selections),
        )
        .route(
            "/api/projects/{project_id}/milestones",
            post(api::create_milestone_state),
        )
        .route(
            "/api/projects/{project_id}/work-items",
            post(api::create_work_item_state),
        )
        .route(
            "/api/projects/{project_id}/decisions",
            post(api::record_decision_state),
        )
        .route(
            "/api/projects/{project_id}/risks",
            post(api::create_risk_state),
        )
        .route(
            "/api/projects/{project_id}/blockers",
            post(api::create_blocker_state),
        )
        .route(
            "/api/projects/{project_id}/improvements",
            post(api::create_improvement_state),
        )
        .route(
            "/api/projects/{project_id}/activity",
            post(api::record_activity_state),
        )
        .route("/api/tasks", get(api::get_tasks).post(api::create_task))
        .route(
            "/api/projects/{project_id}/work-items/{work_item_id}/tasks/{task_id}",
            post(api::link_evaluation_task),
        )
        .route(
            "/api/projects/{project_id}/work-items/{work_item_id}/evaluate",
            post(api::evaluate_work_item),
        )
        .route(
            "/api/projects/{project_id}/work-items/{work_item_id}/evaluations",
            get(api::list_evaluations),
        )
        .route(
            "/api/projects/{project_id}/work-items/{work_item_id}/accept",
            post(api::accept_work_item),
        )
        .route("/api/tasks/{task_id}", get(api::get_task))
        .route("/api/deployments", get(api::get_deployments))
        .route("/api/events", get(api::event_stream))
        .route("/api/openapi.json", get(api::get_openapi))
        .route("/api/logs", get(api::get_logs))
        .route("/api/llm-usage", get(api::get_llm_usage))
        .route("/api/config", get(api::get_config).put(api::update_config))
        .with_state(api_state)
        .layer(from_fn_with_state(
            http_security.clone(),
            http_security::authorize,
        ));

    // Create webhook router with app_state
    let webhook_router = http_security::webhook_router(
        std::env::var("ARGUS_TELEGRAM_BOT_TOKEN").is_ok_and(|token| !token.trim().is_empty()),
        &config.social,
        post(webhook_telegram),
        post(webhook_qq),
    )
    .with_state(app_state.clone());
    let chat_router = Router::new()
        .route("/api/chat", post(api_chat))
        .with_state(app_state)
        .layer(from_fn_with_state(http_security, http_security::authorize));

    // Merge both routers
    let app = api_router
        .merge(webhook_router)
        .merge(chat_router)
        .layer(from_fn(trace_middleware));

    let bind_addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("Failed to bind HTTP server at {bind_addr}"))?;

    info!(addr = %bind_addr, "Argus started — HTTP server listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("HTTP server failed")?;

    // Graceful shutdown
    info!("Shutting down gracefully...");
    event_bus.publish(
        EventSource::System,
        ArgusEvent::SystemShuttingDown {
            reason: "Signal received".to_string(),
        },
    );

    // Shutdown ExecutionLoop
    execution_loop_handle.shutdown().await;
    info!("ExecutionLoop stopped");

    // Close database pool
    pool.close().await;
    info!("Database connections closed");

    info!("Argus stopped");
    Ok(())
}

/// `argus migrate` — Run database migrations only.
async fn cmd_migrate(config: ArgusConfig) -> Result<()> {
    info!("Running database migrations...");

    let pool = create_db_pool(&config).await?;
    info!("Connected to PostgreSQL");

    sqlx::migrate!("../../migrations")
        .run(&pool)
        .await
        .context("Failed to run database migrations")?;

    info!("All migrations applied successfully");

    pool.close().await;
    Ok(())
}

/// `argus status` — Show system status (placeholder for P0).
async fn cmd_status(config: ArgusConfig) -> Result<()> {
    println!("Argus v{}", env!("CARGO_PKG_VERSION"));
    println!();

    // Try to connect to database
    print!("Database: ");
    match create_db_pool(&config).await {
        Ok(pool) => {
            let row: (i64,) = sqlx::query_as("SELECT 1::BIGINT")
                .fetch_one(&pool)
                .await
                .context("Database health check failed")?;
            assert_eq!(row.0, 1);
            println!("connected ({})", config.database.url);
            pool.close().await;
        }
        Err(e) => {
            println!("UNAVAILABLE — {e}");
        }
    }

    println!("Server:   {}:{}", config.server.host, config.server.port);
    println!("Workers:  max_pool_size={}", config.worker.max_pool_size);
    println!("LLM:      provider={}", config.llm.default_provider);
    println!(
        "Memory:   ttl={}h, decay={}",
        config.memory.short_term_ttl_hours, config.memory.decay_rate
    );

    Ok(())
}

fn build_provider_service(
    pool: &sqlx::PgPool,
) -> std::sync::Arc<argus_core::provider::ProviderService> {
    let budget = std::sync::Arc::new(argus_core::budget_service::BudgetService::new(
        std::sync::Arc::new(argus_core::spend_repository::PostgresSpendRepository::new(
            pool.clone(),
        )),
        argus_core::budget_service::BudgetPolicy::default(),
    ));
    let adapters: Vec<std::sync::Arc<dyn argus_core::provider::ProviderAdapter>> = vec![
        std::sync::Arc::new(argus_core::provider::CliProviderAdapter::new(
            argus_core::provider::RuntimeProvider::ClaudeCodeCli,
        )),
        std::sync::Arc::new(argus_core::provider::CliProviderAdapter::new(
            argus_core::provider::RuntimeProvider::CodexCli,
        )),
        std::sync::Arc::new(argus_core::provider::CliProviderAdapter::new(
            argus_core::provider::RuntimeProvider::OpenCodeCli,
        )),
    ];
    std::sync::Arc::new(argus_core::provider::ProviderService::new(
        std::sync::Arc::new(argus_core::provider::PostgresProviderRepository::new(
            pool.clone(),
        )),
        std::sync::Arc::new(argus_core::provider::UnknownQuotaProvider),
        Some(budget),
        adapters,
    ))
}

/// `argus provider list|status|discover`
async fn cmd_provider(config: ArgusConfig, command: ProviderCommand) -> Result<()> {
    let pool = create_db_pool(&config).await?;
    let service = build_provider_service(&pool);
    match command {
        ProviderCommand::List => {
            for p in service.registry().await.providers() {
                let billing: Vec<&str> = p.billing_modes.iter().map(|b| b.as_str()).collect();
                println!(
                    "{:<20} {:<16} enabled={} priority={} billing={}",
                    p.display_name,
                    p.provider_id.as_str(),
                    p.enabled,
                    p.priority,
                    billing.join(",")
                );
            }
        }
        ProviderCommand::Discover => {
            for (provider, discovery) in service.discover_all().await {
                println!(
                    "{:<16} installed={} version={} path={}",
                    provider.as_str(),
                    discovery.installed,
                    discovery.version.as_deref().unwrap_or("unknown"),
                    discovery.path.as_deref().unwrap_or("-")
                );
            }
        }
        ProviderCommand::Status => {
            use argus_core::provider::AuthStatus;
            let state = service.state().await?;
            for p in &state.providers {
                let id = p.provider_id;
                let discovery = state.discovery_for(id);
                // Auth is summarized from ARGUS's own AuthProfile rows, never
                // scraped from the CLI's private credential state.
                let profiles = state.auth_for(id);
                let auth = if profiles.is_empty() {
                    "no-profile".to_string()
                } else if profiles.iter().any(|a| a.status == AuthStatus::Available) {
                    "available".to_string()
                } else if profiles.iter().any(|a| a.status == AuthStatus::NeedsLogin) {
                    "needs_login".to_string()
                } else if profiles.iter().any(|a| a.status == AuthStatus::Expired) {
                    "expired".to_string()
                } else if profiles
                    .iter()
                    .any(|a| matches!(a.status, AuthStatus::Unavailable | AuthStatus::Error))
                {
                    "unavailable".to_string()
                } else {
                    "unknown".to_string()
                };
                let quota = state
                    .quota
                    .iter()
                    .filter(|q| q.provider_id == id)
                    .max_by_key(|q| q.observed_at);
                let quota_text = match quota {
                    Some(q) => format!(
                        "{}/{} remaining={}",
                        q.availability.as_str(),
                        q.confidence.as_str(),
                        q.remaining
                            .map(|r| r.to_string())
                            .unwrap_or_else(|| "unknown".to_string())
                    ),
                    None => "never-observed".to_string(),
                };
                println!(
                    "{:<16} enabled={} installed={} version={} health={} login={}({}) profile={} quota={}",
                    id.as_str(),
                    p.enabled,
                    discovery.map(|d| d.installed).unwrap_or(true),
                    discovery
                        .and_then(|d| d.version.as_deref())
                        .unwrap_or("n/a"),
                    state.health_for(id).as_str(),
                    discovery.map(|d| d.auth_status.as_str()).unwrap_or("n/a"),
                    discovery
                        .map(|d| d.auth_confidence.as_str())
                        .unwrap_or("n/a"),
                    auth,
                    quota_text
                );
            }
        }
    }
    pool.close().await;
    Ok(())
}

/// `argus auth list|status|register`
async fn cmd_auth(config: ArgusConfig, command: AuthCommand) -> Result<()> {
    let pool = create_db_pool(&config).await?;
    let service = build_provider_service(&pool);
    match command {
        AuthCommand::List => {
            for p in service.list_auth_profiles().await? {
                println!(
                    "{} {} kind={} status={} locator={}",
                    p.auth_profile_id,
                    p.provider_id.as_str(),
                    p.auth_kind.as_str(),
                    p.status.as_str(),
                    p.secret_reference
                        .as_ref()
                        .map(|r| r.redacted())
                        .unwrap_or("-")
                );
            }
        }
        AuthCommand::Status { profile } => {
            let id = Uuid::parse_str(&profile)
                .map_err(|_| anyhow::anyhow!("invalid profile UUID '{profile}'"))?;
            let profiles = service.list_auth_profiles().await?;
            let p = profiles
                .into_iter()
                .find(|p| p.auth_profile_id == id)
                .ok_or_else(|| anyhow::anyhow!("auth profile not found"))?;
            println!("Profile: {}", p.auth_profile_id);
            println!("Provider: {}", p.provider_id);
            println!("Kind: {}", p.auth_kind.as_str());
            println!("Status: {}", p.status.as_str());
            println!(
                "Credential locator: {}",
                p.secret_reference
                    .as_ref()
                    .map(|r| r.redacted())
                    .unwrap_or("-")
            );
        }
        AuthCommand::Register {
            provider,
            kind,
            account,
            secret_reference,
        } => {
            let provider_id = argus_core::provider::RuntimeProvider::parse(&provider)
                .ok_or_else(|| anyhow::anyhow!("unknown provider '{provider}'"))?;
            let auth_kind = argus_core::provider::AuthKind::parse(&kind)
                .ok_or_else(|| anyhow::anyhow!("unknown auth kind '{kind}'"))?;
            let profile = service
                .ensure_auth_profile(argus_core::provider::NewAuthProfile {
                    provider_id,
                    auth_kind,
                    account_label: account,
                    workspace_label: None,
                    status: argus_core::provider::AuthStatus::Unknown,
                    secret_reference: secret_reference
                        .map(argus_core::provider::SecretReference::new),
                    credential_source: Some("cli-registered".to_string()),
                })
                .await?;
            println!("Registered auth profile {}", profile.auth_profile_id);
        }
    }
    pool.close().await;
    Ok(())
}

/// `argus quota show|refresh`
async fn cmd_quota(config: ArgusConfig, command: QuotaCommand) -> Result<()> {
    let pool = create_db_pool(&config).await?;
    let service = build_provider_service(&pool);
    let (profile_id, refresh) = match command {
        QuotaCommand::Show { profile } => (profile, false),
        QuotaCommand::Refresh { profile } => (profile, true),
    };
    let id = Uuid::parse_str(&profile_id)
        .map_err(|_| anyhow::anyhow!("invalid profile UUID '{profile_id}'"))?;
    let profiles = service.list_auth_profiles().await?;
    let profile = profiles
        .into_iter()
        .find(|p| p.auth_profile_id == id)
        .ok_or_else(|| anyhow::anyhow!("auth profile not found"))?;
    let quota = if refresh {
        service.refresh_quota(profile.provider_id, Some(id)).await?
    } else {
        service
            .latest_quota(profile.provider_id, Some(id))
            .await?
            .unwrap_or_else(|| {
                argus_core::provider::quota::unknown_snapshot(profile.provider_id, Some(id))
            })
    };
    println!("Provider: {}", quota.provider_id);
    println!("Kind: {}", quota.quota_kind.as_str());
    println!("Availability: {}", quota.availability.as_str());
    println!(
        "Remaining: {}",
        quota
            .remaining
            .map(|r| r.to_string())
            .unwrap_or_else(|| "unknown".to_string())
    );
    println!("Source: {}", quota.source.as_str());
    println!("Confidence: {}", quota.confidence.as_str());
    pool.close().await;
    Ok(())
}

/// `argus route <routing-decision>`
async fn cmd_route(
    config: ArgusConfig,
    routing_decision: String,
    role: String,
    purpose: String,
    estimated_cost_usd: Option<f64>,
) -> Result<()> {
    let pool = create_db_pool(&config).await?;
    let state_repo: std::sync::Arc<dyn argus_core::project_state::ProjectStateRepository> =
        std::sync::Arc::new(
            argus_core::project_state::PostgresProjectStateRepository::new(pool.clone()),
        );
    let intelligence = argus_core::intelligence::TaskIntelligenceService::new(
        state_repo,
        std::sync::Arc::new(
            argus_core::intelligence::PostgresTaskIntelligenceRepository::new(pool.clone()),
        ),
    );
    let provider = build_provider_service(&pool);
    let routing_decision_id = Uuid::parse_str(&routing_decision)
        .map_err(|_| anyhow::anyhow!("invalid routing decision UUID '{routing_decision}'"))?;
    let role = argus_core::context::ContextRole::parse(&role)
        .ok_or_else(|| anyhow::anyhow!("unknown role '{role}'"))?;
    let purpose = argus_core::context::ContextPurpose::parse(&purpose)
        .ok_or_else(|| anyhow::anyhow!("unknown purpose '{purpose}'"))?;
    let decision = intelligence
        .get_routing_decision(routing_decision_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("routing decision not found"))?;
    let policy = provider.get_policy(decision.project_id).await?;
    let outcome = provider
        .route(argus_core::provider::ProviderRoutingInput {
            project_id: decision.project_id,
            routing_decision_id: Some(routing_decision_id),
            task_id: decision.task_id,
            role: role.as_str().to_string(),
            purpose: purpose.as_str().to_string(),
            required_tier: decision.required_tier,
            required_capabilities: decision.required_capabilities.clone(),
            reasoning_effort: decision.reasoning_effort,
            estimated_cost_usd,
            policy: policy.to_policy(),
        })
        .await?;
    println!("{}", serde_json::to_string_pretty(&outcome.result)?);
    if let Some(reasons) = outcome
        .result
        .selected_target()
        .map(|t| t.selection_reasons.clone())
    {
        println!("Reasons:");
        for r in reasons {
            println!("- {r}");
        }
    }
    pool.close().await;
    Ok(())
}

/// `argus project status|bootstrap` — deterministic Project State reads.
fn project_permissions(command: &ProjectCommand) -> Vec<Permission> {
    match command {
        ProjectCommand::Accept { .. }
        | ProjectCommand::Bootstrap
        | ProjectCommand::Plan {
            command:
                PlanCommand::Approve { .. } | PlanCommand::Reject { .. } | PlanCommand::Apply { .. },
        } => vec![Permission::ConfigureSystem],
        ProjectCommand::Evaluate { .. } => vec![Permission::WriteCode, Permission::ExecuteCommand],
        ProjectCommand::LinkTask { .. }
        | ProjectCommand::Classify { .. }
        | ProjectCommand::Plan {
            command: PlanCommand::Export { .. } | PlanCommand::Import { .. },
        } => vec![Permission::WriteCode],
        _ => vec![Permission::ReadProject],
    }
}

async fn resolve_project(
    service: &argus_core::project_state::ProjectStateService,
    project: &str,
) -> Result<argus_core::project_state::ProjectRecord> {
    Ok(match Uuid::parse_str(project) {
        Ok(id) => service.get_project(id).await?,
        Err(_) => service.get_project_by_slug(project).await?,
    })
}

async fn cmd_project(config: ArgusConfig, command: ProjectCommand) -> Result<()> {
    let pool = create_db_pool(&config).await?;
    let state_repo: std::sync::Arc<dyn argus_core::project_state::ProjectStateRepository> =
        std::sync::Arc::new(
            argus_core::project_state::PostgresProjectStateRepository::new(pool.clone()),
        );
    let service = argus_core::project_state::ProjectStateService::new(state_repo.clone());
    let intelligence = std::sync::Arc::new(argus_core::intelligence::TaskIntelligenceService::new(
        state_repo.clone(),
        std::sync::Arc::new(
            argus_core::intelligence::PostgresTaskIntelligenceRepository::new(pool.clone()),
        ),
    ));
    let planner = argus_core::planner::PlannerHarness::new(
        state_repo,
        std::sync::Arc::new(argus_core::planner::PostgresPlanningRepository::new(
            pool.clone(),
        )),
        Some(intelligence.clone()),
    );

    match command {
        ProjectCommand::Evaluate { project, work_item } => {
            let record = resolve_project(&service, &project).await?;
            let report = argus_core::evaluation::EvaluationHarness::new(pool.clone())
                .evaluate(record.project_id, work_item)
                .await?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        ProjectCommand::Evaluations { project, work_item } => {
            let record = resolve_project(&service, &project).await?;
            let reports = argus_core::evaluation::EvaluationHarness::new(pool.clone())
                .list(record.project_id, work_item)
                .await?;
            println!("{}", serde_json::to_string_pretty(&reports)?);
        }
        ProjectCommand::Accept {
            project,
            work_item,
            evaluation,
        } => {
            let record = resolve_project(&service, &project).await?;
            let actor = std::env::var("ARGUS_ACTOR").unwrap_or_else(|_| "argus-cli".into());
            argus_core::evaluation::EvaluationHarness::new(pool.clone())
                .accept(record.project_id, work_item, evaluation, &actor)
                .await?;
            println!("Work item accepted");
        }
        ProjectCommand::LinkTask {
            project,
            work_item,
            task,
        } => {
            let record = resolve_project(&service, &project).await?;
            let item = service
                .get_work_item(work_item)
                .await?
                .ok_or_else(|| anyhow!("Work item not found"))?;
            anyhow::ensure!(
                item.project_id == record.project_id,
                "Work item belongs to another project"
            );
            service
                .repository()
                .link_work_item_task(work_item, task)
                .await?;
            println!("Task linked");
        }
        ProjectCommand::Status { project } => {
            let record = match Uuid::parse_str(&project) {
                Ok(id) => service.get_project(id).await?,
                Err(_) => service.get_project_by_slug(&project).await?,
            };
            println!("{}", service.render_status(record.project_id).await?);
        }
        ProjectCommand::Progress { project } => {
            let record = match Uuid::parse_str(&project) {
                Ok(id) => service.get_project(id).await?,
                Err(_) => service.get_project_by_slug(&project).await?,
            };
            let report = service.get_progress_report(record.project_id).await?;
            println!("{}", record.name);
            println!(
                "Overall Verified Progress: {:.0}%",
                report.completion_percent
            );
            match &report.current_milestone {
                Some(m) => println!("Current Milestone: {}", m.title),
                None => println!("Current Milestone: None"),
            }
            if let Some(p) = &report.current_phase {
                println!("Current Phase: {}", p.title);
            }
            println!("Active: {}", report.active_work_items.len());
            println!("Next: {}", report.next_candidates.len());
        }
        ProjectCommand::Context {
            project,
            role,
            purpose,
            format,
        } => {
            let record = match Uuid::parse_str(&project) {
                Ok(id) => service.get_project(id).await?,
                Err(_) => service.get_project_by_slug(&project).await?,
            };
            let role = argus_core::context::ContextRole::parse(&role).ok_or_else(|| {
                anyhow::anyhow!(
                    "unknown role '{role}' (chief|project-manager|developer|tester|reviewer|researcher|integrator)"
                )
            })?;
            let purpose =
                argus_core::context::ContextPurpose::parse(&purpose).ok_or_else(|| {
                    anyhow::anyhow!(
                        "unknown purpose '{purpose}' (planning|execution|review|debugging|research|integration|status-review)"
                    )
                })?;
            let request =
                argus_core::context::ContextRequest::new(record.project_id, role, purpose);
            let packet = service.compile_context(&request).await?;
            match argus_core::context::ContextFormat::parse(&format) {
                Some(argus_core::context::ContextFormat::Json) => {
                    println!("{}", argus_core::context::render_json(&packet));
                }
                Some(argus_core::context::ContextFormat::Markdown) | None => {
                    println!("{}", argus_core::context::render_markdown(&packet));
                }
            }
        }
        ProjectCommand::Classify {
            project,
            work_item,
            role,
            purpose,
        } => {
            let record = match Uuid::parse_str(&project) {
                Ok(id) => service.get_project(id).await?,
                Err(_) => service.get_project_by_slug(&project).await?,
            };
            let work_item_id = Uuid::parse_str(&work_item)
                .map_err(|_| anyhow::anyhow!("invalid work_item UUID '{work_item}'"))?;
            let role = argus_core::context::ContextRole::parse(&role)
                .ok_or_else(|| anyhow::anyhow!("unknown role '{role}'"))?;
            let purpose = argus_core::context::ContextPurpose::parse(&purpose)
                .ok_or_else(|| anyhow::anyhow!("unknown purpose '{purpose}'"))?;
            let (profile, decision) = intelligence
                .classify_work_item(record.project_id, work_item_id, role, purpose)
                .await?;
            let title = service
                .get_work_item(work_item_id)
                .await?
                .map(|w| w.title)
                .unwrap_or_else(|| work_item_id.to_string());
            println!("Task: {title}");
            println!();
            println!("Complexity: {:.2}", profile.complexity_score);
            println!("Risk: {:.2}", profile.risk_score);
            println!();
            println!(
                "Tier: {} (minimum {}; preferred {})",
                profile.recommended_tier, decision.required_tier, decision.preferred_tier
            );
            println!("Reasoning: {}", decision.reasoning_effort);
            println!("Verification: {}", decision.verification_requirement);
            println!("Context: {}", decision.context_requirement);
            println!();
            println!("Task Kind: {}", profile.task_kind);
            println!("Confidence: {:.2}", profile.confidence);
            if !profile.required_capabilities.is_empty() {
                let caps: Vec<&str> = profile
                    .required_capabilities
                    .iter()
                    .map(|c| c.as_str())
                    .collect();
                println!("Capabilities: {}", caps.join(", "));
            }
            println!();
            println!("Reasons:");
            for reason in &decision.reasons {
                println!("- {reason}");
            }
            let extra: Vec<&String> = profile
                .reasons
                .iter()
                .filter(|r| !decision.reasons.contains(r))
                .collect();
            for reason in extra {
                println!("- {reason}");
            }
            println!();
            println!("Profile Hash: {}", profile.input_hash);
            println!("Routing Hash: {}", decision.input_hash);
        }
        ProjectCommand::Plan { command } => match command {
            PlanCommand::Export {
                project,
                kind,
                role,
                purpose,
                objective,
                format,
                work_item,
            } => {
                let record = match Uuid::parse_str(&project) {
                    Ok(id) => service.get_project(id).await?,
                    Err(_) => service.get_project_by_slug(&project).await?,
                };
                let planning_kind = argus_core::planner::PlanningKind::parse(&kind)
                    .ok_or_else(|| anyhow::anyhow!("unknown planning kind '{kind}'"))?;
                let role = argus_core::context::ContextRole::parse(&role)
                    .ok_or_else(|| anyhow::anyhow!("unknown role '{role}'"))?;
                let purpose = argus_core::context::ContextPurpose::parse(&purpose)
                    .ok_or_else(|| anyhow::anyhow!("unknown purpose '{purpose}'"))?;
                let work_item_id = match work_item {
                    Some(s) => Some(
                        Uuid::parse_str(&s)
                            .map_err(|_| anyhow::anyhow!("invalid work_item UUID '{s}'"))?,
                    ),
                    None => None,
                };
                let request = planner
                    .create_planning_request(argus_core::planner::CreatePlanningRequestInput {
                        project_id: record.project_id,
                        planning_kind,
                        role,
                        purpose,
                        objective,
                        constraints: vec![],
                        requested_outputs: vec![
                            argus_core::planner::RequestedOutput::RoadmapChanges,
                            argus_core::planner::RequestedOutput::WorkItems,
                        ],
                        work_item_id,
                    })
                    .await?;
                let packet = planner.export_packet(request.planning_request_id).await?;
                match argus_core::context::ContextFormat::parse(&format) {
                    Some(argus_core::context::ContextFormat::Json) => println!("{packet}"),
                    _ => println!("{packet}"),
                }
                eprintln!("Planning Request ID: {}", request.planning_request_id);
            }
            PlanCommand::Import { file } => {
                let content = std::fs::read_to_string(&file)
                    .with_context(|| format!("read {}", file.display()))?;
                let result: argus_core::planner::PlanningResult =
                    serde_json::from_str(&content).context("parse PlanningResult JSON")?;
                let outcome = planner.import_result(result).await?;
                println!("Proposal: {}", outcome.proposal.proposal_id);
                println!("Status: {:?}", outcome.proposal.status);
                if !outcome.validation_errors.is_empty() {
                    println!("Validation errors:");
                    for error in &outcome.validation_errors {
                        println!("- {error}");
                    }
                }
                println!("Diff: {}", outcome.diff.summary());
            }
            PlanCommand::Show { proposal } => {
                let id = Uuid::parse_str(&proposal)
                    .map_err(|_| anyhow::anyhow!("invalid proposal UUID '{proposal}'"))?;
                let record = planner.get_proposal(id).await?;
                println!("{}", serde_json::to_string_pretty(&record)?);
            }
            PlanCommand::Diff { proposal } => {
                let id = Uuid::parse_str(&proposal)
                    .map_err(|_| anyhow::anyhow!("invalid proposal UUID '{proposal}'"))?;
                let diff = planner.get_diff(id).await?;
                println!("{}", diff.summary());
                for entry in &diff.entries {
                    println!(
                        "{:?} {:?} {}",
                        entry.entity_type, entry.operation, entry.label
                    );
                }
            }
            PlanCommand::Approve { proposal } => {
                let id = Uuid::parse_str(&proposal)
                    .map_err(|_| anyhow::anyhow!("invalid proposal UUID '{proposal}'"))?;
                let record = planner.approve_proposal(id).await?;
                println!("Proposal {} -> {:?}", record.proposal_id, record.status);
            }
            PlanCommand::Reject { proposal } => {
                let id = Uuid::parse_str(&proposal)
                    .map_err(|_| anyhow::anyhow!("invalid proposal UUID '{proposal}'"))?;
                let record = planner.reject_proposal(id).await?;
                println!("Proposal {} -> {:?}", record.proposal_id, record.status);
            }
            PlanCommand::Apply { proposal } => {
                let id = Uuid::parse_str(&proposal)
                    .map_err(|_| anyhow::anyhow!("invalid proposal UUID '{proposal}'"))?;
                let outcome = planner.apply_proposal(id).await?;
                if outcome.already_applied {
                    println!("AlreadyApplied: {}", outcome.proposal_id);
                } else {
                    println!(
                        "Applied {}: +{} milestone, +{} phase, +{} work item, +{} decision, +{} risk, +{} blocker, +{} deferred",
                        outcome.proposal_id,
                        outcome.milestones_created,
                        outcome.phases_created,
                        outcome.work_items_created,
                        outcome.decisions_created,
                        outcome.risks_created,
                        outcome.blockers_created,
                        outcome.deferred_improvements_created
                    );
                }
            }
        },
        ProjectCommand::Bootstrap => {
            let record = service.bootstrap_argus().await?;
            println!(
                "Bootstrapped project '{}' (slug: {}, id: {})",
                record.name, record.slug, record.project_id
            );
            println!();
            println!("{}", service.render_status(record.project_id).await?);
        }
    }

    pool.close().await;
    Ok(())
}

/// `argus mvp` — Run the single-project MVP flow.
async fn cmd_mvp(prompt: String, workspace: PathBuf) -> Result<()> {
    info!("Running P6 MVP flow");

    let config = ArgusConfig::load_auto()?;
    let pool = create_db_pool(&config).await?;
    let embedding = EmbeddingEngine::new()?;
    let event_bus = argus_core::event_bus::shared_event_bus();
    let memory_store = MemoryStore::new(pool.clone(), std::sync::Arc::new(embedding), event_bus);

    let orchestrator = argus_core::mvp::MvpOrchestrator::new();
    let request = argus_core::mvp::MvpRequest {
        prompt,
        workspace_dir: workspace,
    };

    let result = orchestrator.run(request).await?;

    if result.completed_tasks < result.task_count {
        bail!(
            "MVP run incomplete: tasks={}, completed={}",
            result.task_count,
            result.completed_tasks
        );
    }

    let summary = format!(
        "MVP run completed: tasks={}, completed={}",
        result.task_count, result.completed_tasks
    );
    memory_store
        .store(NewMemory {
            content: summary,
            layer: argus_core::types::MemoryLayer::ShortTerm,
            memory_type: argus_core::types::MemoryType::Experience,
            importance: 0.6,
            project_id: None,
            task_id: None,
            tags: vec!["mvp".to_string(), "p6".to_string()],
            source_type: Some("mvp_orchestrator".to_string()),
            source_model: None,
        })
        .await?;

    pool.close().await;

    println!(
        "MVP completed: tasks={}, completed={}, committed={}",
        result.task_count, result.completed_tasks, result.committed
    );

    Ok(())
}

async fn cmd_scheduler_crash_probe(
    config: ArgusConfig,
    phase: &str,
    task_id: Option<&str>,
) -> Result<()> {
    let pool = create_db_pool(&config).await?;
    sqlx::migrate!("../../migrations")
        .run(&pool)
        .await
        .context("Failed to run probe migrations")?;
    let scheduler = TaskScheduler::new_with_repository(
        Arc::new(PostgresTaskRepository::new(pool.clone())),
        None,
    );

    match phase {
        "prepare" => {
            let project_id = Uuid::new_v4();
            let project_name = format!("crash-probe-{}", &project_id.to_string()[..8]);
            sqlx::query(
                r#"
                INSERT INTO projects (id, name, description, git_local_path, active, priority)
                VALUES ($1, $2, 'scheduler crash probe', $3, true, 'medium')
                "#,
            )
            .bind(project_id)
            .bind(&project_name)
            .bind(format!("/tmp/argus/projects/{project_name}"))
            .execute(&pool)
            .await?;

            let task = TaskBuilder::new(project_id, "Crash recovery probe").build();
            let task_id = task.id;
            scheduler.add_task_durable(task).await?;
            if !scheduler.claim_task(task_id).await?
                || !scheduler.transition(task_id, TaskState::Assigned).await
                || !scheduler.transition(task_id, TaskState::Running).await
            {
                bail!("failed to advance crash probe task to running");
            }
            let task = scheduler
                .get_task(task_id)
                .await
                .context("probe task missing after transition")?;
            println!(
                "ARGUS_CRASH_PROBE {}",
                serde_json::json!({
                    "task_id": task.id,
                    "project_id": project_id,
                    "execution_id": task.execution_id,
                    "state": task.state,
                })
            );
            std::io::stdout().flush()?;
            std::future::pending::<()>().await;
            Ok(())
        }
        "resume" => {
            let task_id = Uuid::from_str(task_id.context("--task-id is required for resume")?)
                .context("invalid --task-id")?;
            scheduler.restore().await?;
            let restored = scheduler
                .get_task(task_id)
                .await
                .context("probe task was not restored")?;
            if restored.state != TaskState::Ready || restored.resume_count != 1 {
                bail!(
                    "unexpected restored state: state={}, resume_count={}",
                    restored.state,
                    restored.resume_count
                );
            }
            let execution_id = restored
                .execution_id
                .context("restored task lost execution id")?;
            if !scheduler.claim_task(task_id).await?
                || !scheduler.transition(task_id, TaskState::Assigned).await
                || !scheduler.transition(task_id, TaskState::Running).await
                || !scheduler.transition(task_id, TaskState::Testing).await
            {
                bail!("failed to resume crash probe task");
            }
            if !scheduler
                .submit_task_output(
                    task_id,
                    TaskOutput {
                        summary: "resumed exactly once".to_string(),
                        ..TaskOutput::default()
                    },
                )
                .await
            {
                bail!("failed to persist resumed task output");
            }
            let report = argus_core::quality::QualityReport {
                task_id: task_id.to_string(),
                overall: argus_core::quality::QualityResult::Passed,
                stages: Vec::new(),
                total_duration_ms: 0,
                recommendations: Vec::new(),
            };
            if !scheduler.finalize_task(task_id, &report).await? {
                bail!("failed to finalize resumed crash probe task");
            }

            let attempt_count: (i64,) =
                sqlx::query_as("SELECT COUNT(*) FROM task_execution_attempts WHERE task_id = $1")
                    .bind(task_id)
                    .fetch_one(&pool)
                    .await?;
            if attempt_count.0 != 1 {
                bail!("expected one logical execution, got {}", attempt_count.0);
            }
            println!(
                "ARGUS_RESUME_PROBE {}",
                serde_json::json!({
                    "task_id": task_id,
                    "execution_id": execution_id,
                    "resume_count": 1,
                    "logical_execution_count": attempt_count.0,
                    "state": "completed",
                })
            );
            Ok(())
        }
        other => bail!("unsupported probe phase: {other}"),
    }
}

#[derive(Clone)]
struct AppState {
    gateway: Arc<SocialGateway>,
    conversation: ConversationManager,
    llm: Option<Arc<ProviderRouter>>,
    default_model_class: ModelClass,
    qq_bot_secret: Option<String>,
    db_pool: sqlx::PgPool,
    brain: Arc<BrainEngine>,
}

#[derive(Clone, Debug)]
struct TraceId(String);

async fn trace_middleware(mut request: Request, next: Next) -> Response {
    let trace_id = request
        .headers()
        .get("x-trace-id")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let started = std::time::Instant::now();

    request.extensions_mut().insert(TraceId(trace_id.clone()));

    let mut response = next.run(request).await;
    let elapsed_ms = started.elapsed().as_millis() as u64;
    let status = response.status();

    if let Ok(header_value) = HeaderValue::from_str(&trace_id) {
        response.headers_mut().insert("x-trace-id", header_value);
    }

    tracing::info!(
        trace_id,
        method = %method,
        path,
        status = status.as_u16(),
        elapsed_ms,
        "HTTP request completed"
    );

    response
}

#[derive(Debug, Deserialize)]
struct ApiChatRequest {
    session_id: String,
    user_id: String,
    text: String,
}

#[derive(Debug, Serialize)]
struct ApiChatResponse {
    reply: String,
    provider: Option<String>,
    model: Option<String>,
}

#[derive(Debug, Serialize)]
struct WebhookAck {
    ok: bool,
    message: String,
}

async fn api_chat(
    State(state): State<Arc<AppState>>,
    Extension(trace_id): Extension<TraceId>,
    Json(req): Json<ApiChatRequest>,
) -> Result<Json<ApiChatResponse>, (StatusCode, Json<WebhookAck>)> {
    let payload = serde_json::json!({
        "session_id": req.session_id,
        "user_id": req.user_id,
        "text": req.text,
    });

    match handle_incoming_message(&state, Platform::WebDashboard, payload, false, &trace_id.0).await
    {
        Ok(reply) => Ok(Json(ApiChatResponse {
            reply,
            provider: None,
            model: None,
        })),
        Err(err) => Err((
            StatusCode::BAD_REQUEST,
            Json(WebhookAck {
                ok: false,
                message: err.to_string(),
            }),
        )),
    }
}

async fn webhook_telegram(
    State(state): State<Arc<AppState>>,
    Extension(trace_id): Extension<TraceId>,
    Json(payload): Json<serde_json::Value>,
) -> Json<WebhookAck> {
    let normalized = payload
        .get("message")
        .map(|msg| {
            serde_json::json!({
                "chat_id": msg.get("chat").and_then(|v| v.get("id")).map(|v| v.to_string().trim_matches('"').to_string()).unwrap_or_default(),
                "user_id": msg.get("from").and_then(|v| v.get("id")).map(|v| v.to_string().trim_matches('"').to_string()).unwrap_or_default(),
                "text": msg.get("text").and_then(|v| v.as_str()).unwrap_or_default(),
            })
        })
        .unwrap_or(payload);

    webhook_response(
        handle_incoming_message(&state, Platform::Telegram, normalized, true, &trace_id.0).await,
    )
}

#[derive(Debug, Deserialize)]
struct QqValidationRequest {
    plain_token: String,
    event_ts: String,
}

#[derive(Debug, Serialize)]
struct QqValidationResponse {
    plain_token: String,
    signature: String,
}

async fn webhook_qq(
    State(state): State<Arc<AppState>>,
    Extension(trace_id): Extension<TraceId>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let bot_secret = state.qq_bot_secret.clone().unwrap_or_default();
    if !bot_secret.trim().is_empty() && !verify_qq_signature(&headers, &body, &bot_secret) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "ok": false,
                "message": "qq signature verification failed"
            })),
        )
            .into_response();
    }

    let payload: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "ok": false,
                    "message": format!("invalid qq payload: {err}")
                })),
            )
                .into_response();
        }
    };

    let op = payload
        .get("op")
        .and_then(|v| v.as_i64())
        .unwrap_or_default();

    if op == 13 {
        let validation = payload
            .get("d")
            .cloned()
            .and_then(|v| serde_json::from_value::<QqValidationRequest>(v).ok());

        let Some(validation) = validation else {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "ok": false,
                    "message": "qq validation payload missing d.plain_token/event_ts"
                })),
            )
                .into_response();
        };

        if bot_secret.trim().is_empty() {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "ok": false,
                    "message": "ARGUS_QQ_BOT_SECRET is required for qq callback validation"
                })),
            )
                .into_response();
        }

        match qq_validation_signature(&bot_secret, &validation.event_ts, &validation.plain_token) {
            Ok(signature) => {
                let response = QqValidationResponse {
                    plain_token: validation.plain_token,
                    signature,
                };
                return (StatusCode::OK, Json(serde_json::json!(response))).into_response();
            }
            Err(err) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "ok": false,
                        "message": format!("qq validation signature generation failed: {err}")
                    })),
                )
                    .into_response();
            }
        }
    }

    let event_type = payload
        .get("t")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if op != 0 || event_type != "C2C_MESSAGE_CREATE" {
        return Json(WebhookAck {
            ok: true,
            message: format!("ignored qq event op={op} type={event_type}"),
        })
        .into_response();
    }

    webhook_response(
        handle_incoming_message(&state, Platform::Qq, payload, true, &trace_id.0).await,
    )
    .into_response()
}

fn webhook_response(result: argus_core::types::ArgusResult<String>) -> Json<WebhookAck> {
    match result {
        Ok(msg) => Json(WebhookAck {
            ok: true,
            message: msg,
        }),
        Err(err) => Json(WebhookAck {
            ok: false,
            message: err.to_string(),
        }),
    }
}

async fn handle_incoming_message(
    state: &AppState,
    platform: Platform,
    payload: serde_json::Value,
    send_back_to_platform: bool,
    trace_id: &str,
) -> argus_core::types::ArgusResult<String> {
    tracing::info!(trace_id, ?platform, "Handling incoming social message");
    let incoming = state.gateway.normalize(platform, payload).await?;
    state.conversation.append(incoming.clone())?;

    let reply = generate_reply(state, &incoming, trace_id).await?;

    let outgoing = SocialMessage::outgoing(platform, &incoming.channel_id, "argus", &reply);
    state.conversation.append(outgoing)?;

    if send_back_to_platform {
        state
            .gateway
            .send(platform, &incoming.channel_id, &reply)
            .await?;
    }

    Ok(reply)
}

async fn generate_reply(
    state: &AppState,
    incoming: &SocialMessage,
    trace_id: &str,
) -> argus_core::types::ArgusResult<String> {
    // Step 1: Use BrainEngine to classify intent
    let intent_context = argus_core::brain::IntentContext::default();
    let classification = state
        .brain
        .classify_intent(&incoming.text, &intent_context)
        .await;

    tracing::info!(trace_id, intent = ?classification.intent, confidence = ?classification.confidence, "Classified intent");

    // Step 2: Handle specific intents with actions
    let action_response = handle_intent_action(state, incoming, &classification.intent).await?;
    if !action_response.is_empty() {
        return Ok(action_response);
    }

    // Step 3: Fall back to LLM for general conversation
    let Some(router) = &state.llm else {
        return Err(argus_core::types::ArgusError::Social(
            "LLM router is not configured. Add config/providers.toml and API keys.".to_string(),
        ));
    };

    let session = state
        .conversation
        .get(&incoming.channel_id, &incoming.user_id)?
        .ok_or_else(|| {
            argus_core::types::ArgusError::Social("conversation session not found".to_string())
        })?;

    let mut messages = vec![ChatMessage::system(
        "You are Argus assistant. Reply concisely and helpfully.",
    )];

    for msg in session.messages.iter().rev().take(12).rev() {
        match msg.direction {
            argus_social::types::MessageDirection::Incoming => {
                messages.push(ChatMessage::user(msg.text.clone()));
            }
            argus_social::types::MessageDirection::Outgoing => {
                messages.push(ChatMessage::assistant(msg.text.clone()));
            }
        }
    }

    let request = LlmRequest::new(
        state.default_model_class.clone(),
        messages,
        format!("social_chat_{:?}", incoming.platform),
    )
    .with_trace_id(trace_id.to_string());

    let response = router.chat(&request).await?;
    Ok(response.content)
}

/// Handle specific intents with actions
async fn handle_intent_action(
    state: &AppState,
    _incoming: &SocialMessage,
    intent: &Intent,
) -> argus_core::types::ArgusResult<String> {
    match intent {
        Intent::ListProjects => {
            // Query projects from database
            let rows = sqlx::query_as::<_, (String, String)>(
                "SELECT id, name FROM projects ORDER BY created_at DESC LIMIT 10"
            )
            .fetch_all(&state.db_pool)
            .await?;

            if rows.is_empty() {
                return Ok("No projects found. Create one with 'create project <name>'".to_string());
            }

            let list: Vec<String> = rows.iter().map(|(id, name)| format!("• {} ({})", name, &id[..8])).collect();
            Ok(format!("Your projects:\n{}", list.join("\n")))
        }

        Intent::ProjectStatus { project_hint } => {
            if let Some(hint) = project_hint {
                let rows = sqlx::query_as::<_, (String, String, bool)>(
                    "SELECT id, name, active FROM projects WHERE name ILIKE $1 LIMIT 5"
                )
                .bind(format!("%{}%", hint))
                .fetch_all(&state.db_pool)
                .await?;

                if rows.is_empty() {
                    return Ok(format!("No project found matching '{}'", hint));
                }

                let status: Vec<String> = rows.iter()
                    .map(|(id, name, active)| format!("• {} ({}) - {}", name, &id[..8], if *active { "active" } else { "paused" }))
                    .collect();
                return Ok(format!("Project status:\n{}", status.join("\n")));
            }

            // Show overall status
            let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM projects")
                .fetch_one(&state.db_pool)
                .await?;

            let active: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM projects WHERE active = true")
                .fetch_one(&state.db_pool)
                .await?;

            Ok(format!("Total projects: {}, Active: {}", total.0, active.0))
        }

        Intent::CreateProject { description } => {
            let name = description.split_whitespace().next().unwrap_or("new-project");
            let project_id = uuid::Uuid::new_v4();
            let git_local_path = format!("/tmp/argus/projects/{name}");

            sqlx::query(
                "INSERT INTO projects (id, name, description, git_local_path, active, priority, created_at, updated_at) VALUES ($1, $2, $3, $4, true, 'medium', NOW(), NOW())"
            )
            .bind(project_id)
            .bind(name)
            .bind(description)
            .bind(git_local_path)
            .execute(&state.db_pool)
            .await?;

            Ok(format!("Created project '{}' (ID: {})", name, &project_id.to_string()[..8]))
        }

        Intent::Greeting => {
            Ok("Hello! I'm Argus, your autonomous AI developer. I can help you create projects, manage tasks, deploy code, and more. What would you like to do?".to_string())
        }

        Intent::Help => {
            Ok("I can help you with:\n• Creating projects: 'create project my-app'\n• Listing projects: 'list projects' or 'show projects'\n• Checking status: 'project status' or 'show status'\n• Creating tasks: 'create task for my-app'\n• And more via natural language!".to_string())
        }

        Intent::QueryMemory { .. } => {
            Ok("I remember our conversations. Use 'what did I ask about' to recall specific topics. My memory persists across sessions via PostgreSQL.".to_string())
        }

        // For other intents, return empty to fall back to LLM
        _ => Ok(String::new()),
    }
}

fn init_social_gateway(config: &ArgusConfig) -> Arc<SocialGateway> {
    let gateway = Arc::new(SocialGateway::new());

    let _ = gateway.register_adapter(Arc::new(WebDashboardAdapter::new()));

    if let Ok(token) = std::env::var("ARGUS_TELEGRAM_BOT_TOKEN") {
        if !token.trim().is_empty() {
            let _ = gateway.register_adapter(Arc::new(TelegramAdapter::new(token)));
        }
    }

    if let Ok(url) = std::env::var("ARGUS_FEISHU_WEBHOOK_URL") {
        if !url.trim().is_empty() {
            let _ = gateway.register_adapter(Arc::new(FeishuAdapter::new(url)));
        }
    }

    if let Ok(url) = std::env::var("ARGUS_GOOGLE_CHAT_WEBHOOK_URL") {
        if !url.trim().is_empty() {
            let _ = gateway.register_adapter(Arc::new(GoogleChatAdapter::new(url)));
        }
    }

    let wa_token = std::env::var("ARGUS_WHATSAPP_ACCESS_TOKEN").unwrap_or_default();
    let wa_phone_id = std::env::var("ARGUS_WHATSAPP_PHONE_NUMBER_ID").unwrap_or_default();
    if !wa_token.trim().is_empty() && !wa_phone_id.trim().is_empty() {
        let _ = gateway.register_adapter(Arc::new(WhatsAppAdapter::new(wa_token, wa_phone_id)));
    }

    let qq_enabled = config.social.qq.enabled
        || (!config.social.qq.app_id.trim().is_empty()
            && !config.social.qq.app_secret.trim().is_empty());
    if qq_enabled {
        let _ = gateway.register_adapter(Arc::new(QqAdapter::new(
            config.social.qq.app_id.clone(),
            config.social.qq.app_secret.clone(),
            config.social.qq.api_base_url.clone(),
            config.social.qq.token_url.clone(),
        )));
    }

    gateway
}

async fn init_llm_router(event_bus: &Arc<EventBus>) -> Result<Option<Arc<ProviderRouter>>> {
    let Some(mut providers_config) = load_providers_config()? else {
        return Ok(None);
    };

    apply_provider_env_overrides(&mut providers_config);

    let budget = Arc::new(BudgetTracker::new(
        Some(providers_config.global.max_monthly_budget_usd),
        Some(providers_config.global.max_daily_budget_usd),
        0.8,
        event_bus.clone(),
    ));

    let router = ProviderRouter::from_config(&providers_config, budget, event_bus.clone());
    if router.provider_count() == 0 {
        return Ok(None);
    }

    Ok(Some(Arc::new(router)))
}

fn load_providers_config() -> Result<Option<ProvidersConfig>> {
    let explicit = std::env::var("ARGUS_PROVIDERS_CONFIG_PATH").ok();
    let candidate_paths = if let Some(path) = explicit {
        vec![PathBuf::from(path)]
    } else {
        vec![PathBuf::from("config/providers.toml")]
    };

    for path in candidate_paths {
        if path.exists() {
            let raw = std::fs::read_to_string(&path)
                .with_context(|| format!("Failed to read providers config {}", path.display()))?;
            let config = toml::from_str::<ProvidersConfig>(&raw)
                .with_context(|| format!("Failed to parse providers config {}", path.display()))?;
            return Ok(Some(config));
        }
    }

    Ok(None)
}

fn apply_provider_env_overrides(config: &mut ProvidersConfig) {
    for provider in config.providers.values_mut() {
        provider.api_keys = provider
            .api_keys
            .iter()
            .map(|key| resolve_env_token(key))
            .filter(|key| !key.trim().is_empty())
            .collect();

        if let Some(base_url) = provider.base_url.as_ref() {
            provider.base_url = Some(resolve_env_token(base_url));
        }
    }
}

fn resolve_env_token(value: &str) -> String {
    let trimmed = value.trim();
    if let Some(name) = trimmed.strip_prefix("${").and_then(|s| s.strip_suffix('}')) {
        return std::env::var(name).unwrap_or_default();
    }

    if let Some(name) = trimmed.strip_prefix('$') {
        return std::env::var(name).unwrap_or_default();
    }

    trimmed.to_string()
}

fn derive_qq_signing_key(bot_secret: &str) -> Result<ed25519_dalek::SigningKey> {
    if bot_secret.is_empty() {
        return Err(anyhow!("empty bot secret"));
    }

    let mut seed = String::from(bot_secret);
    while seed.len() < ed25519_dalek::SECRET_KEY_LENGTH {
        seed.push_str(bot_secret);
    }

    let seed_bytes = &seed.as_bytes()[..ed25519_dalek::SECRET_KEY_LENGTH];
    let seed_array: [u8; ed25519_dalek::SECRET_KEY_LENGTH] = seed_bytes
        .try_into()
        .map_err(|_| anyhow!("failed to derive bot secret seed"))?;

    Ok(ed25519_dalek::SigningKey::from_bytes(&seed_array))
}

fn qq_validation_signature(bot_secret: &str, event_ts: &str, plain_token: &str) -> Result<String> {
    use ed25519_dalek::Signer;

    let signing_key = derive_qq_signing_key(bot_secret)?;
    let mut message = String::with_capacity(event_ts.len() + plain_token.len());
    message.push_str(event_ts);
    message.push_str(plain_token);

    let signature = signing_key.sign(message.as_bytes());
    Ok(hex::encode(signature.to_bytes()))
}

fn verify_qq_signature(headers: &HeaderMap, body: &[u8], bot_secret: &str) -> bool {
    use ed25519_dalek::Verifier;

    let signature_hex = match headers
        .get("X-Signature-Ed25519")
        .and_then(|v| v.to_str().ok())
    {
        Some(v) => v,
        None => {
            warn!("qq signature header missing: X-Signature-Ed25519");
            return false;
        }
    };

    let timestamp = match headers
        .get("X-Signature-Timestamp")
        .and_then(|v| v.to_str().ok())
    {
        Some(v) => v,
        None => {
            warn!("qq signature header missing: X-Signature-Timestamp");
            return false;
        }
    };

    let signature_bytes = match hex::decode(signature_hex) {
        Ok(v) => v,
        Err(err) => {
            warn!(error = %err, "qq signature decode failed");
            return false;
        }
    };

    let signature = match ed25519_dalek::Signature::from_slice(&signature_bytes) {
        Ok(v) => v,
        Err(err) => {
            warn!(error = %err, "qq signature parse failed");
            return false;
        }
    };

    let signing_key = match derive_qq_signing_key(bot_secret) {
        Ok(v) => v,
        Err(err) => {
            warn!(error = %err, "qq signing key derive failed");
            return false;
        }
    };

    let mut message = timestamp.as_bytes().to_vec();
    message.extend_from_slice(body);

    signing_key
        .verifying_key()
        .verify(&message, &signature)
        .is_ok()
}

struct CliSecurity {
    actor: String,
    role: Role,
    rbac: RbacPolicyEngine,
    audit: AuditLogger,
}

impl CliSecurity {
    fn from_config(config: &ArgusConfig) -> Self {
        let role = std::env::var("ARGUS_ACTOR_ROLE")
            .ok()
            .as_deref()
            .and_then(parse_role)
            .or_else(|| parse_role(&config.security.default_actor_role))
            .unwrap_or(Role::Owner);

        let actor = std::env::var("ARGUS_ACTOR").unwrap_or_else(|_| "argus-cli".to_string());

        Self {
            actor,
            role,
            rbac: RbacPolicyEngine::new(),
            audit: AuditLogger::new(),
        }
    }

    fn enforce_permissions(&self, command: &str, permissions: &[Permission]) -> Result<()> {
        for permission in permissions {
            if let Err(err) = self
                .rbac
                .require_roles(std::slice::from_ref(&self.role), *permission)
            {
                self.audit
                    .append(
                        self.actor.clone(),
                        format!("cli:{command}"),
                        AuditOutcome::Blocked,
                        Some(command.to_string()),
                        Some(serde_json::json!({
                            "role": format!("{:?}", self.role),
                            "permission": format!("{:?}", permission),
                            "reason": format!("{err}"),
                        })),
                    )
                    .map_err(|e| anyhow!("audit append failed: {e}"))?;

                return Err(anyhow!(err));
            }
        }
        Ok(())
    }

    fn audit_result(&self, command: &str, result: &Result<()>) -> Result<()> {
        let (outcome, error) = match result {
            Ok(_) => (AuditOutcome::Success, None::<String>),
            Err(e) => (AuditOutcome::Failure, Some(e.to_string())),
        };

        self.audit
            .append(
                self.actor.clone(),
                format!("cli:{command}"),
                outcome,
                Some(command.to_string()),
                Some(serde_json::json!({
                    "role": format!("{:?}", self.role),
                    "error": error,
                })),
            )
            .map_err(|e| anyhow!("audit append failed: {e}"))?;

        self.audit
            .verify_chain()
            .map_err(|e| anyhow!("audit verification failed: {e}"))
            .and_then(|ok| {
                if ok {
                    Ok(())
                } else {
                    Err(anyhow!("audit chain verification failed"))
                }
            })
    }
}

async fn run_secured_command<F>(
    security: &CliSecurity,
    command: &str,
    permissions: &[Permission],
    action: F,
) -> Result<()>
where
    F: std::future::Future<Output = Result<()>>,
{
    security.enforce_permissions(command, permissions)?;
    let result = action.await;
    security.audit_result(command, &result)?;
    result
}

fn parse_role(raw: &str) -> Option<Role> {
    match raw.to_ascii_lowercase().as_str() {
        "owner" => Some(Role::Owner),
        "admin" => Some(Role::Admin),
        "developer" => Some(Role::Developer),
        "viewer" => Some(Role::Viewer),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RuntimeEnvironment {
    Development,
    Production,
}

impl RuntimeEnvironment {
    fn detect() -> Self {
        let raw = std::env::var("ARGUS_ENV")
            .or_else(|_| std::env::var("RUST_ENV"))
            .unwrap_or_else(|_| "development".to_string())
            .to_ascii_lowercase();

        match raw.as_str() {
            "prod" | "production" => Self::Production,
            _ => Self::Development,
        }
    }
}

fn validate_startup_config(
    config: &ArgusConfig,
    command: &Commands,
    runtime_env: RuntimeEnvironment,
) -> Result<()> {
    if matches!(command, Commands::Version) {
        return Ok(());
    }

    if config.database.url.trim().is_empty() {
        bail!("database.url cannot be empty. Set ARGUS_DATABASE_URL or config.database.url");
    }

    let require_master_key = !matches!(command, Commands::Status | Commands::Project { .. })
        || matches!(runtime_env, RuntimeEnvironment::Production);
    if require_master_key {
        let key_name = config.security.master_key_env.trim();
        if key_name.is_empty() {
            bail!("security.master_key_env cannot be empty");
        }

        let key_val = std::env::var(key_name).unwrap_or_default();
        if key_val.trim().is_empty() {
            bail!("required master key is missing: {key_name}");
        }
        if key_val.contains("replace-with") {
            bail!("{key_name} still uses placeholder value; set a real 32-byte secret");
        }
        if matches!(runtime_env, RuntimeEnvironment::Production) && key_val.len() < 32 {
            bail!("{key_name} must be at least 32 bytes in production");
        }
    }

    if !matches!(command, Commands::Run | Commands::Mvp { .. }) {
        return Ok(());
    }

    let Some(mut providers_config) = load_providers_config()? else {
        if matches!(runtime_env, RuntimeEnvironment::Production) {
            bail!(
                "providers config is required in production for `run`/`mvp`; set ARGUS_PROVIDERS_CONFIG_PATH or config/providers.toml"
            );
        }
        return Ok(());
    };

    apply_provider_env_overrides(&mut providers_config);

    let providers_missing_credentials: Vec<String> = providers_config
        .providers
        .iter()
        .filter_map(|(id, provider)| {
            if !provider.enabled {
                return None;
            }

            let requires_api_key = !matches!(
                provider.provider_type.to_ascii_lowercase().as_str(),
                "ollama" | "web_scraper"
            );
            if requires_api_key && provider.api_keys.is_empty() {
                return Some(id.clone());
            }

            None
        })
        .collect();

    if providers_missing_credentials.is_empty() {
        return Ok(());
    }

    if matches!(runtime_env, RuntimeEnvironment::Production) {
        bail!(
            "enabled providers missing API keys: {}",
            providers_missing_credentials.join(", ")
        );
    }

    warn!(
        providers = %providers_missing_credentials.join(","),
        "some enabled providers have no API keys; they may be unusable"
    );
    Ok(())
}

// ============================================================
// Helpers
// ============================================================

/// Print a detailed startup recovery report.
///
/// This informs the operator what the daemon recovered after restart,
/// helping diagnose recovery issues and understand task state.
fn print_recovery_report(report: &argus_core::task_scheduler::RecoveryReport) {
    use std::io::Write;

    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout);
    let _ = writeln!(
        stdout,
        "┌─────────────────────────────────────────────────────┐"
    );
    let _ = writeln!(
        stdout,
        "│  ARGUS Recovery Report                              │"
    );
    let _ = writeln!(
        stdout,
        "├─────────────────────────────────────────────────────┤"
    );
    let _ = writeln!(
        stdout,
        "│ Active tasks loaded:     {:3} (non-terminal)          │",
        report.active_tasks
    );
    let _ = writeln!(
        stdout,
        "│ Interrupted resumed:     {:3} → Ready state           │",
        report.interrupted_resumed
    );
    let _ = writeln!(
        stdout,
        "│ Ready for execution:     {:3}                         │",
        report.ready_for_execution
    );
    let _ = writeln!(
        stdout,
        "│ Other states:            {:3} (Pending/Paused/etc)    │",
        report.other_states
    );
    if let Some(skipped) = report.terminal_tasks_skipped {
        let _ = writeln!(
            stdout,
            "│ Terminal tasks skipped:  {:3} (historical excluded)   │",
            skipped
        );
    }
    let _ = writeln!(
        stdout,
        "└─────────────────────────────────────────────────────┘"
    );
    let _ = writeln!(stdout);
}

/// Create a PostgreSQL connection pool from config.
async fn create_db_pool(config: &ArgusConfig) -> Result<sqlx::PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(config.database.max_connections)
        .min_connections(config.database.min_connections)
        .acquire_timeout(Duration::from_secs(config.database.connect_timeout_seconds))
        .idle_timeout(Duration::from_secs(config.database.idle_timeout_seconds))
        .connect(&config.database.url)
        .await
        .context("Failed to connect to PostgreSQL")?;

    Ok(pool)
}

/// Initialize tracing based on config.
fn init_tracing(config: &ArgusConfig) {
    use tracing_subscriber::{EnvFilter, fmt};

    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.logging.level));

    match config.logging.format.as_str() {
        "json" => {
            let subscriber = fmt::Subscriber::builder()
                .with_env_filter(env_filter)
                .json()
                .with_target(true)
                .with_thread_ids(false)
                .with_file(false)
                .with_line_number(false)
                .finish();
            tracing::subscriber::set_global_default(subscriber)
                .expect("Failed to set tracing subscriber");
        }
        _ => {
            // "pretty" or any other value
            let subscriber = fmt::Subscriber::builder()
                .with_env_filter(env_filter)
                .with_target(false)
                .with_thread_ids(false)
                .finish();
            tracing::subscriber::set_global_default(subscriber)
                .expect("Failed to set tracing subscriber");
        }
    }
}

/// Wait for Ctrl+C (SIGINT) or SIGTERM.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {
            info!("Received SIGINT (Ctrl+C)");
        }
        () = terminate => {
            info!("Received SIGTERM");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use argus_core::self_evolution::{
        EvolutionCategory, EvolutionDeployer, EvolutionEngine, EvolutionGit, EvolutionRequest,
        EvolutionSecurityHooks, EvolutionStatus, EvolutionTester, FileChange, TestSummary,
    };
    use argus_mcp::registry::ServerRegistry;
    use argus_mcp::router::ToolRouter;
    use argus_skills::manifest::{SkillManifest, SkillRuntime};
    use argus_skills::runtime::SkillRuntimeEngine;
    use async_trait::async_trait;
    use axum::Json;
    use axum::extract::{Extension, State};
    use sqlx::postgres::PgConnectOptions;

    use super::*;

    static ENV_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn unique_env_var(prefix: &str) -> String {
        let nonce = ENV_COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("{}_{}", prefix, nonce)
    }

    fn integration_database_url() -> String {
        std::env::var("ARGUS_DATABASE_URL")
            .or_else(|_| std::env::var("DATABASE_URL"))
            .expect("set ARGUS_DATABASE_URL or DATABASE_URL for integration tests")
    }

    #[test]
    fn test_parse_role() {
        assert!(matches!(parse_role("owner"), Some(Role::Owner)));
        assert!(matches!(parse_role("ADMIN"), Some(Role::Admin)));
        assert!(matches!(parse_role("developer"), Some(Role::Developer)));
        assert!(matches!(parse_role("viewer"), Some(Role::Viewer)));
        assert!(parse_role("unknown").is_none());
    }

    #[test]
    fn test_validate_startup_config_requires_master_key_for_run() {
        let mut config = ArgusConfig::default();
        config.security.master_key_env = unique_env_var("ARGUS_TEST_MISSING_MASTER_KEY");

        let err = validate_startup_config(&config, &Commands::Run, RuntimeEnvironment::Development)
            .unwrap_err();
        assert!(err.to_string().contains("required master key is missing"));
    }

    #[test]
    fn test_validate_startup_config_rejects_placeholder_master_key() {
        let mut config = ArgusConfig::default();
        let env_name = unique_env_var("ARGUS_TEST_PLACEHOLDER_MASTER_KEY");
        config.security.master_key_env = env_name.clone();

        unsafe {
            std::env::set_var(&env_name, "replace-with-32-byte-secret");
        }

        let err = validate_startup_config(&config, &Commands::Run, RuntimeEnvironment::Development)
            .unwrap_err();
        assert!(err.to_string().contains("placeholder value"));

        unsafe {
            std::env::remove_var(&env_name);
        }
    }

    #[test]
    fn test_validate_startup_config_requires_provider_keys_in_production() {
        let mut config = ArgusConfig::default();

        let master_key_env = unique_env_var("ARGUS_TEST_MASTER_KEY");
        config.security.master_key_env = master_key_env.clone();
        unsafe {
            std::env::set_var(&master_key_env, "0123456789abcdef0123456789abcdef");
        }

        let missing_key_env = unique_env_var("ARGUS_TEST_PROVIDER_KEY");
        let providers_path = std::env::temp_dir().join(format!(
            "argus-providers-test-{}.toml",
            ENV_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(
            &providers_path,
            format!(
                "[global]\ndefault_model_class = \"coding\"\n\n[providers.test]\nenabled = true\ntype = \"openai_compatible\"\napi_keys = [\"${{{missing_key_env}}}\"]\n"
            ),
        )
        .unwrap();

        unsafe {
            std::env::set_var("ARGUS_PROVIDERS_CONFIG_PATH", &providers_path);
            std::env::remove_var(&missing_key_env);
        }

        let err = validate_startup_config(&config, &Commands::Run, RuntimeEnvironment::Production)
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("enabled providers missing API keys")
        );

        unsafe {
            std::env::remove_var("ARGUS_PROVIDERS_CONFIG_PATH");
            std::env::remove_var(&master_key_env);
        }
        let _ = std::fs::remove_file(providers_path);
    }

    #[tokio::test]
    async fn test_secured_command_denied_for_viewer() {
        let security = CliSecurity {
            actor: "test-cli".to_string(),
            role: Role::Viewer,
            rbac: RbacPolicyEngine::new(),
            audit: AuditLogger::new(),
        };

        let res = run_secured_command(&security, "mvp", &[Permission::ExecuteCommand], async {
            Ok(())
        })
        .await;

        assert!(res.is_err());
        assert!(security.audit.verify_chain().unwrap());
    }

    #[tokio::test]
    async fn test_e2e_cli_to_mcp_shell_success_for_developer() {
        let security = CliSecurity {
            actor: "test-cli".to_string(),
            role: Role::Developer,
            rbac: RbacPolicyEngine::new(),
            audit: AuditLogger::new(),
        };

        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("argus-cli-e2e-{nonce}"));
        std::fs::create_dir_all(&root).unwrap();
        let router = ToolRouter::new(ServerRegistry::new(), &root);

        let result = run_secured_command(
            &security,
            "shell.run",
            &[Permission::ExecuteCommand],
            async {
                let out = router
                    .call_builtin_with_roles(
                        "shell.run",
                        serde_json::json!({"command": "echo cli-mcp-e2e"}),
                        &[Role::Developer],
                    )
                    .await
                    .map_err(anyhow::Error::from)?;
                assert!(out.success);
                assert!(
                    out.content
                        .as_str()
                        .unwrap_or_default()
                        .contains("cli-mcp-e2e")
                );
                Ok(())
            },
        )
        .await;

        assert!(result.is_ok());
        assert!(security.audit.verify_chain().unwrap());
        assert!(router.audit.verify_chain().unwrap());
    }

    #[tokio::test]
    async fn test_e2e_cli_to_skills_untrusted_blocked() {
        let security = CliSecurity {
            actor: "test-cli".to_string(),
            role: Role::Developer,
            rbac: RbacPolicyEngine::new(),
            audit: AuditLogger::new(),
        };

        let runtime = SkillRuntimeEngine::new();
        let skill = SkillManifest {
            id: "e2e-skill".to_string(),
            name: "e2e-skill".to_string(),
            description: "test".to_string(),
            runtime: SkillRuntime::Shell,
            command: "echo hi".to_string(),
            permissions: vec![],
            trust_level: argus_core::types::TrustLevel::Untrusted,
        };

        let result = run_secured_command(
            &security,
            "skill.execute",
            &[Permission::ExecuteCommand],
            async {
                runtime
                    .execute(&skill, &[])
                    .await
                    .map_err(anyhow::Error::from)?;
                Ok(())
            },
        )
        .await;

        assert!(result.is_err());
        assert!(runtime.audit_chain_ok().unwrap());
        assert!(security.audit.verify_chain().unwrap());
    }

    struct MockGit;

    #[async_trait]
    impl EvolutionGit for MockGit {
        async fn create_branch(
            &self,
            _source_dir: &PathBuf,
            _branch: &str,
        ) -> argus_core::types::ArgusResult<()> {
            Ok(())
        }

        async fn get_changes(
            &self,
            _source_dir: &PathBuf,
            _branch: &str,
        ) -> argus_core::types::ArgusResult<Vec<FileChange>> {
            Ok(vec![FileChange {
                path: "crates/argus-core/src/self_evolution.rs".to_string(),
                change_type: argus_core::self_evolution::ChangeType::Modified,
            }])
        }

        async fn merge_branch(
            &self,
            _source_dir: &PathBuf,
            _branch: &str,
            _target: &str,
        ) -> argus_core::types::ArgusResult<()> {
            Ok(())
        }
    }

    struct MockTester;

    #[async_trait]
    impl EvolutionTester for MockTester {
        async fn run_full_suite(
            &self,
            _source_dir: &PathBuf,
            _branch: &str,
        ) -> argus_core::types::ArgusResult<TestSummary> {
            Ok(TestSummary {
                all_passed: true,
                failure_summary: String::new(),
            })
        }
    }

    struct MockDeployer;

    #[async_trait]
    impl EvolutionDeployer for MockDeployer {
        async fn deploy(
            &self,
            _source_dir: &PathBuf,
            _branch: &str,
        ) -> argus_core::types::ArgusResult<()> {
            Ok(())
        }
    }

    struct DenyAuthorizeHook;

    impl EvolutionSecurityHooks for DenyAuthorizeHook {
        fn authorize_request(
            &self,
            _request: &EvolutionRequest,
        ) -> argus_core::types::ArgusResult<()> {
            Err(argus_core::types::ArgusError::PermissionDenied(
                "cli-e2e authorization denied".to_string(),
            ))
        }

        fn audit_transition(
            &self,
            _request: &EvolutionRequest,
            _status: &EvolutionStatus,
        ) -> argus_core::types::ArgusResult<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_e2e_cli_to_self_evolution_hook_blocked() {
        let security = CliSecurity {
            actor: "test-cli".to_string(),
            role: Role::Admin,
            rbac: RbacPolicyEngine::new(),
            audit: AuditLogger::new(),
        };

        let result = run_secured_command(
            &security,
            "self-evolution",
            &[Permission::ConfigureSystem],
            async {
                let engine =
                    EvolutionEngine::new("/tmp/argus-e2e", MockGit, MockTester, MockDeployer)
                        .with_security_hooks(Arc::new(DenyAuthorizeHook));
                let mut req =
                    EvolutionRequest::new("E2E self evolution", EvolutionCategory::Infrastructure);

                let err = engine.evolve(&mut req).await.unwrap_err();
                assert!(matches!(req.status, EvolutionStatus::SafetyRejected(_)));
                Err(anyhow!(err))
            },
        )
        .await;

        assert!(result.is_err());
        assert!(security.audit.verify_chain().unwrap());
    }

    #[tokio::test]
    async fn test_api_chat_greeting_without_llm() {
        let db_pool =
            sqlx::postgres::PgPoolOptions::new().connect_lazy_with(PgConnectOptions::new());
        let config = ArgusConfig::default();
        let state = Arc::new(AppState {
            gateway: init_social_gateway(&config),
            conversation: ConversationManager::new(),
            llm: None,
            default_model_class: ModelClass::Coding,
            qq_bot_secret: None,
            db_pool,
            brain: Arc::new(BrainEngine::default()),
        });

        let request = ApiChatRequest {
            session_id: "s1".to_string(),
            user_id: "u1".to_string(),
            text: "hello".to_string(),
        };

        let Json(response) = api_chat(
            State(state),
            Extension(TraceId("trace-test".to_string())),
            Json(request),
        )
        .await
        .unwrap();

        assert!(response.reply.to_ascii_lowercase().contains("hello"));
    }

    #[tokio::test]
    #[ignore = "requires postgres and migrations"]
    async fn test_cmd_migrate_integration() {
        let mut config = ArgusConfig::default();
        config.database.url = integration_database_url();
        cmd_migrate(config).await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires postgres and migrations"]
    async fn test_cmd_status_integration() {
        let mut config = ArgusConfig::default();
        config.database.url = integration_database_url();
        cmd_status(config).await.unwrap();
    }

    // ============================================================
    // V0.44 security closure — provider credential handling
    // ============================================================

    /// Detect a literal API key of the OpenAI/DashScope shape.
    ///
    /// Deliberately narrow: `sk-` alone is useless because it also occurs
    /// inside words like `task_...`. A hit additionally requires a long
    /// alphanumeric/dash run containing a digit, which ordinary config keys,
    /// identifiers and durations do not produce.
    fn contains_literal_key_shape(text: &str) -> bool {
        let mut from = 0;
        while let Some(pos) = text[from..].find("sk-") {
            let start = from + pos + 3;
            let run: String = text[start..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect();
            let body = run.trim_start_matches('-');
            if body.len() >= 16 && body.chars().any(|c| c.is_ascii_digit()) {
                return true;
            }
            from = start;
        }
        false
    }

    fn contains_bearer_token(text: &str) -> bool {
        text.to_ascii_lowercase()
            .split("bearer ")
            .skip(1)
            .any(|rest| {
                rest.chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'))
                    .count()
                    >= 20
            })
    }

    /// Every tracked config file must carry placeholders, never credentials.
    #[test]
    fn tracked_configs_hold_no_literal_credentials() {
        let config_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config");
        let mut seen = Vec::new();
        for entry in std::fs::read_dir(&config_dir)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", config_dir.display()))
            .flatten()
        {
            let path = entry.path();
            // Runtime configuration is private; only shipped templates are tracked.
            let filename = path.file_name().and_then(|name| name.to_str());
            if !matches!(
                filename,
                Some("argus.default.toml" | "providers.example.toml")
            ) {
                continue;
            }
            let raw = std::fs::read_to_string(&path).unwrap();
            assert!(
                !contains_literal_key_shape(&raw),
                "{} contains what looks like a literal API key; use \"${{ENV_VAR}}\" \
                 indirection — the value belongs in the environment, never in git",
                path.display()
            );
            assert!(
                !contains_bearer_token(&raw),
                "{} contains what looks like a literal bearer token",
                path.display()
            );

            // The precise check: whatever `api_keys` declares must be a locator.
            if let Ok(cfg) = toml::from_str::<ProvidersConfig>(&raw) {
                for (id, provider) in &cfg.providers {
                    for key in &provider.api_keys {
                        assert!(
                            key.is_empty() || key.starts_with('$'),
                            "provider '{id}' in {} declares a literal credential; \
                             it must be an env placeholder like \"${{MY_API_KEY}}\"",
                            path.display()
                        );
                    }
                }
            }
            seen.push(path.file_name().unwrap().to_string_lossy().to_string());
        }

        for required in ["argus.default.toml", "providers.example.toml"] {
            assert!(
                seen.iter().any(|f| f == required),
                "the guard never scanned {required}; scan is not covering the real config"
            );
        }
    }

    /// `config placeholder -> env override -> provider receives the credential`,
    /// and the resolved value never appears on an Argus loggable surface.
    #[test]
    fn provider_env_indirection_resolves_without_leaking() {
        let key_env = unique_env_var("ARGUS_TEST_PROVIDER_KEY");
        let fake_value = "test-secret-do-not-use-8f2c1a4b";
        let providers_path = std::env::temp_dir().join(format!(
            "argus-providers-env-{}.toml",
            ENV_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(
            &providers_path,
            format!(
                "[global]\ndefault_model_class = \"coding\"\n\n[providers.dash]\
                 \nenabled = true\ntype = \"openai_compatible\"\npriority = 1\
                 \nbase_url = \"https://example.invalid/v1\"\
                 \napi_keys = [\"${{{key_env}}}\"]\
                 \n\n[[providers.dash.models]]\nid = \"m1\"\nclass = \"coding\"\n"
            ),
        )
        .unwrap();

        unsafe {
            std::env::set_var(&key_env, fake_value);
            std::env::set_var("ARGUS_PROVIDERS_CONFIG_PATH", &providers_path);
        }

        let loaded = (|| -> Result<()> {
            let raw = std::fs::read_to_string(&providers_path).unwrap();
            assert!(raw.contains(&format!("${{{key_env}}}")));
            assert!(
                !raw.contains(fake_value),
                "the on-disk config must never contain the resolved value"
            );

            let mut cfg =
                load_providers_config()?.expect("config loads from ARGUS_PROVIDERS_CONFIG_PATH");
            // Before resolution the value is still just a placeholder.
            assert!(!format!("{cfg:?}").contains(fake_value));

            apply_provider_env_overrides(&mut cfg);
            assert_eq!(
                cfg.providers["dash"].api_keys,
                vec![fake_value.to_string()],
                "config placeholder must resolve from the environment"
            );
            Ok(())
        })();

        unsafe {
            std::env::remove_var("ARGUS_PROVIDERS_CONFIG_PATH");
            std::env::remove_var(&key_env);
        }
        let _ = std::fs::remove_file(providers_path);
        loaded.unwrap();

        // The V0.44 credential type stores a locator, so a real key cannot be
        // carried into Debug output, serialized API responses, or DB metadata.
        // (AuthProfile serialization against Postgres is covered by the
        // `secrets_are_redacted` end-to-end test.)
        let secret = argus_core::provider::SecretReference::env(&key_env);
        let rendered = format!("{:?} {} {}", secret, secret.redacted(), secret.locator);
        assert!(!rendered.contains(fake_value));
        assert!(!serde_json::to_string(&secret).unwrap().contains(fake_value));
    }

    /// The public provider template resolves environment references without a
    /// database, daemon, or provider call, or writing credentials back to disk.
    #[test]
    fn example_provider_config_resolves_from_environment() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../config/providers.example.toml");
        let raw = std::fs::read_to_string(&path).unwrap();
        let mut cfg =
            toml::from_str::<ProvidersConfig>(&raw).expect("providers.example.toml parses");

        let placeholders: Vec<(String, String)> = cfg
            .providers
            .iter()
            .flat_map(|(id, p)| {
                p.api_keys
                    .iter()
                    .filter(|k| k.starts_with("${"))
                    .map(move |k| (id.clone(), k.clone()))
            })
            .collect();
        assert!(
            !placeholders.is_empty(),
            "providers.example.toml declares no env-indirect credential; was a literal reintroduced?"
        );

        let fake = "test-secret-do-not-use-7d4e9b2a";
        let mut names = Vec::new();
        for (_, token) in &placeholders {
            let name = token.trim_start_matches("${").trim_end_matches('}');
            names.push(name.to_string());
            unsafe { std::env::set_var(name, fake) };
        }

        apply_provider_env_overrides(&mut cfg);
        for (id, _) in &placeholders {
            assert_eq!(
                cfg.providers[id].api_keys,
                vec![fake.to_string()],
                "provider '{id}' did not resolve its credential from the environment"
            );
        }

        // Resolution is in-memory only; the tracked file keeps holding a placeholder.
        let raw_after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(raw, raw_after, "the config file must not be rewritten");
        assert!(
            !raw_after.contains(fake),
            "a resolved credential must never be written back to tracked config"
        );

        for name in names {
            unsafe { std::env::remove_var(&name) };
        }
    }
}

#[cfg(test)]
mod project_permission_tests {
    use super::*;

    #[test]
    fn project_writes_and_acceptance_do_not_inherit_read_permissions() {
        let rbac = RbacPolicyEngine::new();
        let command = ProjectCommand::Evaluate {
            project: "test".into(),
            work_item: Uuid::nil(),
        };
        assert!(
            project_permissions(&command)
                .iter()
                .any(|p| rbac.require_roles(&[Role::Viewer], *p).is_err())
        );
        let command = ProjectCommand::Accept {
            project: "test".into(),
            work_item: Uuid::nil(),
            evaluation: Uuid::nil(),
        };
        assert!(
            project_permissions(&command)
                .iter()
                .any(|p| rbac.require_roles(&[Role::Developer], *p).is_err())
        );
        assert!(
            project_permissions(&command)
                .iter()
                .all(|p| rbac.require_roles(&[Role::Admin], *p).is_ok())
        );
    }
}
