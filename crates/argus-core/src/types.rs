//! Shared types used across all Argus crates.
//!
//! Contains common ID types, error types, enums, and result aliases.

use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

pub use crate::task_types::TaskState;
pub use crate::worker_types::{WorkerState, WorkerType};

// ============================================================
// ID type aliases
// ============================================================

/// Unique identifier for a project.
pub type ProjectId = Uuid;

/// Unique identifier for a milestone.
pub type MilestoneId = Uuid;

/// Unique identifier for a task.
pub type TaskId = Uuid;

/// Unique identifier for a worker.
pub type WorkerId = Uuid;

/// Unique identifier for a deployment.
pub type DeploymentId = Uuid;

/// Unique identifier for a memory entry.
pub type MemoryId = Uuid;

/// Unique identifier for an identity (user).
pub type IdentityId = Uuid;

/// Unique identifier for a collaboration session.
pub type SessionId = Uuid;

/// Unique identifier for an evolution request.
pub type EvolutionId = Uuid;

// ============================================================
// Common enums (matching DATABASE_SCHEMA.md ENUM types)
// ============================================================

/// Task priority levels.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "task_priority", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum TaskPriority {
    Critical,
    High,
    Medium,
    Low,
    Background,
}

impl fmt::Display for TaskPriority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Critical => "critical",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
            Self::Background => "background",
        };
        write!(f, "{s}")
    }
}

/// Deployment environment.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "environment", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum Environment {
    Development,
    Staging,
    Production,
    Custom,
}

/// Deployment status.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "deployment_status", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum DeploymentStatus {
    Queued,
    Running,
    WaitingApproval,
    Approved,
    Succeeded,
    Failed,
    RolledBack,
    Cancelled,
}

/// Memory storage layer.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "memory_layer", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum MemoryLayer {
    Working,
    ShortTerm,
    LongTerm,
    Permanent,
}

/// Memory content type.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "memory_type", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum MemoryType {
    Fact,
    Decision,
    Experience,
    Preference,
    ErrorPattern,
    SkillLearned,
    ProjectContext,
    ConversationSummary,
    SafetyRule,
}

/// Trust level for MCP servers and skills.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "trust_level", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum TrustLevel {
    Builtin,
    Verified,
    ThirdParty,
    Untrusted,
}

/// User role (RBAC).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "role", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Owner,
    Admin,
    Developer,
    Viewer,
}

/// Communication platform.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "platform", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Telegram,
    Discord,
    Wechat,
    Slack,
    WebDashboard,
}

/// Audit log outcome.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "audit_outcome", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum AuditOutcome {
    Success,
    Failure,
    Blocked,
    Warned,
}

/// Self-evolution status.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "evolution_status", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum EvolutionStatus {
    Requested,
    SafetyChecking,
    SafetyApproved,
    SafetyRejected,
    BranchCreated,
    InDevelopment,
    Testing,
    TestsPassed,
    TestsFailed,
    TestInstanceRunning,
    AwaitingApproval,
    Approved,
    Rejected,
    Deploying,
    Deployed,
    HealthCheckPassed,
    HealthCheckFailed,
    RolledBack,
    Completed,
    Failed,
}

/// Collaboration mode.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "collaboration_mode", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum CollaborationMode {
    Solo,
    PairReview,
    FullTeam,
}

/// Collaboration phase.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "collaboration_phase", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum CollaborationPhase {
    Architecture,
    Implementation,
    Review,
    Testing,
    SecurityAudit,
    Merging,
    Completed,
    Failed,
}

/// Collaboration role.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "collaboration_role", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum CollaborationRole {
    Architect,
    Developer,
    Reviewer,
    Tester,
    SecurityAuditor,
    Devops,
    TechLead,
}

// ============================================================
// Error types
// ============================================================

/// Top-level Argus error type.
#[derive(Debug, thiserror::Error)]
pub enum ArgusError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Task error: {0}")]
    Task(String),

    #[error("Worker error: {0}")]
    Worker(String),

    #[error("LLM error: {0}")]
    Llm(String),

    #[error("Memory error: {0}")]
    Memory(String),

    #[error("Deploy error: {0}")]
    Deploy(String),

    #[error("Git error: {0}")]
    Git(String),

    #[error("Workspace error: {0}")]
    Workspace(String),

    #[error("MCP error: {0}")]
    Mcp(String),

    #[error("Security error: {0}")]
    Security(String),

    #[error("Social error: {0}")]
    Social(String),

    #[error("Evolution error: {0}")]
    Evolution(String),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

/// Result alias using ArgusError.
pub type ArgusResult<T> = Result<T, ArgusError>;
