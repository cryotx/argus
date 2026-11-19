# API Design — Event Bus, REST API, and WebSocket

This document defines all internal and external APIs in Argus: the internal event bus (pub/sub), the Web Dashboard REST API, and the WebSocket real-time API.

## 1. Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                         Argus Process                                │
│                                                                      │
│  ┌────────────────────────────────────────────────────────────────┐ │
│  │                  Internal Event Bus                             │ │
│  │  tokio::sync::broadcast — All modules pub/sub                  │ │
│  │  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────────────┐  │ │
│  │  │ Brain    │ │ Scheduler│ │ Worker   │ │ Deploy/Git/MCP.. │  │ │
│  │  │ Engine   │ │          │ │ Manager  │ │                  │  │ │
│  │  └────┬─────┘ └────┬─────┘ └────┬─────┘ └────────┬─────────┘  │ │
│  │       └─────────────┴────────────┴────────────────┘            │ │
│  │                          │                                      │ │
│  │                 publish / subscribe                              │ │
│  └──────────────────────────┬─────────────────────────────────────┘ │
│                              │                                       │
│  ┌──────────────────────────▼─────────────────────────────────────┐ │
│  │                    Web API Layer (axum)                         │ │
│  │                                                                 │ │
│  │  ┌────────────────────┐  ┌──────────────────────────────────┐  │ │
│  │  │  REST API           │  │  WebSocket API                    │  │ │
│  │  │  /api/v1/*          │  │  /ws                              │  │ │
│  │  │                     │  │                                    │  │ │
│  │  │  Projects CRUD      │  │  Real-time event stream           │  │ │
│  │  │  Tasks CRUD/query   │  │  Task progress                    │  │ │
│  │  │  Workers status     │  │  Worker status                    │  │ │
│  │  │  Deploy trigger     │  │  Deploy stages                    │  │ │
│  │  │  Config get/set     │  │  Notifications                    │  │ │
│  │  │  LLM usage stats   │  │  Chat messages                    │  │ │
│  │  │  Memory search      │  │                                    │  │ │
│  │  │  Audit log query    │  │  Subscriptions:                    │  │ │
│  │  │  System health      │  │    by project / by event type      │  │ │
│  │  └────────────────────┘  └──────────────────────────────────┘  │ │
│  └────────────────────────────────────────────────────────────────┘ │
│                                                                      │
│  ┌────────────────────────────────────────────────────────────────┐ │
│  │  Auth Middleware: JWT tokens from web_sessions table            │ │
│  │  Rate Limiter: per-identity request throttle                    │ │
│  │  Audit Logger: all API calls logged                             │ │
│  └────────────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────────┘
```

---

## 2. Internal Event Bus API

The event bus is the nervous system of Argus. All inter-module communication flows through it.

### 2.1 Core Types

```rust
use tokio::sync::broadcast;
use std::time::Duration;
use uuid::Uuid;
use chrono::{DateTime, Utc};

/// Unique identifier for events (for dedup, correlation, audit)
#[derive(Clone, Debug)]
struct EventEnvelope {
    id: Uuid,
    timestamp: DateTime<Utc>,
    source: EventSource,
    event: ArgusEvent,
    /// Optional correlation ID to link related events
    correlation_id: Option<Uuid>,
}

#[derive(Clone, Debug)]
enum EventSource {
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
```

### 2.2 Full ArgusEvent Enum

All events that flow through the bus. Every variant carries enough data for any subscriber to act without needing to query back.

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
enum ArgusEvent {
    // ==========================================
    // Project Lifecycle
    // ==========================================
    ProjectCreated {
        project_id: Uuid,
        name: String,
        git_repo_url: Option<String>,
        language: Option<String>,
    },
    ProjectUpdated {
        project_id: Uuid,
        changes: serde_json::Value,  // diff of changed fields
    },
    ProjectPaused { project_id: Uuid },
    ProjectResumed { project_id: Uuid },
    ProjectDeleted { project_id: Uuid },

    // ==========================================
    // Milestone Lifecycle
    // ==========================================
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

    // ==========================================
    // Task Lifecycle
    // ==========================================
    TaskCreated {
        task_id: Uuid,
        project_id: Uuid,
        title: String,
        task_type: String,
        priority: TaskPriority,
    },
    TaskQueued { task_id: Uuid },
    TaskReady { task_id: Uuid },
    TaskAssigned {
        task_id: Uuid,
        worker_id: Uuid,
        worker_type: WorkerType,
    },
    TaskStarted { task_id: Uuid },
    TaskProgress {
        task_id: Uuid,
        progress: f32,     // 0.0 to 1.0
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

    // ==========================================
    // Worker
    // ==========================================
    WorkerSpawned {
        worker_id: Uuid,
        worker_type: WorkerType,
        sandbox_type: String,
    },
    WorkerReady { worker_id: Uuid },
    WorkerBusy {
        worker_id: Uuid,
        task_id: Uuid,
    },
    WorkerIdle { worker_id: Uuid },
    WorkerCrashed {
        worker_id: Uuid,
        error: String,
        task_id: Option<Uuid>,  // task it was working on, if any
    },
    WorkerRecovered { worker_id: Uuid },
    WorkerShuttingDown { worker_id: Uuid },
    WorkerResourceAlert {
        worker_id: Uuid,
        resource: String,      // memory, cpu, disk
        current: f64,
        limit: f64,
    },

    // ==========================================
    // Git
    // ==========================================
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

    // ==========================================
    // Deployment
    // ==========================================
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
        stage: String,           // build, test, package, pre_check, deploy, health_check
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
        approved_by: Uuid,       // identity_id
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

    // ==========================================
    // LLM
    // ==========================================
    LlmRequestSent {
        provider: String,
        model: String,
        model_class: String,
        estimated_tokens: u32,
        purpose: String,
    },
    LlmResponseReceived {
        provider: String,
        model: String,
        input_tokens: u32,
        output_tokens: u32,
        cost_usd: f64,
        latency_ms: u64,
    },
    LlmRequestFailed {
        provider: String,
        model: String,
        error: String,
        will_failover: bool,
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

    // ==========================================
    // Social / Communication
    // ==========================================
    MessageReceived {
        platform: String,
        sender_id: Uuid,
        content_type: String,    // text, command, file, image
        content_preview: String, // first 100 chars
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

    // ==========================================
    // Memory
    // ==========================================
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
        reason: String,         // decay, explicit_delete, ttl
    },
    MemoryDecayRun {
        affected_count: u32,
        deleted_count: u32,
        promoted_count: u32,
    },

    // ==========================================
    // MCP / Skills
    // ==========================================
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
        verdict: String,           // safe, suspicious, dangerous
        trust_level_assigned: String,
    },
    SkillInstalled {
        name: String,
        skill_type: String,
        trust_level: String,
    },
    SkillRemoved { name: String },
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

    // ==========================================
    // Config
    // ==========================================
    ConfigChanged {
        domain: String,
        key: String,
        previous_value: Option<serde_json::Value>,
        new_value: serde_json::Value,
        source: String,            // conversation, api, auto
    },
    ConfigUndone {
        domain: String,
        key: String,
        restored_value: serde_json::Value,
    },

    // ==========================================
    // Security
    // ==========================================
    SecurityAuditCompleted {
        target: String,
        result: String,            // pass, warn, fail
        details: serde_json::Value,
    },
    DangerousOperationBlocked {
        command: String,
        severity: String,          // critical, high, medium
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
        action: String,            // redacted, blocked
    },
    RateLimitExceeded {
        identity_id: Uuid,
        endpoint: String,
    },

    // ==========================================
    // Self-Evolution
    // ==========================================
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
        strategy: String,         // hot_reload, graceful_restart, blue_green, schema_migration
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

    // ==========================================
    // System
    // ==========================================
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
        job_name: String,         // memory_decay, usage_refresh, cleanup, backup
        duration_ms: u64,
        result: String,
    },

    // === Collaboration ===
    CollaborationStarted {
        session_id: Uuid,
        task_id: Uuid,
        mode: String,             // Solo, PairReview, FullTeam
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
```

### 2.3 Event Bus Implementation

```rust
use tokio::sync::broadcast;
use std::sync::Arc;

const EVENT_BUS_CAPACITY: usize = 4096;

struct EventBus {
    sender: broadcast::Sender<EventEnvelope>,
}

impl EventBus {
    fn new() -> Self {
        let (sender, _) = broadcast::channel(EVENT_BUS_CAPACITY);
        Self { sender }
    }

    /// Publish an event to all subscribers.
    /// Returns the number of active receivers.
    fn publish(&self, source: EventSource, event: ArgusEvent) -> usize {
        self.publish_with_correlation(source, event, None)
    }

    /// Publish an event with a correlation ID to link related events.
    fn publish_with_correlation(
        &self,
        source: EventSource,
        event: ArgusEvent,
        correlation_id: Option<Uuid>,
    ) -> usize {
        let envelope = EventEnvelope {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            source,
            event,
            correlation_id,
        };

        // broadcast::send returns Err only if there are no receivers,
        // which is not an error condition during startup/shutdown
        self.sender.send(envelope).unwrap_or(0)
    }

    /// Subscribe to the event bus. Returns a receiver that yields all events.
    fn subscribe(&self) -> broadcast::Receiver<EventEnvelope> {
        self.sender.subscribe()
    }

    /// Get count of active subscribers.
    fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }
}
```

### 2.4 Filtered Subscription Helper

Modules typically only care about a subset of events. This helper filters at the subscriber level.

```rust
use tokio::sync::mpsc;

struct EventFilter {
    /// Only pass events matching these source types (empty = all)
    sources: Vec<EventSource>,
    /// Only pass events matching these discriminants
    event_types: Vec<String>,
    /// Only pass events related to these project IDs
    project_ids: Vec<Uuid>,
}

impl EventFilter {
    fn matches(&self, envelope: &EventEnvelope) -> bool {
        // Source filter
        if !self.sources.is_empty() {
            let source_matches = self.sources.iter().any(|s| {
                std::mem::discriminant(s) == std::mem::discriminant(&envelope.source)
            });
            if !source_matches {
                return false;
            }
        }

        // Event type filter (by variant name)
        if !self.event_types.is_empty() {
            let event_name = format!("{:?}", std::mem::discriminant(&envelope.event));
            if !self.event_types.iter().any(|t| event_name.contains(t)) {
                return false;
            }
        }

        // Project ID filter: extract project_id from event if present
        if !self.project_ids.is_empty() {
            if let Some(pid) = extract_project_id(&envelope.event) {
                if !self.project_ids.contains(&pid) {
                    return false;
                }
            }
        }

        true
    }
}

/// Spawn a filtered event forwarder task.
/// Events matching the filter are sent to the returned mpsc receiver.
async fn subscribe_filtered(
    bus: &EventBus,
    filter: EventFilter,
    buffer_size: usize,
) -> mpsc::Receiver<EventEnvelope> {
    let (tx, rx) = mpsc::channel(buffer_size);
    let mut broadcast_rx = bus.subscribe();

    tokio::spawn(async move {
        loop {
            match broadcast_rx.recv().await {
                Ok(envelope) => {
                    if filter.matches(&envelope) {
                        if tx.send(envelope).await.is_err() {
                            break; // receiver dropped
                        }
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("Event subscriber lagged, missed {n} events");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    rx
}
```

### 2.5 Event Persistence (Audit Integration)

All events are persisted to the `audit_log` table for observability and debugging. A dedicated subscriber handles this.

```rust
async fn start_event_logger(bus: &EventBus, pool: &PgPool) {
    let mut rx = bus.subscribe();
    let pool = pool.clone();
    let mut batch: Vec<EventEnvelope> = Vec::with_capacity(100);
    let mut flush_interval = tokio::time::interval(Duration::from_secs(1));

    tokio::spawn(async move {
        loop {
            tokio::select! {
                result = rx.recv() => {
                    match result {
                        Ok(envelope) => {
                            // Security-critical events flush immediately
                            if is_security_critical(&envelope.event) {
                                flush_single(&pool, &envelope).await;
                            } else {
                                batch.push(envelope);
                                if batch.len() >= 100 {
                                    flush_batch(&pool, &mut batch).await;
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            tracing::error!("Audit logger lagged by {n} events");
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                _ = flush_interval.tick() => {
                    if !batch.is_empty() {
                        flush_batch(&pool, &mut batch).await;
                    }
                }
            }
        }
    });
}

fn is_security_critical(event: &ArgusEvent) -> bool {
    matches!(
        event,
        ArgusEvent::DangerousOperationBlocked { .. }
            | ArgusEvent::UnauthorizedAccessAttempt { .. }
            | ArgusEvent::SecretDetected { .. }
            | ArgusEvent::EvolutionGuardrailTriggered { .. }
            | ArgusEvent::RateLimitExceeded { .. }
    )
}
```

### 2.6 Module Subscription Map

Which modules subscribe to which events:

| Event Category | Subscribers |
|---------------|-------------|
| `Project*` | Scheduler, Memory, Social, Monitor, WebApi |
| `Task*` | Brain, WorkerManager, Memory, Social, Monitor, DeployEngine, WebApi |
| `Worker*` | Scheduler, Monitor, WebApi |
| `Git*` | DeployEngine (auto-deploy on push), Social, Memory, WebApi |
| `Deploy*` | Social (notifications), Monitor, Memory, WebApi |
| `Llm*` | Monitor, Social (budget alerts), WebApi |
| `Message*` | Brain (intent classification), ConversationalConfig, WebApi |
| `Memory*` | Monitor, WebApi |
| `Mcp*`/`Skill*` | Security, Monitor, WebApi |
| `Config*` | All affected modules (hot reload), WebApi |
| `Security*` | Social (alert owner), Monitor, WebApi |
| `Evolution*` | Social, Monitor, Security, WebApi |
| `Collaboration*` | Brain, WorkerManager, Social, Monitor, WebApi |
| `System*` | Social, Monitor, WebApi |

### 2.7 Performance Constraints

| Metric | Target |
|--------|--------|
| Publish latency | < 1ms (non-blocking broadcast) |
| Bus capacity | 4096 events in-flight |
| Batch flush interval | 1 second (audit persistence) |
| Max subscribers | ~20 (one per module + WebApi bridge) |
| Event size (average) | ~500 bytes serialized |

---

## 3. Web Dashboard REST API

The Web Dashboard is served by `axum` and provides full administrative control over Argus. All endpoints require authentication.

### 3.1 Authentication

```
POST /api/v1/auth/login
POST /api/v1/auth/logout
GET  /api/v1/auth/me
POST /api/v1/auth/refresh
```

#### Login

```
POST /api/v1/auth/login
Content-Type: application/json

{
    "username": "owner",
    "password": "..."
}

Response 200:
{
    "token": "eyJ...",
    "expires_at": "2026-02-11T12:00:00Z",
    "identity": {
        "id": "uuid",
        "name": "owner",
        "role": "owner"
    }
}
```

All subsequent requests include:
```
Authorization: Bearer <token>
```

JWT tokens are validated against the `web_sessions` table. Token payload:

```rust
struct JwtClaims {
    sub: Uuid,              // identity_id
    role: Role,
    session_id: Uuid,
    exp: i64,               // expiration timestamp
    iat: i64,               // issued at
}
```

### 3.2 Common Patterns

All responses follow a consistent envelope:

```json
// Success
{
    "data": { ... },
    "meta": {
        "request_id": "uuid",
        "timestamp": "2026-02-10T12:00:00Z"
    }
}

// Success (list)
{
    "data": [ ... ],
    "meta": {
        "request_id": "uuid",
        "timestamp": "2026-02-10T12:00:00Z",
        "pagination": {
            "page": 1,
            "per_page": 50,
            "total": 234,
            "total_pages": 5
        }
    }
}

// Error
{
    "error": {
        "code": "NOT_FOUND",
        "message": "Project not found",
        "details": null
    },
    "meta": {
        "request_id": "uuid",
        "timestamp": "2026-02-10T12:00:00Z"
    }
}
```

Pagination query params (all list endpoints):
- `page` — Page number (default 1)
- `per_page` — Items per page (default 50, max 200)
- `sort` — Sort field (default varies per endpoint)
- `order` — `asc` or `desc` (default `desc`)

### 3.3 Error Codes

| HTTP Status | Code | Description |
|------------|------|-------------|
| 400 | `BAD_REQUEST` | Invalid request body or parameters |
| 401 | `UNAUTHORIZED` | Missing or invalid token |
| 403 | `FORBIDDEN` | Insufficient permissions (RBAC) |
| 404 | `NOT_FOUND` | Resource not found |
| 409 | `CONFLICT` | Resource conflict (e.g., duplicate name) |
| 422 | `VALIDATION_ERROR` | Request validated but semantically invalid |
| 429 | `RATE_LIMITED` | Too many requests |
| 500 | `INTERNAL_ERROR` | Server error |

### 3.4 Projects

```
GET    /api/v1/projects                  — List projects
POST   /api/v1/projects                  — Create project
GET    /api/v1/projects/:id              — Get project
PUT    /api/v1/projects/:id              — Update project
DELETE /api/v1/projects/:id              — Delete project
POST   /api/v1/projects/:id/pause        — Pause project
POST   /api/v1/projects/:id/resume       — Resume project
GET    /api/v1/projects/:id/milestones   — List milestones
GET    /api/v1/projects/:id/stats        — Project statistics
```

#### List Projects

```
GET /api/v1/projects?active=true&sort=updated_at&order=desc

Response 200:
{
    "data": [
        {
            "id": "uuid",
            "name": "my-web-app",
            "description": "A Next.js web application",
            "git_repo_url": "https://github.com/owner/my-web-app",
            "git_local_path": "/var/argus/workspaces/my-web-app",
            "default_branch": "main",
            "language": "typescript",
            "framework": "next",
            "active": true,
            "priority": "high",
            "created_at": "2026-01-15T10:00:00Z",
            "updated_at": "2026-02-10T08:30:00Z",
            "stats": {
                "total_tasks": 47,
                "completed_tasks": 35,
                "active_tasks": 5,
                "failed_tasks": 2,
                "total_deployments": 12,
                "last_deploy_at": "2026-02-09T20:00:00Z"
            }
        }
    ]
}
```

#### Create Project

```
POST /api/v1/projects
Content-Type: application/json

{
    "name": "my-web-app",
    "description": "A Next.js web application",
    "git_repo_url": "https://github.com/owner/my-web-app",
    "language": "typescript",
    "framework": "next",
    "build_command": "npm run build",
    "test_command": "npm test",
    "priority": "high"
}

Response 201:
{
    "data": {
        "id": "uuid",
        "name": "my-web-app",
        ...
    }
}
```

#### Project Statistics

```
GET /api/v1/projects/:id/stats

Response 200:
{
    "data": {
        "tasks": {
            "total": 47,
            "by_state": {
                "pending": 5,
                "ready": 3,
                "running": 2,
                "completed": 35,
                "failed": 2
            }
        },
        "deployments": {
            "total": 12,
            "last_successful": "2026-02-09T20:00:00Z",
            "success_rate": 0.917
        },
        "llm_usage": {
            "total_tokens": 1250000,
            "total_cost_usd": 15.75,
            "by_provider": {
                "anthropic": { "tokens": 800000, "cost_usd": 12.00 },
                "deepseek": { "tokens": 450000, "cost_usd": 3.75 }
            }
        },
        "git": {
            "total_commits": 89,
            "total_prs": 15,
            "last_commit_at": "2026-02-10T08:30:00Z"
        }
    }
}
```

### 3.5 Tasks

```
GET    /api/v1/tasks                     — List/query tasks (global)
POST   /api/v1/tasks                     — Create task
GET    /api/v1/tasks/:id                 — Get task detail
PUT    /api/v1/tasks/:id                 — Update task
DELETE /api/v1/tasks/:id                 — Cancel task
POST   /api/v1/tasks/:id/retry          — Retry failed task
GET    /api/v1/tasks/:id/episodes        — Get execution episodes
GET    /api/v1/tasks/:id/checkpoints     — Get checkpoints

GET    /api/v1/projects/:id/tasks        — List tasks for project
```

#### Query Tasks

```
GET /api/v1/tasks?project_id=uuid&state=running,ready&priority=high,critical&sort=effective_priority&order=desc

Response 200:
{
    "data": [
        {
            "id": "uuid",
            "project_id": "uuid",
            "milestone_id": "uuid",
            "title": "Implement user authentication",
            "description": "Add JWT-based auth with login/register endpoints",
            "task_type": "code",
            "state": "running",
            "priority": "high",
            "effective_priority": 8.5,
            "assigned_worker": {
                "id": "uuid",
                "type": "opencode",
                "name": "opencode-1"
            },
            "retry_count": 0,
            "max_retries": 3,
            "timeout_seconds": 3600,
            "branch": "feat/auth",
            "dependencies": ["uuid1", "uuid2"],
            "dependents": ["uuid3"],
            "created_at": "2026-02-10T08:00:00Z",
            "started_at": "2026-02-10T08:05:00Z"
        }
    ]
}
```

#### Task Detail (includes full output/error)

```
GET /api/v1/tasks/:id

Response 200:
{
    "data": {
        "id": "uuid",
        ...all fields...,
        "output": { ... },
        "error": null,
        "context": {
            "files": ["src/auth/mod.rs", "src/auth/jwt.rs"],
            "instructions": "..."
        },
        "episodes": [
            {
                "id": "uuid",
                "attempt": 1,
                "worker_id": "uuid",
                "status": "completed",
                "started_at": "...",
                "completed_at": "...",
                "tokens_used": 15000,
                "cost_usd": 0.45
            }
        ]
    }
}
```

### 3.6 Workers

```
GET    /api/v1/workers                   — List all workers
GET    /api/v1/workers/:id               — Get worker detail
POST   /api/v1/workers/:id/kill          — Kill a worker
POST   /api/v1/workers/spawn             — Spawn a new worker
GET    /api/v1/workers/stats             — Worker pool statistics
```

#### List Workers

```
GET /api/v1/workers

Response 200:
{
    "data": [
        {
            "id": "uuid",
            "worker_type": "opencode",
            "name": "opencode-1",
            "state": "busy",
            "current_task": {
                "id": "uuid",
                "title": "Implement user authentication"
            },
            "sandbox_type": "docker",
            "pid": 12345,
            "resources": {
                "memory_mb": 512,
                "cpu_percent": 15.3,
                "memory_limit_mb": 2048
            },
            "stats": {
                "tasks_completed": 23,
                "tasks_failed": 1,
                "uptime_seconds": 86400
            },
            "last_heartbeat": "2026-02-10T12:00:00Z"
        }
    ]
}
```

#### Worker Pool Stats

```
GET /api/v1/workers/stats

Response 200:
{
    "data": {
        "total": 5,
        "by_state": {
            "idle": 2,
            "busy": 3,
            "crashed": 0
        },
        "by_type": {
            "opencode": 3,
            "claude_code": 1,
            "aider": 1
        },
        "resource_usage": {
            "total_memory_mb": 2560,
            "total_cpu_percent": 45.2,
            "memory_limit_mb": 10240
        }
    }
}
```

### 3.7 Deployments

```
GET    /api/v1/deployments                      — List all deployments
POST   /api/v1/deployments                      — Trigger deployment
GET    /api/v1/deployments/:id                  — Get deployment detail
POST   /api/v1/deployments/:id/approve          — Approve pending deployment
POST   /api/v1/deployments/:id/reject           — Reject pending deployment
POST   /api/v1/deployments/:id/rollback         — Rollback deployment
GET    /api/v1/deployments/:id/logs             — Get deploy stage logs

GET    /api/v1/projects/:id/deployments          — List deployments for project
GET    /api/v1/projects/:id/environments         — List environment configs
PUT    /api/v1/projects/:id/environments/:env    — Update environment config
```

#### Trigger Deployment

```
POST /api/v1/deployments
Content-Type: application/json

{
    "project_id": "uuid",
    "environment": "production",
    "git_ref": "main",
    "skip_tests": false
}

Response 202:
{
    "data": {
        "id": "uuid",
        "project_id": "uuid",
        "environment": "production",
        "status": "queued",
        "created_at": "2026-02-10T12:00:00Z"
    }
}
```

#### Deployment Detail

```
GET /api/v1/deployments/:id

Response 200:
{
    "data": {
        "id": "uuid",
        "project_id": "uuid",
        "environment": "production",
        "target_type": "docker",
        "status": "succeeded",
        "git_ref": "abc123",
        "git_branch": "main",
        "stages": [
            {
                "name": "build",
                "status": "completed",
                "started_at": "2026-02-10T12:00:05Z",
                "completed_at": "2026-02-10T12:01:30Z",
                "duration_seconds": 85,
                "logs_available": true
            },
            {
                "name": "test",
                "status": "completed",
                "started_at": "2026-02-10T12:01:31Z",
                "completed_at": "2026-02-10T12:03:00Z",
                "duration_seconds": 89,
                "logs_available": true
            },
            {
                "name": "deploy",
                "status": "completed",
                "started_at": "2026-02-10T12:03:01Z",
                "completed_at": "2026-02-10T12:03:15Z",
                "duration_seconds": 14,
                "logs_available": true
            },
            {
                "name": "health_check",
                "status": "completed",
                "started_at": "2026-02-10T12:03:16Z",
                "completed_at": "2026-02-10T12:03:46Z",
                "duration_seconds": 30,
                "logs_available": true
            }
        ],
        "snapshot_id": "uuid",
        "created_at": "2026-02-10T12:00:00Z",
        "finished_at": "2026-02-10T12:03:46Z",
        "total_duration_seconds": 226
    }
}
```

### 3.8 Configuration

```
GET    /api/v1/config                    — Get all config (tree structure)
GET    /api/v1/config/:domain            — Get config for domain
PUT    /api/v1/config/:domain/:key       — Set config value
DELETE /api/v1/config/:domain/:key       — Delete config value
POST   /api/v1/config/undo              — Undo last config change
GET    /api/v1/config/audit              — Config change history
POST   /api/v1/config/export            — Export all config as TOML
POST   /api/v1/config/import            — Import config from TOML
```

#### Get Config Tree

```
GET /api/v1/config

Response 200:
{
    "data": {
        "llm": {
            "default_provider": "anthropic",
            "default_model": "claude-sonnet-4-20250514",
            "monthly_budget_usd": 100.0
        },
        "memory": {
            "short_term_ttl_hours": 72,
            "decay_rate": 0.01,
            "promotion_threshold": 0.7
        },
        "worker": {
            "max_pool_size": 8,
            "default_sandbox": "docker",
            "idle_timeout_seconds": 300
        },
        "notification": {
            "disturbance_level": "normal",
            "active_hours_start": "08:00",
            "active_hours_end": "23:00"
        },
        ...
    }
}
```

#### Set Config Value

```
PUT /api/v1/config/worker/max_pool_size
Content-Type: application/json

{
    "value": 10
}

Response 200:
{
    "data": {
        "domain": "worker",
        "key": "max_pool_size",
        "previous_value": 8,
        "new_value": 10,
        "requires_restart": false
    }
}
```

### 3.9 LLM Usage

```
GET    /api/v1/llm/usage                — Usage summary
GET    /api/v1/llm/usage/daily          — Daily breakdown
GET    /api/v1/llm/providers            — Provider status
GET    /api/v1/llm/budget               — Budget status
```

#### Usage Summary

```
GET /api/v1/llm/usage?period=30d

Response 200:
{
    "data": {
        "period": "30d",
        "total_requests": 4523,
        "total_input_tokens": 12500000,
        "total_output_tokens": 3200000,
        "total_cost_usd": 127.50,
        "by_provider": [
            {
                "provider": "anthropic",
                "requests": 2100,
                "input_tokens": 6000000,
                "output_tokens": 1800000,
                "cost_usd": 84.00,
                "avg_latency_ms": 1200,
                "success_rate": 0.995
            },
            {
                "provider": "deepseek",
                "requests": 1800,
                "input_tokens": 5000000,
                "output_tokens": 1200000,
                "cost_usd": 38.50,
                "avg_latency_ms": 800,
                "success_rate": 0.988
            }
        ],
        "by_model_class": {
            "reasoning": { "requests": 500, "cost_usd": 45.00 },
            "coding": { "requests": 2500, "cost_usd": 65.00 },
            "fast": { "requests": 1200, "cost_usd": 12.50 },
            "long_context": { "requests": 323, "cost_usd": 5.00 }
        },
        "by_project": [
            {
                "project_id": "uuid",
                "project_name": "my-web-app",
                "cost_usd": 45.00,
                "tokens": 5200000
            }
        ],
        "savings": {
            "local_model_savings_usd": 15.00,
            "cache_hit_savings_usd": 8.50,
            "context_compression_savings_usd": 12.00,
            "total_savings_usd": 35.50
        }
    }
}
```

#### Provider Status

```
GET /api/v1/llm/providers

Response 200:
{
    "data": [
        {
            "name": "anthropic",
            "display_name": "Anthropic",
            "enabled": true,
            "priority": 10,
            "status": "healthy",
            "models": ["claude-sonnet-4-20250514", "claude-haiku-35"],
            "keys": {
                "total": 3,
                "active_index": 0,
                "exhausted": []
            },
            "budget": {
                "monthly_limit_usd": 50.00,
                "current_usage_usd": 28.50,
                "remaining_percent": 43.0
            },
            "rate_limits": {
                "rpm_limit": 60,
                "tpm_limit": 100000,
                "current_rpm": 12,
                "current_tpm": 25000
            }
        }
    ]
}
```

### 3.10 Memory

```
POST   /api/v1/memory/search            — Semantic search
GET    /api/v1/memory/:id               — Get memory by ID
DELETE /api/v1/memory/:id               — Delete memory
GET    /api/v1/memory/stats             — Memory statistics
POST   /api/v1/memory/store             — Manually store a memory
```

#### Semantic Search

```
POST /api/v1/memory/search
Content-Type: application/json

{
    "query": "how does the authentication system work",
    "project_id": "uuid",
    "layers": ["long_term", "permanent"],
    "memory_types": ["decision", "experience"],
    "limit": 10,
    "min_similarity": 0.5
}

Response 200:
{
    "data": {
        "results": [
            {
                "id": "uuid",
                "layer": "long_term",
                "memory_type": "decision",
                "content": "Decided to use JWT with RS256 for auth...",
                "summary": "Auth uses JWT RS256",
                "similarity": 0.87,
                "importance": 0.8,
                "project_id": "uuid",
                "tags": ["auth", "jwt", "security"],
                "created_at": "2026-01-20T10:00:00Z",
                "last_accessed": "2026-02-08T15:00:00Z"
            }
        ],
        "query_embedding_time_ms": 45,
        "search_time_ms": 12
    }
}
```

#### Memory Statistics

```
GET /api/v1/memory/stats

Response 200:
{
    "data": {
        "total_memories": 1523,
        "by_layer": {
            "working": 15,
            "short_term": 234,
            "long_term": 1089,
            "permanent": 185
        },
        "by_type": {
            "fact": 312,
            "decision": 189,
            "experience": 445,
            "preference": 78,
            "error_pattern": 156,
            "skill_learned": 45,
            "project_context": 203,
            "conversation_summary": 67,
            "safety_rule": 28
        },
        "embedding_model": "BGE-M3",
        "embedding_dimensions": 1024,
        "index_type": "HNSW",
        "last_decay_run": "2026-02-10T03:00:00Z",
        "last_consolidation_run": "2026-02-10T04:00:00Z"
    }
}
```

### 3.11 Audit Log

```
GET    /api/v1/audit                     — Query audit log
GET    /api/v1/audit/stats               — Audit statistics
```

#### Query Audit Log

```
GET /api/v1/audit?action=deploy&outcome=failure&from=2026-02-01&to=2026-02-10&per_page=50

Response 200:
{
    "data": [
        {
            "id": "uuid",
            "timestamp": "2026-02-09T14:30:00Z",
            "actor": {
                "type": "system",
                "module": "deploy_engine"
            },
            "action": {
                "type": "deploy_execute",
                "target": "my-web-app",
                "environment": "production"
            },
            "resource": {
                "type": "deployment",
                "id": "uuid"
            },
            "outcome": "failure",
            "details": {
                "stage": "health_check",
                "error": "HTTP probe returned 503"
            }
        }
    ]
}
```

### 3.12 System Health

```
GET    /api/v1/system/health             — System health report
GET    /api/v1/system/version            — Version info
POST   /api/v1/system/shutdown           — Graceful shutdown (owner only)
POST   /api/v1/system/backup             — Trigger manual backup
GET    /api/v1/system/backups            — List backups
```

#### System Health

```
GET /api/v1/system/health

Response 200:
{
    "data": {
        "status": "healthy",
        "version": "0.1.0",
        "uptime_seconds": 345600,
        "started_at": "2026-02-06T12:00:00Z",
        "resources": {
            "cpu_percent": 23.5,
            "memory_used_mb": 1024,
            "memory_total_mb": 8192,
            "disk_used_percent": 45.2,
            "disk_used_gb": 90.4,
            "disk_total_gb": 200.0
        },
        "modules": {
            "brain": "healthy",
            "scheduler": "healthy",
            "worker_manager": "healthy",
            "llm_provider": "healthy",
            "memory_engine": "healthy",
            "mcp_client": "healthy",
            "social_gateway": "healthy",
            "deploy_engine": "healthy",
            "git_ops": "healthy",
            "security": "healthy",
            "monitor": "healthy"
        },
        "database": {
            "status": "connected",
            "pool_size": 10,
            "active_connections": 3,
            "latency_ms": 2
        },
        "workers": {
            "total": 5,
            "idle": 2,
            "busy": 3,
            "crashed": 0
        },
        "tasks": {
            "running": 3,
            "queued": 7,
            "completed_today": 45,
            "failed_today": 1
        },
        "social_platforms": {
            "telegram": "connected",
            "discord": "connected",
            "wechat": "disconnected",
            "slack": "not_configured",
            "web_dashboard": "connected"
        }
    }
}
```

### 3.13 Chat (Conversational Interface via Web Dashboard)

```
POST   /api/v1/chat/send                — Send a message to Argus
GET    /api/v1/chat/history              — Get chat history
```

#### Send Message

This is the web equivalent of sending a message via Telegram/Discord. The message goes through the same Intent Classifier → Brain Engine pipeline.

```
POST /api/v1/chat/send
Content-Type: application/json

{
    "content": "帮我新建一个项目，用Next.js做前端",
    "thread_id": "uuid"       // optional, for multi-turn
}

Response 200:
{
    "data": {
        "message_id": "uuid",
        "response": "好的，我来帮你创建一个 Next.js 项目。请问项目叫什么名字？",
        "dialog_state": "awaiting_value",
        "thread_id": "uuid"
    }
}
```

### 3.14 MCP & Skills

```
GET    /api/v1/mcp/servers               — List MCP servers
POST   /api/v1/mcp/servers               — Install MCP server
DELETE /api/v1/mcp/servers/:name          — Remove MCP server
POST   /api/v1/mcp/servers/:name/audit    — Trigger security audit

GET    /api/v1/skills                     — List skills
POST   /api/v1/skills                     — Install skill
DELETE /api/v1/skills/:name               — Remove skill
POST   /api/v1/skills/:name/execute       — Execute skill manually
```

### 3.15 Self-Evolution

```
GET    /api/v1/evolution/history          — List evolution records
GET    /api/v1/evolution/:id              — Get evolution detail
POST   /api/v1/evolution                  — Request new evolution
POST   /api/v1/evolution/:id/approve      — Approve pending evolution
POST   /api/v1/evolution/:id/reject       — Reject pending evolution
```

### 3.16 Collaborations

```
GET    /api/v1/collaborations                — List collaboration sessions (filterable by project, mode, phase)
GET    /api/v1/collaborations/:id            — Get collaboration session detail (phases, role assignments, artifacts)
GET    /api/v1/collaborations/stats          — Collaboration statistics (mode distribution, costs, review outcomes)
```

#### Response: `GET /api/v1/collaborations/:id`

```json
{
    "id": "session-uuid",
    "task_id": "task-uuid",
    "project_id": "project-uuid",
    "mode": "full_team",
    "phase": "review",
    "complexity_assessment": {
        "score": 0.72,
        "factors": ["SecuritySensitive", "DatabaseMigration", "PublicApiChange"],
        "risk_level": "high"
    },
    "review_policy": {
        "min_review_rounds": 2,
        "force_different_model": true,
        "require_security_audit": true,
        "require_owner_approval": false
    },
    "role_assignments": {
        "architect": { "worker_id": "w1-uuid", "worker_type": "opencode", "model": "claude-sonnet", "status": "completed" },
        "developer": { "worker_id": "w2-uuid", "worker_type": "claude_code", "model": "claude-sonnet", "status": "completed" },
        "reviewer": { "worker_id": "w3-uuid", "worker_type": "opencode", "model": "gpt-4o", "status": "in_progress" }
    },
    "review_rounds_completed": 1,
    "artifacts": [
        { "role": "architect", "type": "architecture_doc", "file": "architecture.md" },
        { "role": "developer", "type": "implementation_summary", "file": "implementation.md" },
        { "role": "reviewer", "type": "review_report", "file": "review-round-1.json" }
    ],
    "created_at": "2026-02-10T10:00:00Z",
    "updated_at": "2026-02-10T10:45:00Z"
}
```

### 3.17 API Routes Summary

| Method | Path | Permission | Description |
|--------|------|-----------|-------------|
| `POST` | `/api/v1/auth/login` | Public | Login |
| `POST` | `/api/v1/auth/logout` | Any | Logout |
| `GET` | `/api/v1/auth/me` | Any | Current identity |
| `POST` | `/api/v1/auth/refresh` | Any | Refresh token |
| `GET` | `/api/v1/projects` | Viewer+ | List projects |
| `POST` | `/api/v1/projects` | Admin+ | Create project |
| `GET` | `/api/v1/projects/:id` | Viewer+ | Get project |
| `PUT` | `/api/v1/projects/:id` | Admin+ | Update project |
| `DELETE` | `/api/v1/projects/:id` | Owner | Delete project |
| `POST` | `/api/v1/projects/:id/pause` | Admin+ | Pause project |
| `POST` | `/api/v1/projects/:id/resume` | Admin+ | Resume project |
| `GET` | `/api/v1/projects/:id/milestones` | Viewer+ | List milestones |
| `GET` | `/api/v1/projects/:id/stats` | Viewer+ | Project stats |
| `GET` | `/api/v1/projects/:id/tasks` | Viewer+ | Tasks for project |
| `GET` | `/api/v1/projects/:id/deployments` | Viewer+ | Deploys for project |
| `GET` | `/api/v1/projects/:id/environments` | Developer+ | Env configs |
| `PUT` | `/api/v1/projects/:id/environments/:env` | Admin+ | Update env config |
| `GET` | `/api/v1/tasks` | Viewer+ | Query tasks |
| `POST` | `/api/v1/tasks` | Developer+ | Create task |
| `GET` | `/api/v1/tasks/:id` | Viewer+ | Get task |
| `PUT` | `/api/v1/tasks/:id` | Developer+ | Update task |
| `DELETE` | `/api/v1/tasks/:id` | Developer+ | Cancel task |
| `POST` | `/api/v1/tasks/:id/retry` | Developer+ | Retry task |
| `GET` | `/api/v1/tasks/:id/episodes` | Viewer+ | Execution episodes |
| `GET` | `/api/v1/tasks/:id/checkpoints` | Viewer+ | Checkpoints |
| `GET` | `/api/v1/workers` | Viewer+ | List workers |
| `GET` | `/api/v1/workers/:id` | Viewer+ | Get worker |
| `POST` | `/api/v1/workers/:id/kill` | Admin+ | Kill worker |
| `POST` | `/api/v1/workers/spawn` | Admin+ | Spawn worker |
| `GET` | `/api/v1/workers/stats` | Viewer+ | Pool stats |
| `GET` | `/api/v1/deployments` | Viewer+ | List deployments |
| `POST` | `/api/v1/deployments` | Developer+ | Trigger deploy |
| `GET` | `/api/v1/deployments/:id` | Viewer+ | Deploy detail |
| `POST` | `/api/v1/deployments/:id/approve` | Admin+ | Approve deploy |
| `POST` | `/api/v1/deployments/:id/reject` | Admin+ | Reject deploy |
| `POST` | `/api/v1/deployments/:id/rollback` | Admin+ | Rollback deploy |
| `GET` | `/api/v1/deployments/:id/logs` | Viewer+ | Deploy logs |
| `GET` | `/api/v1/config` | Admin+ | Get all config |
| `GET` | `/api/v1/config/:domain` | Admin+ | Get domain config |
| `PUT` | `/api/v1/config/:domain/:key` | Admin+ | Set config |
| `DELETE` | `/api/v1/config/:domain/:key` | Owner | Delete config |
| `POST` | `/api/v1/config/undo` | Admin+ | Undo config change |
| `GET` | `/api/v1/config/audit` | Admin+ | Config history |
| `POST` | `/api/v1/config/export` | Admin+ | Export TOML |
| `POST` | `/api/v1/config/import` | Owner | Import TOML |
| `GET` | `/api/v1/llm/usage` | Viewer+ | Usage summary |
| `GET` | `/api/v1/llm/usage/daily` | Viewer+ | Daily breakdown |
| `GET` | `/api/v1/llm/providers` | Admin+ | Provider status |
| `GET` | `/api/v1/llm/budget` | Viewer+ | Budget status |
| `POST` | `/api/v1/memory/search` | Developer+ | Semantic search |
| `GET` | `/api/v1/memory/:id` | Developer+ | Get memory |
| `DELETE` | `/api/v1/memory/:id` | Admin+ | Delete memory |
| `GET` | `/api/v1/memory/stats` | Viewer+ | Memory stats |
| `POST` | `/api/v1/memory/store` | Admin+ | Store memory |
| `GET` | `/api/v1/audit` | Admin+ | Query audit log |
| `GET` | `/api/v1/audit/stats` | Admin+ | Audit stats |
| `GET` | `/api/v1/system/health` | Viewer+ | Health report |
| `GET` | `/api/v1/system/version` | Public | Version info |
| `POST` | `/api/v1/system/shutdown` | Owner | Shutdown |
| `POST` | `/api/v1/system/backup` | Admin+ | Trigger backup |
| `GET` | `/api/v1/system/backups` | Admin+ | List backups |
| `POST` | `/api/v1/chat/send` | Developer+ | Chat message |
| `GET` | `/api/v1/chat/history` | Developer+ | Chat history |
| `GET` | `/api/v1/mcp/servers` | Viewer+ | List MCP servers |
| `POST` | `/api/v1/mcp/servers` | Admin+ | Install server |
| `DELETE` | `/api/v1/mcp/servers/:name` | Admin+ | Remove server |
| `POST` | `/api/v1/mcp/servers/:name/audit` | Admin+ | Audit server |
| `GET` | `/api/v1/skills` | Viewer+ | List skills |
| `POST` | `/api/v1/skills` | Admin+ | Install skill |
| `DELETE` | `/api/v1/skills/:name` | Admin+ | Remove skill |
| `POST` | `/api/v1/skills/:name/execute` | Developer+ | Execute skill |
| `GET` | `/api/v1/evolution/history` | Viewer+ | Evolution history |
| `GET` | `/api/v1/evolution/:id` | Viewer+ | Evolution detail |
| `POST` | `/api/v1/evolution` | Admin+ | Request evolution |
| `POST` | `/api/v1/evolution/:id/approve` | Owner | Approve evolution |
| `POST` | `/api/v1/evolution/:id/reject` | Owner | Reject evolution |
| `GET` | `/api/v1/collaborations` | Viewer+ | List collaboration sessions |
| `GET` | `/api/v1/collaborations/:id` | Viewer+ | Get collaboration detail |
| `GET` | `/api/v1/collaborations/stats` | Viewer+ | Collaboration statistics |

---

## 4. WebSocket Real-Time API

The WebSocket API provides a persistent, bidirectional connection for the Web Dashboard to receive live updates from the event bus.

### 4.1 Connection

```
ws://localhost:3000/ws?token=<jwt_token>
```

Or with TLS:
```
wss://argus.example.com/ws?token=<jwt_token>
```

The JWT token is validated on connection upgrade. Unauthorized connections are rejected with HTTP 401.

### 4.2 Message Protocol

All WebSocket messages are JSON. Both client-to-server and server-to-client messages follow this format:

```rust
/// Client → Server
#[derive(Serialize, Deserialize)]
struct WsClientMessage {
    /// Message type
    msg_type: WsClientMessageType,
    /// Unique request ID for correlating responses
    request_id: Option<String>,
}

#[derive(Serialize, Deserialize)]
enum WsClientMessageType {
    /// Subscribe to event streams
    Subscribe(WsSubscription),
    /// Unsubscribe from event streams
    Unsubscribe(WsSubscription),
    /// Send a chat message (equivalent to POST /api/v1/chat/send)
    ChatMessage { content: String, thread_id: Option<String> },
    /// Approve/reject inline (convenience for deploy/evolution approvals)
    Approve { resource_type: String, resource_id: Uuid },
    Reject { resource_type: String, resource_id: Uuid, reason: String },
    /// Ping for keepalive
    Ping,
}

/// Server → Client
#[derive(Serialize, Deserialize)]
struct WsServerMessage {
    msg_type: WsServerMessageType,
    /// Correlation to client request_id, if applicable
    request_id: Option<String>,
    timestamp: DateTime<Utc>,
}

#[derive(Serialize, Deserialize)]
enum WsServerMessageType {
    /// Acknowledgment of subscribe/unsubscribe
    Ack { success: bool, message: String },
    /// Live event from the event bus
    Event(WsEvent),
    /// Chat response
    ChatResponse {
        message_id: String,
        response: String,
        dialog_state: Option<String>,
        thread_id: String,
    },
    /// Notification (formatted for display)
    Notification {
        id: String,
        priority: String,
        title: String,
        body: String,
        actions: Vec<WsNotificationAction>,
    },
    /// Error
    Error { code: String, message: String },
    /// Pong
    Pong,
}
```

### 4.3 Subscription Model

Clients subscribe to event streams filtered by category and/or project. By default, no events are streamed (client must explicitly subscribe).

```rust
#[derive(Serialize, Deserialize)]
struct WsSubscription {
    /// Event categories to subscribe to (empty = all)
    categories: Vec<WsEventCategory>,
    /// Filter by project IDs (empty = all projects)
    project_ids: Vec<Uuid>,
}

#[derive(Serialize, Deserialize)]
enum WsEventCategory {
    Tasks,
    Workers,
    Deployments,
    Git,
    Llm,
    Memory,
    McpSkills,
    Config,
    Security,
    Evolution,
    Collaborations,
    System,
    Chat,
    All,
}
```

#### Subscribe Example

Client sends:
```json
{
    "msg_type": {
        "Subscribe": {
            "categories": ["Tasks", "Workers", "Deployments"],
            "project_ids": ["project-uuid-1"]
        }
    },
    "request_id": "sub-1"
}
```

Server responds:
```json
{
    "msg_type": {
        "Ack": {
            "success": true,
            "message": "Subscribed to Tasks, Workers, Deployments for 1 project(s)"
        }
    },
    "request_id": "sub-1",
    "timestamp": "2026-02-10T12:00:00Z"
}
```

### 4.4 Live Event Format

Events from the event bus are transformed into a dashboard-friendly format:

```rust
#[derive(Serialize, Deserialize)]
struct WsEvent {
    /// Event category for client-side routing
    category: WsEventCategory,
    /// Specific event type (e.g. "TaskStarted", "DeploySucceeded")
    event_type: String,
    /// Event data (varies by type)
    data: serde_json::Value,
    /// Related project ID, if applicable
    project_id: Option<Uuid>,
    /// Related task ID, if applicable
    task_id: Option<Uuid>,
    /// Severity for UI display
    severity: WsEventSeverity,
}

#[derive(Serialize, Deserialize)]
enum WsEventSeverity {
    Info,
    Success,
    Warning,
    Error,
    Critical,
}
```

#### Event Severity Mapping

| Event | Severity |
|-------|----------|
| `TaskCreated`, `TaskQueued`, `TaskStarted`, `TaskProgress` | Info |
| `TaskCompleted`, `DeploySucceeded`, `HealthCheckPassed` | Success |
| `TaskRetrying`, `LlmBudgetWarning`, `WorkerResourceAlert` | Warning |
| `TaskFailed`, `DeployFailed`, `HealthCheckFailed`, `LlmProviderDown` | Error |
| `DangerousOperationBlocked`, `UnauthorizedAccessAttempt`, `EvolutionGuardrailTriggered` | Critical |

### 4.5 Notification Actions

Notifications can include inline actions for quick responses:

```rust
#[derive(Serialize, Deserialize)]
struct WsNotificationAction {
    label: String,
    action_type: String,    // "approve", "reject", "view", "retry"
    resource_type: String,  // "deployment", "evolution", "task"
    resource_id: Uuid,
}
```

Example notification with actions:

```json
{
    "msg_type": {
        "Notification": {
            "id": "notif-uuid",
            "priority": "high",
            "title": "部署等待审批",
            "body": "项目 my-web-app 的生产环境部署需要你的审批",
            "actions": [
                {
                    "label": "批准",
                    "action_type": "approve",
                    "resource_type": "deployment",
                    "resource_id": "deploy-uuid"
                },
                {
                    "label": "拒绝",
                    "action_type": "reject",
                    "resource_type": "deployment",
                    "resource_id": "deploy-uuid"
                },
                {
                    "label": "查看详情",
                    "action_type": "view",
                    "resource_type": "deployment",
                    "resource_id": "deploy-uuid"
                }
            ]
        }
    },
    "timestamp": "2026-02-10T12:00:00Z"
}
```

### 4.6 Event Bus → WebSocket Bridge

The bridge subscribes to the event bus and fans out events to all connected WebSocket clients based on their subscriptions.

```rust
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use axum::extract::ws::{WebSocket, Message};

struct WsState {
    /// Connected clients and their subscriptions
    clients: RwLock<HashMap<Uuid, WsClient>>,
}

struct WsClient {
    identity_id: Uuid,
    role: Role,
    sender: mpsc::Sender<WsServerMessage>,
    subscriptions: Vec<WsSubscription>,
}

impl WsState {
    /// Start the bridge that reads from event bus and fans out to WS clients
    async fn start_bridge(self: Arc<Self>, bus: &EventBus) {
        let mut rx = bus.subscribe();
        let state = self.clone();

        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(envelope) => {
                        let ws_event = transform_event(&envelope);
                        let clients = state.clients.read().await;

                        for client in clients.values() {
                            if client.should_receive(&ws_event) {
                                let msg = WsServerMessage {
                                    msg_type: WsServerMessageType::Event(ws_event.clone()),
                                    request_id: None,
                                    timestamp: envelope.timestamp,
                                };
                                // Non-blocking send — drop if client is slow
                                let _ = client.sender.try_send(msg);
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("WebSocket bridge lagged, missed {n} events");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
}

impl WsClient {
    fn should_receive(&self, event: &WsEvent) -> bool {
        self.subscriptions.iter().any(|sub| {
            // Category match
            let category_match = sub.categories.is_empty()
                || sub.categories.contains(&WsEventCategory::All)
                || sub.categories.contains(&event.category);

            // Project match
            let project_match = sub.project_ids.is_empty()
                || event.project_id.map_or(true, |pid| sub.project_ids.contains(&pid));

            category_match && project_match
        })
    }
}
```

### 4.7 Connection Lifecycle

```
Client                                    Server
  │                                          │
  ├── GET /ws?token=jwt ──────────────────→  │
  │                                          ├── Validate JWT
  │  ←── 101 Switching Protocols ──────────  │
  │                                          │
  ├── Subscribe { Tasks, Workers } ───────→  │
  │  ←── Ack { success: true } ───────────  │
  │                                          │
  │  ←── Event { TaskStarted ... } ───────  │
  │  ←── Event { TaskProgress 50% } ──────  │
  │  ←── Event { TaskCompleted } ─────────  │
  │                                          │
  ├── ChatMessage { "状态如何?" } ─────────→  │
  │  ←── ChatResponse { "当前有3个任务..." }  │
  │                                          │
  │  ←── Notification { deploy approval } ─  │
  ├── Approve { deployment, uuid } ───────→  │
  │  ←── Ack { success: true } ───────────  │
  │                                          │
  ├── Ping ────────────────────────────────→  │
  │  ←── Pong ─────────────────────────────  │
  │                                          │
  ├── Close ───────────────────────────────→  │
  │  ←── Close ────────────────────────────  │
```

### 4.8 Keepalive & Reconnection

- **Server-side**: Ping frames every 30 seconds. If no pong received within 10 seconds, connection is closed.
- **Client-side**: Should send `Ping` message every 25 seconds. Should automatically reconnect on disconnect with exponential backoff (1s, 2s, 4s, 8s, max 30s).
- **Re-subscribe on reconnect**: Client should re-send all `Subscribe` messages after reconnecting.

### 4.9 Rate Limiting

WebSocket messages are rate-limited per client:
- Chat messages: 10 per minute
- Subscribe/Unsubscribe: 30 per minute
- Approve/Reject: 60 per minute
- Ping: unlimited

---

## 5. Axum Router Structure

The API is mounted in `argus-social` (web dashboard adapter) with the following structure:

```rust
use axum::{Router, middleware};

fn api_router(state: AppState) -> Router {
    Router::new()
        // Public routes
        .route("/api/v1/auth/login", post(auth::login))
        .route("/api/v1/system/version", get(system::version))
        // Protected routes
        .nest("/api/v1", protected_routes(state.clone()))
        // WebSocket
        .route("/ws", get(ws::ws_handler))
        .with_state(state)
}

fn protected_routes(state: AppState) -> Router {
    Router::new()
        // Auth
        .route("/auth/logout", post(auth::logout))
        .route("/auth/me", get(auth::me))
        .route("/auth/refresh", post(auth::refresh))
        // Projects
        .route("/projects", get(projects::list).post(projects::create))
        .route("/projects/:id", get(projects::get).put(projects::update).delete(projects::delete))
        .route("/projects/:id/pause", post(projects::pause))
        .route("/projects/:id/resume", post(projects::resume))
        .route("/projects/:id/milestones", get(projects::milestones))
        .route("/projects/:id/stats", get(projects::stats))
        .route("/projects/:id/tasks", get(projects::tasks))
        .route("/projects/:id/deployments", get(projects::deployments))
        .route("/projects/:id/environments", get(projects::environments))
        .route("/projects/:id/environments/:env", put(projects::update_env))
        // Tasks
        .route("/tasks", get(tasks::list).post(tasks::create))
        .route("/tasks/:id", get(tasks::get).put(tasks::update).delete(tasks::cancel))
        .route("/tasks/:id/retry", post(tasks::retry))
        .route("/tasks/:id/episodes", get(tasks::episodes))
        .route("/tasks/:id/checkpoints", get(tasks::checkpoints))
        // Workers
        .route("/workers", get(workers::list))
        .route("/workers/spawn", post(workers::spawn))
        .route("/workers/stats", get(workers::stats))
        .route("/workers/:id", get(workers::get))
        .route("/workers/:id/kill", post(workers::kill))
        // Deployments
        .route("/deployments", get(deployments::list).post(deployments::trigger))
        .route("/deployments/:id", get(deployments::get))
        .route("/deployments/:id/approve", post(deployments::approve))
        .route("/deployments/:id/reject", post(deployments::reject))
        .route("/deployments/:id/rollback", post(deployments::rollback))
        .route("/deployments/:id/logs", get(deployments::logs))
        // Config
        .route("/config", get(config::get_all))
        .route("/config/undo", post(config::undo))
        .route("/config/audit", get(config::audit))
        .route("/config/export", post(config::export))
        .route("/config/import", post(config::import_config))
        .route("/config/:domain", get(config::get_domain))
        .route("/config/:domain/:key", put(config::set).delete(config::delete))
        // LLM
        .route("/llm/usage", get(llm::usage))
        .route("/llm/usage/daily", get(llm::usage_daily))
        .route("/llm/providers", get(llm::providers))
        .route("/llm/budget", get(llm::budget))
        // Memory
        .route("/memory/search", post(memory::search))
        .route("/memory/stats", get(memory::stats))
        .route("/memory/store", post(memory::store))
        .route("/memory/:id", get(memory::get).delete(memory::delete))
        // Audit
        .route("/audit", get(audit::query))
        .route("/audit/stats", get(audit::stats))
        // System
        .route("/system/health", get(system::health))
        .route("/system/shutdown", post(system::shutdown))
        .route("/system/backup", post(system::backup))
        .route("/system/backups", get(system::list_backups))
        // Chat
        .route("/chat/send", post(chat::send))
        .route("/chat/history", get(chat::history))
        // MCP
        .route("/mcp/servers", get(mcp::list_servers).post(mcp::install_server))
        .route("/mcp/servers/:name", delete(mcp::remove_server))
        .route("/mcp/servers/:name/audit", post(mcp::audit_server))
        // Skills
        .route("/skills", get(skills::list).post(skills::install))
        .route("/skills/:name", delete(skills::remove))
        .route("/skills/:name/execute", post(skills::execute))
        // Evolution
        .route("/evolution/history", get(evolution::history))
        .route("/evolution", post(evolution::request))
        .route("/evolution/:id", get(evolution::get))
        .route("/evolution/:id/approve", post(evolution::approve))
        .route("/evolution/:id/reject", post(evolution::reject))
        // Collaborations
        .route("/collaborations", get(collaborations::list))
        .route("/collaborations/stats", get(collaborations::stats))
        .route("/collaborations/:id", get(collaborations::get))
        // Middleware
        .layer(middleware::from_fn_with_state(state.clone(), auth::auth_middleware))
        .layer(middleware::from_fn_with_state(state.clone(), audit::audit_middleware))
        .layer(middleware::from_fn(rate_limit::rate_limit_middleware))
        .with_state(state)
}
```

### 5.1 Shared Application State

```rust
#[derive(Clone)]
struct AppState {
    db: PgPool,
    event_bus: Arc<EventBus>,
    ws_state: Arc<WsState>,
    // Module handles for API to invoke operations
    brain: Arc<BrainEngine>,
    scheduler: Arc<TaskScheduler>,
    worker_manager: Arc<WorkerManager>,
    deploy_engine: Arc<DeployEngine>,
    memory_engine: Arc<MemoryEngine>,
    mcp_client: Arc<McpClientManager>,
    skill_engine: Arc<SkillEngine>,
    config_store: Arc<ConfigStore>,
    security: Arc<SecurityModule>,
    key_vault: Arc<KeyVault>,
}
```

---

## 6. CORS & Security Headers

```rust
use tower_http::cors::{CorsLayer, Any};
use axum::http::header;

fn cors_layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(["https://argus.example.com".parse().unwrap()])
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION])
        .allow_credentials(true)
        .max_age(Duration::from_secs(3600))
}

// Security headers applied via middleware
// X-Content-Type-Options: nosniff
// X-Frame-Options: DENY
// X-XSS-Protection: 1; mode=block
// Strict-Transport-Security: max-age=31536000; includeSubDomains
// Content-Security-Policy: default-src 'self'
// Referrer-Policy: strict-origin-when-cross-origin
```

---

## 7. Performance Targets

| Metric | Target |
|--------|--------|
| REST API response time (95th percentile) | < 100ms |
| REST API response time (99th percentile) | < 500ms |
| WebSocket event delivery latency | < 50ms from event bus publish |
| Concurrent WebSocket connections | 100+ |
| REST API concurrent requests | 1000+ |
| Authentication overhead | < 5ms (JWT validation) |
| Database query time (typical) | < 20ms |
| Semantic search (memory) | < 500ms |

---

## 8. Versioning

The API is versioned via URL path (`/api/v1/`). Breaking changes will increment the version number. Non-breaking additions (new fields, new endpoints) can be added to the current version without bumping.

Future versions will coexist temporarily:
```
/api/v1/projects   ← maintained for backward compat
/api/v2/projects   ← new version with breaking changes
```

Deprecation policy: Old API versions are supported for at least 3 months after a new version is released, with deprecation warnings in response headers:
```
Deprecation: true
Sunset: 2026-06-01
Link: </api/v2/projects>; rel="successor-version"
```
