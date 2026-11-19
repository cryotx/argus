//! Event types for the Argus event bus.
//!
//! All inter-module communication flows through the event bus using these types.
//! See API_DESIGN.md Section 2 for the full specification.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::types::{TaskPriority, WorkerType};

// ============================================================
// EventEnvelope — wraps every event with metadata
// ============================================================

/// Wraps an `ArgusEvent` with routing and audit metadata.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventEnvelope {
    /// Unique event ID (for dedup, correlation, audit).
    pub id: Uuid,
    /// When the event was created.
    pub timestamp: DateTime<Utc>,
    /// Which module published the event.
    pub source: EventSource,
    /// The actual event payload.
    pub event: ArgusEvent,
    /// Optional correlation ID to link related events (e.g., task chain).
    pub correlation_id: Option<Uuid>,
}

impl EventEnvelope {
    /// Create a new envelope for the given source and event.
    pub fn new(source: EventSource, event: ArgusEvent) -> Self {
        Self {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            source,
            event,
            correlation_id: None,
        }
    }

    /// Create a new envelope with a correlation ID.
    pub fn with_correlation(source: EventSource, event: ArgusEvent, correlation_id: Uuid) -> Self {
        Self {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            source,
            event,
            correlation_id: Some(correlation_id),
        }
    }
}

// ============================================================
// EventSource — identifies who published the event
// ============================================================

/// Identifies the module that published an event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventSource {
    Brain,
    Scheduler,
    WorkerManager,
    Worker(Uuid),
    LlmProvider,
    MemoryEngine,
    McpClient,
    SkillEngine,
    GitOps,
    DeployEngine,
    SocialGateway,
    Security,
    Monitor,
    ConversationalConfig,
    SelfEvolution,
    WebApi,
    System,
}

// ============================================================
// ArgusEvent — all events that flow through the bus
// ============================================================

/// All events in Argus. Every variant carries enough data for any subscriber
/// to act without needing to query back.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ArgusEvent {
    // === Project Lifecycle ===
    ProjectCreated {
        project_id: Uuid,
        name: String,
        git_repo_url: Option<String>,
        language: Option<String>,
    },
    ProjectUpdated {
        project_id: Uuid,
        changes: serde_json::Value,
    },
    ProjectPaused {
        project_id: Uuid,
    },
    ProjectResumed {
        project_id: Uuid,
    },
    ProjectDeleted {
        project_id: Uuid,
    },

    // === Milestone Lifecycle ===
    MilestoneCreated {
        milestone_id: Uuid,
        project_id: Uuid,
        name: String,
        sequence: i32,
    },
    MilestoneCompleted {
        milestone_id: Uuid,
        project_id: Uuid,
        total_tasks: i32,
        duration_seconds: i64,
    },
    MilestoneFailed {
        milestone_id: Uuid,
        project_id: Uuid,
        reason: String,
    },

    // === Task Lifecycle ===
    TaskCreated {
        task_id: Uuid,
        project_id: Uuid,
        title: String,
        task_type: String,
        priority: TaskPriority,
    },
    TaskQueued {
        task_id: Uuid,
    },
    TaskReady {
        task_id: Uuid,
    },
    TaskAssigned {
        task_id: Uuid,
        worker_id: Uuid,
        worker_type: WorkerType,
    },
    TaskStarted {
        task_id: Uuid,
    },
    TaskProgress {
        task_id: Uuid,
        progress: f32,
        message: Option<String>,
    },
    TaskCompleted {
        task_id: Uuid,
        output: serde_json::Value,
        duration_seconds: i64,
    },
    TaskFailed {
        task_id: Uuid,
        error: String,
        attempt: u32,
        will_retry: bool,
    },
    TaskRetrying {
        task_id: Uuid,
        attempt: u32,
        max_retries: u32,
        backoff_seconds: u64,
    },
    TaskCancelled {
        task_id: Uuid,
        reason: String,
    },
    TaskTimedOut {
        task_id: Uuid,
        timeout_seconds: u64,
    },
    TaskCheckpointed {
        task_id: Uuid,
        progress: f32,
    },

    // === Worker ===
    WorkerSpawned {
        worker_id: Uuid,
        worker_type: WorkerType,
        sandbox_type: String,
    },
    WorkerReady {
        worker_id: Uuid,
    },
    WorkerBusy {
        worker_id: Uuid,
        task_id: Uuid,
    },
    WorkerIdle {
        worker_id: Uuid,
    },
    WorkerCrashed {
        worker_id: Uuid,
        error: String,
        task_id: Option<Uuid>,
    },
    WorkerRecovered {
        worker_id: Uuid,
    },
    WorkerShuttingDown {
        worker_id: Uuid,
    },
    WorkerResourceAlert {
        worker_id: Uuid,
        resource: String,
        current: f64,
        limit: f64,
    },

    // === Git ===
    GitCommitCreated {
        project_id: Uuid,
        branch: String,
        sha: String,
        message: String,
        files_changed: u32,
    },
    GitPushSucceeded {
        project_id: Uuid,
        branch: String,
        remote: String,
    },
    GitPushFailed {
        project_id: Uuid,
        branch: String,
        error: String,
    },
    GitPrCreated {
        project_id: Uuid,
        pr_number: u64,
        title: String,
        url: String,
    },
    GitPrMerged {
        project_id: Uuid,
        pr_number: u64,
        branch: String,
    },
    GitConflictDetected {
        project_id: Uuid,
        branch: String,
        conflicting_files: Vec<String>,
    },
    GitBranchCreated {
        project_id: Uuid,
        branch: String,
        base_branch: String,
    },

    // === Deployment ===
    DeployQueued {
        deployment_id: Uuid,
        project_id: Uuid,
        environment: String,
        target_type: String,
    },
    DeployStarted {
        deployment_id: Uuid,
        project_id: Uuid,
    },
    DeployStageStarted {
        deployment_id: Uuid,
        stage: String,
    },
    DeployStageCompleted {
        deployment_id: Uuid,
        stage: String,
        duration_seconds: i64,
    },
    DeployStageFailed {
        deployment_id: Uuid,
        stage: String,
        error: String,
    },
    DeployAwaitingApproval {
        deployment_id: Uuid,
        project_id: Uuid,
        environment: String,
    },
    DeployApproved {
        deployment_id: Uuid,
        approved_by: Uuid,
    },
    DeployRejected {
        deployment_id: Uuid,
        rejected_by: Uuid,
        reason: String,
    },
    DeploySucceeded {
        deployment_id: Uuid,
        project_id: Uuid,
        environment: String,
        duration_seconds: i64,
    },
    DeployFailed {
        deployment_id: Uuid,
        project_id: Uuid,
        error: String,
    },
    DeployRolledBack {
        deployment_id: Uuid,
        project_id: Uuid,
        snapshot_id: Uuid,
    },
    HealthCheckPassed {
        deployment_id: Uuid,
        project_id: Uuid,
    },
    HealthCheckFailed {
        deployment_id: Uuid,
        project_id: Uuid,
        error: String,
    },

    // === LLM ===
    LlmRequestSent {
        provider: String,
        model: String,
        model_class: String,
        estimated_tokens: u32,
        purpose: String,
        trace_id: Option<String>,
    },
    LlmResponseReceived {
        provider: String,
        model: String,
        input_tokens: u32,
        output_tokens: u32,
        cost_usd: f64,
        latency_ms: u64,
        trace_id: Option<String>,
    },
    LlmRequestFailed {
        provider: String,
        model: String,
        error: String,
        will_failover: bool,
        trace_id: Option<String>,
    },
    LlmKeyExhausted {
        provider: String,
        key_index: u32,
        next_key_index: Option<u32>,
    },
    LlmProviderDown {
        provider: String,
        error: String,
    },
    LlmProviderRecovered {
        provider: String,
    },
    LlmProviderSwitched {
        from_provider: String,
        to_provider: String,
        reason: String,
    },
    LlmBudgetWarning {
        provider: String,
        remaining_percent: f64,
        monthly_budget: f64,
        current_usage: f64,
    },
    LlmBudgetExhausted {
        provider: String,
    },

    // === Social / Communication ===
    MessageReceived {
        platform: String,
        sender_id: Uuid,
        content_type: String,
        content_preview: String,
    },
    MessageSent {
        platform: String,
        message_id: String,
    },
    PlatformConnected {
        platform: String,
    },
    PlatformDisconnected {
        platform: String,
        error: Option<String>,
    },
    PlatformReconnecting {
        platform: String,
        attempt: u32,
    },
    NotificationQueued {
        identity_id: Uuid,
        priority: String,
        title: String,
    },
    NotificationDelivered {
        identity_id: Uuid,
        platform: String,
    },
    DigestSent {
        identity_id: Uuid,
        platform: String,
        message_count: u32,
    },

    // === Memory ===
    MemoryStored {
        memory_id: Uuid,
        layer: String,
        memory_type: String,
        project_id: Option<Uuid>,
    },
    MemoryRecalled {
        query: String,
        results_count: u32,
        top_similarity: f64,
        latency_ms: u64,
    },
    MemoryPromoted {
        memory_id: Uuid,
        from_layer: String,
        to_layer: String,
    },
    MemoryConsolidated {
        source_ids: Vec<Uuid>,
        consolidated_id: Uuid,
        summary_length: usize,
    },
    MemoryExpired {
        memory_id: Uuid,
        layer: String,
        reason: String,
    },
    MemoryDecayRun {
        affected_count: u32,
        deleted_count: u32,
        promoted_count: u32,
    },

    // === MCP / Skills ===
    McpServerInstalled {
        name: String,
        protocol: String,
        trust_level: String,
    },
    McpServerRemoved {
        name: String,
    },
    McpServerConnected {
        name: String,
        tools_count: u32,
    },
    McpServerDisconnected {
        name: String,
        error: Option<String>,
    },
    McpToolCalled {
        server: String,
        tool: String,
        duration_ms: u64,
        success: bool,
    },
    McpAuditCompleted {
        server_name: String,
        verdict: String,
        trust_level_assigned: String,
    },
    SkillInstalled {
        name: String,
        skill_type: String,
        trust_level: String,
    },
    SkillRemoved {
        name: String,
    },
    SkillGenerated {
        name: String,
        description: String,
        language: String,
    },
    SkillExecuted {
        name: String,
        duration_ms: u64,
        success: bool,
    },

    // === Config ===
    ConfigChanged {
        domain: String,
        key: String,
        previous_value: Option<serde_json::Value>,
        new_value: serde_json::Value,
        source: String,
    },
    ConfigUndone {
        domain: String,
        key: String,
        restored_value: serde_json::Value,
    },

    // === Security ===
    SecurityAuditCompleted {
        target: String,
        result: String,
        details: serde_json::Value,
    },
    DangerousOperationBlocked {
        command: String,
        severity: String,
        source: String,
    },
    UnauthorizedAccessAttempt {
        identity_id: Option<Uuid>,
        resource: String,
        action: String,
    },
    SecretDetected {
        location: String,
        secret_type: String,
        action: String,
    },
    RateLimitExceeded {
        identity_id: Uuid,
        endpoint: String,
    },

    // === Self-Evolution ===
    EvolutionRequested {
        evolution_id: Uuid,
        category: String,
        description: String,
    },
    EvolutionSafetyApproved {
        evolution_id: Uuid,
    },
    EvolutionSafetyRejected {
        evolution_id: Uuid,
        reason: String,
    },
    EvolutionBranchCreated {
        evolution_id: Uuid,
        branch: String,
    },
    EvolutionTestsPassed {
        evolution_id: Uuid,
        test_count: u32,
        duration_seconds: i64,
    },
    EvolutionTestsFailed {
        evolution_id: Uuid,
        failures: u32,
        details: String,
    },
    EvolutionDeployed {
        evolution_id: Uuid,
        strategy: String,
    },
    EvolutionCompleted {
        evolution_id: Uuid,
        files_changed: u32,
    },
    EvolutionRolledBack {
        evolution_id: Uuid,
        reason: String,
    },
    EvolutionGuardrailTriggered {
        evolution_id: Uuid,
        rule: String,
    },

    // === System ===
    SystemStarted {
        version: String,
        modules_loaded: Vec<String>,
    },
    SystemShuttingDown {
        reason: String,
    },
    HealthReport {
        cpu_percent: f64,
        memory_used_mb: u64,
        memory_total_mb: u64,
        disk_used_percent: f64,
        active_workers: u32,
        active_tasks: u32,
        uptime_seconds: u64,
    },
    BackupCompleted {
        backup_id: Uuid,
        size_bytes: u64,
        duration_seconds: i64,
    },
    BackupFailed {
        error: String,
    },
    ScheduledJobRun {
        job_name: String,
        duration_ms: u64,
        result: String,
    },

    // === Collaboration ===
    CollaborationStarted {
        session_id: Uuid,
        task_id: Uuid,
        mode: String,
        risk_level: String,
        roles: Vec<String>,
    },
    CollaborationPhaseChanged {
        session_id: Uuid,
        task_id: Uuid,
        phase: String,
    },
    CollaborationRoleAssigned {
        session_id: Uuid,
        role: String,
        worker_id: Uuid,
        worker_type: String,
        model: String,
    },
    CollaborationRoleCompleted {
        session_id: Uuid,
        role: String,
        worker_id: Uuid,
        duration_seconds: i64,
    },
    CollaborationReviewSubmitted {
        session_id: Uuid,
        round: u32,
        verdict: String,
        issues_count: u32,
    },
    CollaborationCompleted {
        session_id: Uuid,
        task_id: Uuid,
        mode: String,
        total_duration_seconds: i64,
        total_roles_executed: u32,
        review_rounds: u32,
    },
    CollaborationFailed {
        session_id: Uuid,
        task_id: Uuid,
        reason: String,
        phase: String,
    },
}

/// Extract `project_id` from an event if it contains one.
/// Used by the filtered subscription helper.
pub fn extract_project_id(event: &ArgusEvent) -> Option<Uuid> {
    match event {
        // Project
        ArgusEvent::ProjectCreated { project_id, .. }
        | ArgusEvent::ProjectUpdated { project_id, .. }
        | ArgusEvent::ProjectPaused { project_id }
        | ArgusEvent::ProjectResumed { project_id }
        | ArgusEvent::ProjectDeleted { project_id } => Some(*project_id),

        // Milestone
        ArgusEvent::MilestoneCreated { project_id, .. }
        | ArgusEvent::MilestoneCompleted { project_id, .. }
        | ArgusEvent::MilestoneFailed { project_id, .. } => Some(*project_id),

        // Task
        ArgusEvent::TaskCreated { project_id, .. } => Some(*project_id),

        // Git
        ArgusEvent::GitCommitCreated { project_id, .. }
        | ArgusEvent::GitPushSucceeded { project_id, .. }
        | ArgusEvent::GitPushFailed { project_id, .. }
        | ArgusEvent::GitPrCreated { project_id, .. }
        | ArgusEvent::GitPrMerged { project_id, .. }
        | ArgusEvent::GitConflictDetected { project_id, .. }
        | ArgusEvent::GitBranchCreated { project_id, .. } => Some(*project_id),

        // Deploy
        ArgusEvent::DeployQueued { project_id, .. }
        | ArgusEvent::DeployStarted { project_id, .. }
        | ArgusEvent::DeployAwaitingApproval { project_id, .. }
        | ArgusEvent::DeploySucceeded { project_id, .. }
        | ArgusEvent::DeployFailed { project_id, .. }
        | ArgusEvent::DeployRolledBack { project_id, .. }
        | ArgusEvent::HealthCheckPassed { project_id, .. }
        | ArgusEvent::HealthCheckFailed { project_id, .. } => Some(*project_id),

        // Memory (optional project_id)
        ArgusEvent::MemoryStored { project_id, .. } => *project_id,

        // Everything else
        _ => None,
    }
}

/// Returns the event variant name as a string (for filtering/logging).
pub fn event_name(event: &ArgusEvent) -> &'static str {
    match event {
        ArgusEvent::ProjectCreated { .. } => "ProjectCreated",
        ArgusEvent::ProjectUpdated { .. } => "ProjectUpdated",
        ArgusEvent::ProjectPaused { .. } => "ProjectPaused",
        ArgusEvent::ProjectResumed { .. } => "ProjectResumed",
        ArgusEvent::ProjectDeleted { .. } => "ProjectDeleted",
        ArgusEvent::MilestoneCreated { .. } => "MilestoneCreated",
        ArgusEvent::MilestoneCompleted { .. } => "MilestoneCompleted",
        ArgusEvent::MilestoneFailed { .. } => "MilestoneFailed",
        ArgusEvent::TaskCreated { .. } => "TaskCreated",
        ArgusEvent::TaskQueued { .. } => "TaskQueued",
        ArgusEvent::TaskReady { .. } => "TaskReady",
        ArgusEvent::TaskAssigned { .. } => "TaskAssigned",
        ArgusEvent::TaskStarted { .. } => "TaskStarted",
        ArgusEvent::TaskProgress { .. } => "TaskProgress",
        ArgusEvent::TaskCompleted { .. } => "TaskCompleted",
        ArgusEvent::TaskFailed { .. } => "TaskFailed",
        ArgusEvent::TaskRetrying { .. } => "TaskRetrying",
        ArgusEvent::TaskCancelled { .. } => "TaskCancelled",
        ArgusEvent::TaskTimedOut { .. } => "TaskTimedOut",
        ArgusEvent::TaskCheckpointed { .. } => "TaskCheckpointed",
        ArgusEvent::WorkerSpawned { .. } => "WorkerSpawned",
        ArgusEvent::WorkerReady { .. } => "WorkerReady",
        ArgusEvent::WorkerBusy { .. } => "WorkerBusy",
        ArgusEvent::WorkerIdle { .. } => "WorkerIdle",
        ArgusEvent::WorkerCrashed { .. } => "WorkerCrashed",
        ArgusEvent::WorkerRecovered { .. } => "WorkerRecovered",
        ArgusEvent::WorkerShuttingDown { .. } => "WorkerShuttingDown",
        ArgusEvent::WorkerResourceAlert { .. } => "WorkerResourceAlert",
        ArgusEvent::GitCommitCreated { .. } => "GitCommitCreated",
        ArgusEvent::GitPushSucceeded { .. } => "GitPushSucceeded",
        ArgusEvent::GitPushFailed { .. } => "GitPushFailed",
        ArgusEvent::GitPrCreated { .. } => "GitPrCreated",
        ArgusEvent::GitPrMerged { .. } => "GitPrMerged",
        ArgusEvent::GitConflictDetected { .. } => "GitConflictDetected",
        ArgusEvent::GitBranchCreated { .. } => "GitBranchCreated",
        ArgusEvent::DeployQueued { .. } => "DeployQueued",
        ArgusEvent::DeployStarted { .. } => "DeployStarted",
        ArgusEvent::DeployStageStarted { .. } => "DeployStageStarted",
        ArgusEvent::DeployStageCompleted { .. } => "DeployStageCompleted",
        ArgusEvent::DeployStageFailed { .. } => "DeployStageFailed",
        ArgusEvent::DeployAwaitingApproval { .. } => "DeployAwaitingApproval",
        ArgusEvent::DeployApproved { .. } => "DeployApproved",
        ArgusEvent::DeployRejected { .. } => "DeployRejected",
        ArgusEvent::DeploySucceeded { .. } => "DeploySucceeded",
        ArgusEvent::DeployFailed { .. } => "DeployFailed",
        ArgusEvent::DeployRolledBack { .. } => "DeployRolledBack",
        ArgusEvent::HealthCheckPassed { .. } => "HealthCheckPassed",
        ArgusEvent::HealthCheckFailed { .. } => "HealthCheckFailed",
        ArgusEvent::LlmRequestSent { .. } => "LlmRequestSent",
        ArgusEvent::LlmResponseReceived { .. } => "LlmResponseReceived",
        ArgusEvent::LlmRequestFailed { .. } => "LlmRequestFailed",
        ArgusEvent::LlmKeyExhausted { .. } => "LlmKeyExhausted",
        ArgusEvent::LlmProviderDown { .. } => "LlmProviderDown",
        ArgusEvent::LlmProviderRecovered { .. } => "LlmProviderRecovered",
        ArgusEvent::LlmProviderSwitched { .. } => "LlmProviderSwitched",
        ArgusEvent::LlmBudgetWarning { .. } => "LlmBudgetWarning",
        ArgusEvent::LlmBudgetExhausted { .. } => "LlmBudgetExhausted",
        ArgusEvent::MessageReceived { .. } => "MessageReceived",
        ArgusEvent::MessageSent { .. } => "MessageSent",
        ArgusEvent::PlatformConnected { .. } => "PlatformConnected",
        ArgusEvent::PlatformDisconnected { .. } => "PlatformDisconnected",
        ArgusEvent::PlatformReconnecting { .. } => "PlatformReconnecting",
        ArgusEvent::NotificationQueued { .. } => "NotificationQueued",
        ArgusEvent::NotificationDelivered { .. } => "NotificationDelivered",
        ArgusEvent::DigestSent { .. } => "DigestSent",
        ArgusEvent::MemoryStored { .. } => "MemoryStored",
        ArgusEvent::MemoryRecalled { .. } => "MemoryRecalled",
        ArgusEvent::MemoryPromoted { .. } => "MemoryPromoted",
        ArgusEvent::MemoryConsolidated { .. } => "MemoryConsolidated",
        ArgusEvent::MemoryExpired { .. } => "MemoryExpired",
        ArgusEvent::MemoryDecayRun { .. } => "MemoryDecayRun",
        ArgusEvent::McpServerInstalled { .. } => "McpServerInstalled",
        ArgusEvent::McpServerRemoved { .. } => "McpServerRemoved",
        ArgusEvent::McpServerConnected { .. } => "McpServerConnected",
        ArgusEvent::McpServerDisconnected { .. } => "McpServerDisconnected",
        ArgusEvent::McpToolCalled { .. } => "McpToolCalled",
        ArgusEvent::McpAuditCompleted { .. } => "McpAuditCompleted",
        ArgusEvent::SkillInstalled { .. } => "SkillInstalled",
        ArgusEvent::SkillRemoved { .. } => "SkillRemoved",
        ArgusEvent::SkillGenerated { .. } => "SkillGenerated",
        ArgusEvent::SkillExecuted { .. } => "SkillExecuted",
        ArgusEvent::ConfigChanged { .. } => "ConfigChanged",
        ArgusEvent::ConfigUndone { .. } => "ConfigUndone",
        ArgusEvent::SecurityAuditCompleted { .. } => "SecurityAuditCompleted",
        ArgusEvent::DangerousOperationBlocked { .. } => "DangerousOperationBlocked",
        ArgusEvent::UnauthorizedAccessAttempt { .. } => "UnauthorizedAccessAttempt",
        ArgusEvent::SecretDetected { .. } => "SecretDetected",
        ArgusEvent::RateLimitExceeded { .. } => "RateLimitExceeded",
        ArgusEvent::EvolutionRequested { .. } => "EvolutionRequested",
        ArgusEvent::EvolutionSafetyApproved { .. } => "EvolutionSafetyApproved",
        ArgusEvent::EvolutionSafetyRejected { .. } => "EvolutionSafetyRejected",
        ArgusEvent::EvolutionBranchCreated { .. } => "EvolutionBranchCreated",
        ArgusEvent::EvolutionTestsPassed { .. } => "EvolutionTestsPassed",
        ArgusEvent::EvolutionTestsFailed { .. } => "EvolutionTestsFailed",
        ArgusEvent::EvolutionDeployed { .. } => "EvolutionDeployed",
        ArgusEvent::EvolutionCompleted { .. } => "EvolutionCompleted",
        ArgusEvent::EvolutionRolledBack { .. } => "EvolutionRolledBack",
        ArgusEvent::EvolutionGuardrailTriggered { .. } => "EvolutionGuardrailTriggered",
        ArgusEvent::SystemStarted { .. } => "SystemStarted",
        ArgusEvent::SystemShuttingDown { .. } => "SystemShuttingDown",
        ArgusEvent::HealthReport { .. } => "HealthReport",
        ArgusEvent::BackupCompleted { .. } => "BackupCompleted",
        ArgusEvent::BackupFailed { .. } => "BackupFailed",
        ArgusEvent::ScheduledJobRun { .. } => "ScheduledJobRun",
        ArgusEvent::CollaborationStarted { .. } => "CollaborationStarted",
        ArgusEvent::CollaborationPhaseChanged { .. } => "CollaborationPhaseChanged",
        ArgusEvent::CollaborationRoleAssigned { .. } => "CollaborationRoleAssigned",
        ArgusEvent::CollaborationRoleCompleted { .. } => "CollaborationRoleCompleted",
        ArgusEvent::CollaborationReviewSubmitted { .. } => "CollaborationReviewSubmitted",
        ArgusEvent::CollaborationCompleted { .. } => "CollaborationCompleted",
        ArgusEvent::CollaborationFailed { .. } => "CollaborationFailed",
    }
}

/// Returns the event category for WebSocket subscription filtering.
pub fn event_category(event: &ArgusEvent) -> &'static str {
    match event {
        ArgusEvent::ProjectCreated { .. }
        | ArgusEvent::ProjectUpdated { .. }
        | ArgusEvent::ProjectPaused { .. }
        | ArgusEvent::ProjectResumed { .. }
        | ArgusEvent::ProjectDeleted { .. } => "Projects",

        ArgusEvent::MilestoneCreated { .. }
        | ArgusEvent::MilestoneCompleted { .. }
        | ArgusEvent::MilestoneFailed { .. } => "Milestones",

        ArgusEvent::TaskCreated { .. }
        | ArgusEvent::TaskQueued { .. }
        | ArgusEvent::TaskReady { .. }
        | ArgusEvent::TaskAssigned { .. }
        | ArgusEvent::TaskStarted { .. }
        | ArgusEvent::TaskProgress { .. }
        | ArgusEvent::TaskCompleted { .. }
        | ArgusEvent::TaskFailed { .. }
        | ArgusEvent::TaskRetrying { .. }
        | ArgusEvent::TaskCancelled { .. }
        | ArgusEvent::TaskTimedOut { .. }
        | ArgusEvent::TaskCheckpointed { .. } => "Tasks",

        ArgusEvent::WorkerSpawned { .. }
        | ArgusEvent::WorkerReady { .. }
        | ArgusEvent::WorkerBusy { .. }
        | ArgusEvent::WorkerIdle { .. }
        | ArgusEvent::WorkerCrashed { .. }
        | ArgusEvent::WorkerRecovered { .. }
        | ArgusEvent::WorkerShuttingDown { .. }
        | ArgusEvent::WorkerResourceAlert { .. } => "Workers",

        ArgusEvent::GitCommitCreated { .. }
        | ArgusEvent::GitPushSucceeded { .. }
        | ArgusEvent::GitPushFailed { .. }
        | ArgusEvent::GitPrCreated { .. }
        | ArgusEvent::GitPrMerged { .. }
        | ArgusEvent::GitConflictDetected { .. }
        | ArgusEvent::GitBranchCreated { .. } => "Git",

        ArgusEvent::DeployQueued { .. }
        | ArgusEvent::DeployStarted { .. }
        | ArgusEvent::DeployStageStarted { .. }
        | ArgusEvent::DeployStageCompleted { .. }
        | ArgusEvent::DeployStageFailed { .. }
        | ArgusEvent::DeployAwaitingApproval { .. }
        | ArgusEvent::DeployApproved { .. }
        | ArgusEvent::DeployRejected { .. }
        | ArgusEvent::DeploySucceeded { .. }
        | ArgusEvent::DeployFailed { .. }
        | ArgusEvent::DeployRolledBack { .. }
        | ArgusEvent::HealthCheckPassed { .. }
        | ArgusEvent::HealthCheckFailed { .. } => "Deployments",

        ArgusEvent::LlmRequestSent { .. }
        | ArgusEvent::LlmResponseReceived { .. }
        | ArgusEvent::LlmRequestFailed { .. }
        | ArgusEvent::LlmKeyExhausted { .. }
        | ArgusEvent::LlmProviderDown { .. }
        | ArgusEvent::LlmProviderRecovered { .. }
        | ArgusEvent::LlmProviderSwitched { .. }
        | ArgusEvent::LlmBudgetWarning { .. }
        | ArgusEvent::LlmBudgetExhausted { .. } => "LLM",

        ArgusEvent::MessageReceived { .. }
        | ArgusEvent::MessageSent { .. }
        | ArgusEvent::PlatformConnected { .. }
        | ArgusEvent::PlatformDisconnected { .. }
        | ArgusEvent::PlatformReconnecting { .. }
        | ArgusEvent::NotificationQueued { .. }
        | ArgusEvent::NotificationDelivered { .. }
        | ArgusEvent::DigestSent { .. } => "Social",

        ArgusEvent::MemoryStored { .. }
        | ArgusEvent::MemoryRecalled { .. }
        | ArgusEvent::MemoryPromoted { .. }
        | ArgusEvent::MemoryConsolidated { .. }
        | ArgusEvent::MemoryExpired { .. }
        | ArgusEvent::MemoryDecayRun { .. } => "Memory",

        ArgusEvent::McpServerInstalled { .. }
        | ArgusEvent::McpServerRemoved { .. }
        | ArgusEvent::McpServerConnected { .. }
        | ArgusEvent::McpServerDisconnected { .. }
        | ArgusEvent::McpToolCalled { .. }
        | ArgusEvent::McpAuditCompleted { .. }
        | ArgusEvent::SkillInstalled { .. }
        | ArgusEvent::SkillRemoved { .. }
        | ArgusEvent::SkillGenerated { .. }
        | ArgusEvent::SkillExecuted { .. } => "McpSkills",

        ArgusEvent::ConfigChanged { .. } | ArgusEvent::ConfigUndone { .. } => "Config",

        ArgusEvent::SecurityAuditCompleted { .. }
        | ArgusEvent::DangerousOperationBlocked { .. }
        | ArgusEvent::UnauthorizedAccessAttempt { .. }
        | ArgusEvent::SecretDetected { .. }
        | ArgusEvent::RateLimitExceeded { .. } => "Security",

        ArgusEvent::EvolutionRequested { .. }
        | ArgusEvent::EvolutionSafetyApproved { .. }
        | ArgusEvent::EvolutionSafetyRejected { .. }
        | ArgusEvent::EvolutionBranchCreated { .. }
        | ArgusEvent::EvolutionTestsPassed { .. }
        | ArgusEvent::EvolutionTestsFailed { .. }
        | ArgusEvent::EvolutionDeployed { .. }
        | ArgusEvent::EvolutionCompleted { .. }
        | ArgusEvent::EvolutionRolledBack { .. }
        | ArgusEvent::EvolutionGuardrailTriggered { .. } => "Evolution",

        ArgusEvent::SystemStarted { .. }
        | ArgusEvent::SystemShuttingDown { .. }
        | ArgusEvent::HealthReport { .. }
        | ArgusEvent::BackupCompleted { .. }
        | ArgusEvent::BackupFailed { .. }
        | ArgusEvent::ScheduledJobRun { .. } => "System",

        ArgusEvent::CollaborationStarted { .. }
        | ArgusEvent::CollaborationPhaseChanged { .. }
        | ArgusEvent::CollaborationRoleAssigned { .. }
        | ArgusEvent::CollaborationRoleCompleted { .. }
        | ArgusEvent::CollaborationReviewSubmitted { .. }
        | ArgusEvent::CollaborationCompleted { .. }
        | ArgusEvent::CollaborationFailed { .. } => "Collaborations",
    }
}
