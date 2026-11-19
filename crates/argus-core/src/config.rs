//! Configuration management for Argus.
//!
//! All configuration is loaded from TOML files with environment variable overrides.
//! The config file path defaults to `config/argus.toml`, but can be set via
//! `ARGUS_CONFIG_PATH` or `--config` CLI flag.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::types::{ArgusError, ArgusResult};

// ============================================================
// Top-level config
// ============================================================

/// Top-level Argus configuration.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ArgusConfig {
    #[serde(default)]
    pub execution: ExecutionConfig,

    /// Database settings.
    #[serde(default)]
    pub database: DatabaseConfig,

    /// HTTP server / web dashboard settings.
    #[serde(default)]
    pub server: ServerConfig,

    /// Worker pool settings.
    #[serde(default)]
    pub worker: WorkerConfig,

    /// LLM provider settings.
    #[serde(default)]
    pub llm: LlmConfig,

    /// Memory system settings.
    #[serde(default)]
    pub memory: MemoryConfig,

    /// Notification / disturbance settings.
    #[serde(default)]
    pub notification: NotificationConfig,

    /// Social platform integrations.
    #[serde(default)]
    pub social: SocialConfig,

    /// Security settings.
    #[serde(default)]
    pub security: SecurityConfig,

    /// Scheduler settings.
    #[serde(default)]
    pub scheduler: SchedulerConfig,

    /// Long-running project auto-iteration settings.
    #[serde(default)]
    pub autoloop: AutoLoopConfig,

    /// Logging / observability settings.
    #[serde(default)]
    pub logging: LoggingConfig,

    /// System settings.
    #[serde(default)]
    pub system: SystemConfig,

    /// Budget enforcement settings.
    #[serde(default)]
    pub budget: BudgetConfig,
}

// ============================================================
// Sub-configs
// ============================================================

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ExecutionConfig {
    #[serde(default)]
    pub default_worker: ExecutionWorker,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionWorker {
    #[default]
    Codex,
    ClaudeCode,
}

impl ExecutionWorker {
    pub fn worker_type(self) -> crate::worker_types::WorkerType {
        match self {
            Self::Codex => crate::worker_types::WorkerType::GenericCli,
            Self::ClaudeCode => crate::worker_types::WorkerType::ClaudeCode,
        }
    }
}

/// Budget enforcement settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BudgetConfig {
    /// Daily budget in USD for Agent executions (None = unlimited).
    #[serde(default = "BudgetConfig::default_daily_budget")]
    pub daily_budget_usd: Option<f64>,

    /// Per-task maximum spend in USD (None = unlimited).
    #[serde(default = "BudgetConfig::default_per_task_budget")]
    pub max_per_task_usd: Option<f64>,
}

impl BudgetConfig {
    fn default_daily_budget() -> Option<f64> {
        Some(50.0) // Default: $50/day
    }
    fn default_per_task_budget() -> Option<f64> {
        Some(10.0) // Default: $10/task
    }
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            daily_budget_usd: Self::default_daily_budget(),
            max_per_task_usd: Self::default_per_task_budget(),
        }
    }
}

/// Database connection settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DatabaseConfig {
    /// PostgreSQL connection URL.
    /// Can be overridden by `ARGUS_DATABASE_URL` env var.
    #[serde(default = "DatabaseConfig::default_url")]
    pub url: String,

    /// Maximum number of connections in the pool.
    #[serde(default = "DatabaseConfig::default_max_connections")]
    pub max_connections: u32,

    /// Minimum number of idle connections.
    #[serde(default = "DatabaseConfig::default_min_connections")]
    pub min_connections: u32,

    /// Connection timeout in seconds.
    #[serde(default = "DatabaseConfig::default_connect_timeout")]
    pub connect_timeout_seconds: u64,

    /// Idle connection timeout in seconds.
    #[serde(default = "DatabaseConfig::default_idle_timeout")]
    pub idle_timeout_seconds: u64,

    /// Run migrations on startup.
    #[serde(default = "DatabaseConfig::default_run_migrations")]
    pub run_migrations: bool,
}

impl DatabaseConfig {
    fn default_url() -> String {
        "postgres://argus:argus@localhost:5432/argus".to_string()
    }
    fn default_max_connections() -> u32 {
        20
    }
    fn default_min_connections() -> u32 {
        2
    }
    fn default_connect_timeout() -> u64 {
        10
    }
    fn default_idle_timeout() -> u64 {
        600
    }
    fn default_run_migrations() -> bool {
        true
    }
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: Self::default_url(),
            max_connections: Self::default_max_connections(),
            min_connections: Self::default_min_connections(),
            connect_timeout_seconds: Self::default_connect_timeout(),
            idle_timeout_seconds: Self::default_idle_timeout(),
            run_migrations: Self::default_run_migrations(),
        }
    }
}

/// HTTP server / web dashboard settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Host to bind the web dashboard to.
    #[serde(default = "ServerConfig::default_host")]
    pub host: String,

    /// Port for the web dashboard.
    #[serde(default = "ServerConfig::default_port")]
    pub port: u16,

    /// Enable the web dashboard.
    #[serde(default = "ServerConfig::default_enabled")]
    pub enabled: bool,
}

impl ServerConfig {
    fn default_host() -> String {
        "127.0.0.1".to_string()
    }
    fn default_port() -> u16 {
        3100
    }
    fn default_enabled() -> bool {
        true
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: Self::default_host(),
            port: Self::default_port(),
            enabled: Self::default_enabled(),
        }
    }
}

/// Worker pool settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkerConfig {
    /// Maximum number of concurrent workers.
    #[serde(default = "WorkerConfig::default_max_pool_size")]
    pub max_pool_size: u32,

    /// Default sandbox mode for new workers.
    #[serde(default = "WorkerConfig::default_sandbox")]
    pub default_sandbox: String,

    /// How long a worker can be idle before being shut down (seconds).
    #[serde(default = "WorkerConfig::default_idle_timeout")]
    pub idle_timeout_seconds: u64,

    /// Default task timeout for workers (seconds).
    #[serde(default = "WorkerConfig::default_task_timeout")]
    pub task_timeout_seconds: u64,

    /// Preferred worker type for new tasks.
    #[serde(default = "WorkerConfig::default_preferred_type")]
    pub preferred_type: String,
}

impl WorkerConfig {
    fn default_max_pool_size() -> u32 {
        8
    }
    fn default_sandbox() -> String {
        "directory".to_string()
    }
    fn default_idle_timeout() -> u64 {
        300
    }
    fn default_task_timeout() -> u64 {
        3600
    }
    fn default_preferred_type() -> String {
        "opencode".to_string()
    }
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            max_pool_size: Self::default_max_pool_size(),
            default_sandbox: Self::default_sandbox(),
            idle_timeout_seconds: Self::default_idle_timeout(),
            task_timeout_seconds: Self::default_task_timeout(),
            preferred_type: Self::default_preferred_type(),
        }
    }
}

/// LLM provider settings (high-level; per-provider config is in DB).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LlmConfig {
    /// Default provider name.
    #[serde(default = "LlmConfig::default_provider")]
    pub default_provider: String,

    /// Global monthly budget in USD (0 = unlimited).
    #[serde(default)]
    pub monthly_budget_usd: f64,

    /// Budget warning threshold (0.0 - 1.0, fires warning at this %).
    #[serde(default = "LlmConfig::default_budget_warning_threshold")]
    pub budget_warning_threshold: f64,

    /// Failover chain — ordered list of provider names.
    #[serde(default)]
    pub failover_chain: Vec<String>,

    /// Enable token usage caching.
    #[serde(default = "LlmConfig::default_enable_cache")]
    pub enable_cache: bool,
}

impl LlmConfig {
    fn default_provider() -> String {
        "anthropic".to_string()
    }
    fn default_budget_warning_threshold() -> f64 {
        0.8
    }
    fn default_enable_cache() -> bool {
        true
    }
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            default_provider: Self::default_provider(),
            monthly_budget_usd: 0.0,
            budget_warning_threshold: Self::default_budget_warning_threshold(),
            failover_chain: Vec::new(),
            enable_cache: Self::default_enable_cache(),
        }
    }
}

/// Memory system settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// Short-term memory TTL in hours.
    #[serde(default = "MemoryConfig::default_short_term_ttl_hours")]
    pub short_term_ttl_hours: u64,

    /// Default decay rate per day (0.0 - 1.0).
    #[serde(default = "MemoryConfig::default_decay_rate")]
    pub decay_rate: f64,

    /// How often to run consolidation (seconds).
    #[serde(default = "MemoryConfig::default_consolidation_interval")]
    pub consolidation_interval_seconds: u64,

    /// Importance threshold for promotion (short-term → long-term).
    #[serde(default = "MemoryConfig::default_promotion_threshold")]
    pub promotion_threshold: f64,

    /// Maximum tokens for working memory context window.
    #[serde(default = "MemoryConfig::default_max_working_memory_tokens")]
    pub max_working_memory_tokens: u32,

    /// Number of results to return for recall queries.
    #[serde(default = "MemoryConfig::default_recall_top_k")]
    pub recall_top_k: u32,

    /// Minimum similarity score for recall results (0.0 - 1.0).
    #[serde(default = "MemoryConfig::default_similarity_threshold")]
    pub similarity_threshold: f64,
}

impl MemoryConfig {
    fn default_short_term_ttl_hours() -> u64 {
        72
    }
    fn default_decay_rate() -> f64 {
        0.01
    }
    fn default_consolidation_interval() -> u64 {
        3600
    }
    fn default_promotion_threshold() -> f64 {
        0.7
    }
    fn default_max_working_memory_tokens() -> u32 {
        8000
    }
    fn default_recall_top_k() -> u32 {
        10
    }
    fn default_similarity_threshold() -> f64 {
        0.6
    }
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            short_term_ttl_hours: Self::default_short_term_ttl_hours(),
            decay_rate: Self::default_decay_rate(),
            consolidation_interval_seconds: Self::default_consolidation_interval(),
            promotion_threshold: Self::default_promotion_threshold(),
            max_working_memory_tokens: Self::default_max_working_memory_tokens(),
            recall_top_k: Self::default_recall_top_k(),
            similarity_threshold: Self::default_similarity_threshold(),
        }
    }
}

/// Notification / disturbance settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NotificationConfig {
    /// Disturbance level: minimal, normal, verbose.
    #[serde(default = "NotificationConfig::default_disturbance_level")]
    pub disturbance_level: String,

    /// Active hours start (HH:MM).
    #[serde(default = "NotificationConfig::default_active_hours_start")]
    pub active_hours_start: String,

    /// Active hours end (HH:MM).
    #[serde(default = "NotificationConfig::default_active_hours_end")]
    pub active_hours_end: String,

    /// Preferred platform priority order.
    #[serde(default)]
    pub platform_priority: Vec<String>,

    /// Digest schedule (cron expression or "hourly", "daily").
    #[serde(default = "NotificationConfig::default_digest_schedule")]
    pub digest_schedule: String,
}

impl NotificationConfig {
    fn default_disturbance_level() -> String {
        "normal".to_string()
    }
    fn default_active_hours_start() -> String {
        "08:00".to_string()
    }
    fn default_active_hours_end() -> String {
        "23:00".to_string()
    }
    fn default_digest_schedule() -> String {
        "hourly".to_string()
    }
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            disturbance_level: Self::default_disturbance_level(),
            active_hours_start: Self::default_active_hours_start(),
            active_hours_end: Self::default_active_hours_end(),
            platform_priority: Vec::new(),
            digest_schedule: Self::default_digest_schedule(),
        }
    }
}

/// Social platform integration settings.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SocialConfig {
    /// Shared secret for authenticating Telegram webhook requests.
    #[serde(default)]
    pub telegram_webhook_secret: String,

    /// QQ Bot integration settings.
    #[serde(default)]
    pub qq: QqIntegrationConfig,
}

/// QQ Bot integration settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QqIntegrationConfig {
    /// Enable QQ adapter registration.
    #[serde(default)]
    pub enabled: bool,

    /// QQ AppID.
    #[serde(default)]
    pub app_id: String,

    /// QQ AppSecret.
    #[serde(default)]
    pub app_secret: String,

    /// QQ bot secret used for webhook signature verification.
    #[serde(default)]
    pub bot_secret: String,

    /// QQ OpenAPI base URL.
    #[serde(default = "QqIntegrationConfig::default_api_base_url")]
    pub api_base_url: String,

    /// QQ access-token URL.
    #[serde(default = "QqIntegrationConfig::default_token_url")]
    pub token_url: String,
}

impl QqIntegrationConfig {
    fn default_api_base_url() -> String {
        "https://api.sgroup.qq.com".to_string()
    }

    fn default_token_url() -> String {
        "https://bots.qq.com/app/getAppAccessToken".to_string()
    }
}

impl Default for QqIntegrationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            app_id: String::new(),
            app_secret: String::new(),
            bot_secret: String::new(),
            api_base_url: Self::default_api_base_url(),
            token_url: Self::default_token_url(),
        }
    }
}

/// Security settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// Name of the env var that holds the master encryption key.
    #[serde(default = "SecurityConfig::default_master_key_env")]
    pub master_key_env: String,

    /// Default sandbox mode: directory, docker, none.
    #[serde(default = "SecurityConfig::default_sandbox_mode")]
    pub sandbox_mode: String,

    /// Blocked shell commands.
    #[serde(default)]
    pub blocked_commands: Vec<String>,

    /// Additional protected paths for guardrail engine.
    #[serde(default)]
    pub protected_paths: Vec<String>,

    /// Additional forbidden content markers for guardrail engine.
    #[serde(default)]
    pub forbidden_content_markers: Vec<String>,

    /// Additional keywords for secret scanner generic detection.
    #[serde(default)]
    pub secret_keywords: Vec<String>,

    /// Default actor role for CLI commands when env is not set.
    #[serde(default = "SecurityConfig::default_actor_role")]
    pub default_actor_role: String,

    /// Audit log level: minimal, standard, verbose.
    #[serde(default = "SecurityConfig::default_audit_level")]
    pub audit_level: String,
}

impl SecurityConfig {
    fn default_master_key_env() -> String {
        "ARGUS_MASTER_KEY".to_string()
    }
    fn default_sandbox_mode() -> String {
        "directory".to_string()
    }
    fn default_audit_level() -> String {
        "standard".to_string()
    }
    fn default_actor_role() -> String {
        "developer".to_string()
    }
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            master_key_env: Self::default_master_key_env(),
            sandbox_mode: Self::default_sandbox_mode(),
            blocked_commands: Vec::new(),
            protected_paths: Vec::new(),
            forbidden_content_markers: Vec::new(),
            secret_keywords: Vec::new(),
            default_actor_role: Self::default_actor_role(),
            audit_level: Self::default_audit_level(),
        }
    }
}

/// Scheduler settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SchedulerConfig {
    /// Maximum concurrent tasks across all workers.
    #[serde(default = "SchedulerConfig::default_max_concurrent_tasks")]
    pub max_concurrent_tasks: u32,

    /// Default task priority for unclassified tasks.
    #[serde(default = "SchedulerConfig::default_priority")]
    pub default_priority: String,

    /// Default max retries for failed tasks.
    #[serde(default = "SchedulerConfig::default_max_retries")]
    pub max_retries: u32,

    /// Checkpoint interval in seconds.
    #[serde(default = "SchedulerConfig::default_checkpoint_interval")]
    pub checkpoint_interval_seconds: u64,
}

impl SchedulerConfig {
    fn default_max_concurrent_tasks() -> u32 {
        16
    }
    fn default_priority() -> String {
        "medium".to_string()
    }
    fn default_max_retries() -> u32 {
        3
    }
    fn default_checkpoint_interval() -> u64 {
        60
    }
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            max_concurrent_tasks: Self::default_max_concurrent_tasks(),
            default_priority: Self::default_priority(),
            max_retries: Self::default_max_retries(),
            checkpoint_interval_seconds: Self::default_checkpoint_interval(),
        }
    }
}

/// Long-running project auto-iteration settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AutoLoopConfig {
    /// Enable project-level perpetual loop mode.
    #[serde(default)]
    pub enabled: bool,

    /// Hard cap on admitted tasks in a single iteration.
    #[serde(default = "AutoLoopConfig::default_max_tasks_per_iteration")]
    pub max_tasks_per_iteration: u32,

    /// Hard cap on runtime minutes in a single iteration.
    #[serde(default = "AutoLoopConfig::default_max_minutes_per_iteration")]
    pub max_minutes_per_iteration: u64,

    /// Hard cap on USD spend in a single iteration (0 = unlimited).
    #[serde(default = "AutoLoopConfig::default_max_cost_usd_per_iteration")]
    pub max_cost_usd_per_iteration: f64,

    /// Max retries for each task before escalation.
    #[serde(default = "AutoLoopConfig::default_retry_limit_per_task")]
    pub retry_limit_per_task: u32,

    /// Require explicit acceptance criteria for every task.
    #[serde(default = "AutoLoopConfig::default_require_dod")]
    pub require_acceptance_criteria: bool,

    /// Require at least one test command for every task.
    #[serde(default = "AutoLoopConfig::default_require_tests")]
    pub require_test_commands: bool,

    /// Operations that always require human approval.
    #[serde(default)]
    pub require_human_approval_on: Vec<String>,
}

impl AutoLoopConfig {
    fn default_max_tasks_per_iteration() -> u32 {
        20
    }
    fn default_max_minutes_per_iteration() -> u64 {
        60
    }
    fn default_max_cost_usd_per_iteration() -> f64 {
        3.0
    }
    fn default_retry_limit_per_task() -> u32 {
        2
    }
    fn default_require_dod() -> bool {
        true
    }
    fn default_require_tests() -> bool {
        true
    }
}

impl Default for AutoLoopConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_tasks_per_iteration: Self::default_max_tasks_per_iteration(),
            max_minutes_per_iteration: Self::default_max_minutes_per_iteration(),
            max_cost_usd_per_iteration: Self::default_max_cost_usd_per_iteration(),
            retry_limit_per_task: Self::default_retry_limit_per_task(),
            require_acceptance_criteria: Self::default_require_dod(),
            require_test_commands: Self::default_require_tests(),
            require_human_approval_on: vec![
                "deploy_prod".to_string(),
                "security_high".to_string(),
                "budget_exceeded".to_string(),
            ],
        }
    }
}

/// Logging / observability settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LoggingConfig {
    /// Log level: trace, debug, info, warn, error.
    #[serde(default = "LoggingConfig::default_level")]
    pub level: String,

    /// Output format: json, pretty.
    #[serde(default = "LoggingConfig::default_format")]
    pub format: String,

    /// Log directory (for file-based logging). None = stdout only.
    #[serde(default)]
    pub directory: Option<String>,

    /// Log file rotation: daily, hourly, never.
    #[serde(default = "LoggingConfig::default_rotation")]
    pub rotation: String,
}

impl LoggingConfig {
    fn default_level() -> String {
        "info".to_string()
    }
    fn default_format() -> String {
        "pretty".to_string()
    }
    fn default_rotation() -> String {
        "daily".to_string()
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: Self::default_level(),
            format: Self::default_format(),
            directory: None,
            rotation: Self::default_rotation(),
        }
    }
}

/// System-wide settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SystemConfig {
    /// Data directory for file storage.
    #[serde(default = "SystemConfig::default_data_dir")]
    pub data_dir: String,

    /// Backup schedule: "daily", "weekly", cron expression, or "disabled".
    #[serde(default = "SystemConfig::default_backup_schedule")]
    pub backup_schedule: String,

    /// Event bus capacity (number of events buffered).
    #[serde(default = "SystemConfig::default_event_bus_capacity")]
    pub event_bus_capacity: usize,
}

impl SystemConfig {
    fn default_data_dir() -> String {
        ".argus".to_string()
    }
    fn default_backup_schedule() -> String {
        "daily".to_string()
    }
    fn default_event_bus_capacity() -> usize {
        4096
    }
}

impl Default for SystemConfig {
    fn default() -> Self {
        Self {
            data_dir: Self::default_data_dir(),
            backup_schedule: Self::default_backup_schedule(),
            event_bus_capacity: Self::default_event_bus_capacity(),
        }
    }
}

// ============================================================
// Config loading
// ============================================================

impl ArgusConfig {
    /// Load configuration from the given TOML file path, falling back to defaults.
    ///
    /// If the file does not exist, returns defaults.
    /// Environment variable overrides are applied after file loading.
    pub fn load(path: &Path) -> ArgusResult<Self> {
        let mut config = if path.exists() {
            let contents = std::fs::read_to_string(path).map_err(|e| {
                ArgusError::Config(format!(
                    "Failed to read config file {}: {e}",
                    path.display()
                ))
            })?;
            toml::from_str::<ArgusConfig>(&contents).map_err(|e| {
                ArgusError::Config(format!(
                    "Failed to parse config file {}: {e}",
                    path.display()
                ))
            })?
        } else {
            tracing::info!("Config file {} not found, using defaults", path.display());
            ArgusConfig::default()
        };

        // Apply environment variable overrides
        config.apply_env_overrides()?;

        Ok(config)
    }

    /// Load configuration by searching standard paths in order:
    /// 1. `ARGUS_CONFIG_PATH` env var
    /// 2. `./argus.toml`
    /// 3. `./config/argus.toml`
    /// 4. Defaults
    pub fn load_auto() -> ArgusResult<Self> {
        if let Ok(env_path) = std::env::var("ARGUS_CONFIG_PATH") {
            return Self::load(Path::new(&env_path));
        }

        let candidates = [
            PathBuf::from("argus.toml"),
            PathBuf::from("config/argus.toml"),
        ];

        for path in &candidates {
            if path.exists() {
                tracing::info!("Loading config from {}", path.display());
                return Self::load(path);
            }
        }

        tracing::info!("No config file found, using defaults with env overrides");
        let mut config = ArgusConfig::default();
        config.apply_env_overrides()?;
        Ok(config)
    }

    /// Apply environment variable overrides to the loaded config.
    ///
    /// Supported env vars:
    /// - `ARGUS_DATABASE_URL` → database.url
    /// - `ARGUS_DATABASE_MAX_CONNECTIONS` → database.max_connections
    /// - `ARGUS_SERVER_HOST` → server.host
    /// - `ARGUS_SERVER_PORT` → server.port
    /// - `ARGUS_LOG_LEVEL` → logging.level
    /// - `ARGUS_LOG_FORMAT` → logging.format
    /// - `ARGUS_DATA_DIR` → system.data_dir
    /// - `ARGUS_QQ_*` → social.qq.*
    fn apply_env_overrides(&mut self) -> ArgusResult<()> {
        if let Some(value) = std::env::var_os("ARGUS_DEFAULT_WORKER") {
            let value = value.into_string().map_err(|_| {
                ArgusError::Config("ARGUS_DEFAULT_WORKER must be codex or claude_code".into())
            })?;
            self.execution.default_worker = match value.as_str() {
                "codex" => ExecutionWorker::Codex,
                "claude_code" => ExecutionWorker::ClaudeCode,
                _ => {
                    return Err(ArgusError::Config(
                        "ARGUS_DEFAULT_WORKER must be codex or claude_code".into(),
                    ));
                }
            };
        }
        if let Ok(val) = std::env::var("ARGUS_DATABASE_URL") {
            self.database.url = val;
        }
        if let Ok(val) = std::env::var("ARGUS_DATABASE_MAX_CONNECTIONS") {
            if let Ok(n) = val.parse::<u32>() {
                self.database.max_connections = n;
            }
        }
        if let Ok(val) = std::env::var("ARGUS_SERVER_HOST") {
            self.server.host = val;
        }
        if let Ok(val) = std::env::var("ARGUS_SERVER_PORT") {
            if let Ok(n) = val.parse::<u16>() {
                self.server.port = n;
            }
        }
        if let Ok(val) = std::env::var("ARGUS_LOG_LEVEL") {
            self.logging.level = val;
        }
        if let Ok(val) = std::env::var("ARGUS_LOG_FORMAT") {
            self.logging.format = val;
        }
        if let Ok(val) = std::env::var("ARGUS_DATA_DIR") {
            self.system.data_dir = val;
        }
        if let Ok(val) = std::env::var("ARGUS_TELEGRAM_WEBHOOK_SECRET") {
            self.social.telegram_webhook_secret = val;
        }
        if let Ok(val) = std::env::var("ARGUS_QQ_ENABLED") {
            self.social.qq.enabled = matches!(
                val.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            );
        }
        if let Ok(val) = std::env::var("ARGUS_QQ_APP_ID") {
            self.social.qq.app_id = val;
        }
        if let Ok(val) = std::env::var("ARGUS_QQ_APP_SECRET") {
            self.social.qq.app_secret = val;
        }
        if let Ok(val) = std::env::var("ARGUS_QQ_BOT_SECRET") {
            self.social.qq.bot_secret = val;
        }
        if let Ok(val) = std::env::var("ARGUS_QQ_API_BASE_URL") {
            self.social.qq.api_base_url = val;
        }
        if let Ok(val) = std::env::var("ARGUS_QQ_TOKEN_URL") {
            self.social.qq.token_url = val;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_execution_default_and_toml_selection() {
        assert_eq!(
            ArgusConfig::default().execution.default_worker,
            ExecutionWorker::Codex
        );
        let config: ArgusConfig =
            toml::from_str("[execution]\ndefault_worker = 'claude_code'").unwrap();
        assert_eq!(
            config.execution.default_worker.worker_type(),
            crate::worker_types::WorkerType::ClaudeCode
        );
    }

    #[test]
    fn test_execution_invalid_toml_is_rejected() {
        for value in ["", "generic_cli", "claude", "CODEX"] {
            assert!(
                toml::from_str::<ArgusConfig>(&format!("[execution]\ndefault_worker = '{value}'"))
                    .is_err()
            );
        }
    }

    #[test]
    fn test_default_config() {
        let config = ArgusConfig::default();
        assert_eq!(config.database.max_connections, 20);
        assert_eq!(config.server.port, 3100);
        assert_eq!(config.worker.max_pool_size, 8);
        assert_eq!(config.memory.short_term_ttl_hours, 72);
        assert_eq!(config.logging.level, "info");
        assert_eq!(config.security.default_actor_role, "developer");
        assert_eq!(config.autoloop.max_tasks_per_iteration, 20);
        assert!(!config.social.qq.enabled);
    }

    #[test]
    fn test_config_serialization_roundtrip() {
        let config = ArgusConfig::default();
        let toml_str = toml::to_string_pretty(&config).expect("serialize");
        let parsed: ArgusConfig = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(parsed.database.url, config.database.url);
        assert_eq!(parsed.server.port, config.server.port);
        assert_eq!(parsed.worker.max_pool_size, config.worker.max_pool_size);
        assert_eq!(parsed.social.qq.api_base_url, config.social.qq.api_base_url);
        assert_eq!(parsed.autoloop.retry_limit_per_task, 2);
    }

    #[test]
    fn test_load_nonexistent_file() {
        let result = ArgusConfig::load(Path::new("/nonexistent/path.toml"));
        assert!(result.is_ok());
        let config = result.unwrap();
        assert_eq!(config.database.max_connections, 20);
    }

    #[test]
    fn test_partial_toml_parsing() {
        let toml_str = r#"
[database]
max_connections = 50

[server]
port = 4200
"#;
        let config: ArgusConfig = toml::from_str(toml_str).expect("parse partial toml");
        assert_eq!(config.database.max_connections, 50);
        assert_eq!(config.server.port, 4200);
        // Defaults for unspecified fields
        assert_eq!(config.worker.max_pool_size, 8);
        assert_eq!(config.memory.short_term_ttl_hours, 72);
        assert!(!config.social.qq.enabled);
    }
}
