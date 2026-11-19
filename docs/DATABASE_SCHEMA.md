# Database Schema — Complete PostgreSQL Design

All persistent state in Argus is stored in PostgreSQL with the pgvector extension. This document contains the complete schema — every CREATE TABLE, index, constraint, and foreign key. Migrations are managed via `sqlx-cli`.

## 1. Prerequisites

```sql
-- Required extensions
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";
CREATE EXTENSION IF NOT EXISTS "pgvector";
CREATE EXTENSION IF NOT EXISTS "pg_trgm";  -- for text search

-- Custom types
CREATE TYPE task_state AS ENUM (
    'pending', 'ready', 'queued', 'assigned', 'running',
    'paused', 'completed', 'failed', 'cancelled', 'timed_out'
);

CREATE TYPE task_priority AS ENUM (
    'critical', 'high', 'medium', 'low', 'background'
);

CREATE TYPE environment AS ENUM (
    'development', 'staging', 'production', 'custom'
);

CREATE TYPE deployment_status AS ENUM (
    'queued', 'running', 'waiting_approval', 'approved',
    'succeeded', 'failed', 'rolled_back', 'cancelled'
);

CREATE TYPE worker_state AS ENUM (
    'idle', 'busy', 'starting', 'crashed', 'shutting_down'
);

CREATE TYPE worker_type AS ENUM (
    'opencode', 'claude_code', 'aider', 'generic_cli'
);

CREATE TYPE memory_layer AS ENUM (
    'working', 'short_term', 'long_term', 'permanent'
);

CREATE TYPE memory_type AS ENUM (
    'fact', 'decision', 'experience', 'preference',
    'error_pattern', 'skill_learned', 'project_context',
    'conversation_summary', 'safety_rule'
);

CREATE TYPE trust_level AS ENUM (
    'builtin', 'verified', 'third_party', 'untrusted'
);

CREATE TYPE role AS ENUM (
    'owner', 'admin', 'developer', 'viewer'
);

CREATE TYPE platform AS ENUM (
    'telegram', 'discord', 'wechat', 'slack', 'web_dashboard'
);

CREATE TYPE audit_outcome AS ENUM (
    'success', 'failure', 'blocked', 'warned'
);

CREATE TYPE evolution_status AS ENUM (
    'requested', 'safety_checking', 'safety_approved', 'safety_rejected',
    'branch_created', 'in_development', 'testing', 'tests_passed',
    'tests_failed', 'test_instance_running', 'awaiting_approval',
    'approved', 'rejected', 'deploying', 'deployed',
    'health_check_passed', 'health_check_failed', 'rolled_back',
    'completed', 'failed'
);

CREATE TYPE collaboration_mode AS ENUM (
    'solo', 'pair_review', 'full_team'
);

CREATE TYPE collaboration_phase AS ENUM (
    'architecture', 'implementation', 'review', 'testing',
    'security_audit', 'merging', 'completed', 'failed'
);

CREATE TYPE collaboration_role AS ENUM (
    'architect', 'developer', 'reviewer', 'tester',
    'security_auditor', 'devops', 'tech_lead'
);
```

## 2. Projects

```sql
-- ============================================================
-- PROJECTS
-- ============================================================

CREATE TABLE projects (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    name            TEXT NOT NULL UNIQUE,
    description     TEXT,
    -- Git
    git_repo_url    TEXT,
    git_local_path  TEXT NOT NULL,
    default_branch  TEXT NOT NULL DEFAULT 'main',
    -- Build & test
    language        TEXT,               -- rust, typescript, python, etc.
    framework       TEXT,               -- next, django, actix, etc.
    build_command   TEXT,
    test_command    TEXT,
    -- Status
    active          BOOLEAN NOT NULL DEFAULT true,
    priority        task_priority NOT NULL DEFAULT 'medium',
    -- Metadata
    metadata        JSONB NOT NULL DEFAULT '{}',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_projects_active ON projects (active) WHERE active = true;
CREATE INDEX idx_projects_name ON projects USING gin (name gin_trgm_ops);
```

## 3. Milestones

```sql
-- ============================================================
-- MILESTONES
-- ============================================================

CREATE TABLE milestones (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    project_id      UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    description     TEXT,
    -- Ordering
    sequence        INTEGER NOT NULL,
    -- Status
    status          TEXT NOT NULL DEFAULT 'pending',  -- pending, in_progress, completed, failed
    -- Progress
    total_tasks     INTEGER NOT NULL DEFAULT 0,
    completed_tasks INTEGER NOT NULL DEFAULT 0,
    -- Timestamps
    started_at      TIMESTAMPTZ,
    completed_at    TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    UNIQUE (project_id, sequence)
);

CREATE INDEX idx_milestones_project ON milestones (project_id);
```

## 4. Tasks

```sql
-- ============================================================
-- TASKS
-- ============================================================

CREATE TABLE tasks (
    id                  UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    project_id          UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    milestone_id        UUID REFERENCES milestones(id) ON DELETE SET NULL,
    -- Task definition
    title               TEXT NOT NULL,
    description         TEXT,
    task_type           TEXT NOT NULL,          -- code, test, deploy, review, research, etc.
    -- Execution
    state               task_state NOT NULL DEFAULT 'pending',
    priority            task_priority NOT NULL DEFAULT 'medium',
    effective_priority  REAL NOT NULL DEFAULT 0.0,
    assigned_worker_id  UUID,                   -- FK to workers
    -- Retry logic
    retry_count         INTEGER NOT NULL DEFAULT 0,
    max_retries         INTEGER NOT NULL DEFAULT 3,
    -- Time tracking
    timeout_seconds     INTEGER NOT NULL DEFAULT 3600,
    estimated_seconds   INTEGER,
    -- Results
    output              JSONB,                  -- TaskOutput JSON
    error               TEXT,
    -- Context
    context             JSONB NOT NULL DEFAULT '{}',  -- input data, file paths, etc.
    -- Git
    branch              TEXT,
    commit_sha          TEXT,
    -- Timestamps
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    started_at          TIMESTAMPTZ,
    completed_at        TIMESTAMPTZ,
    deadline            TIMESTAMPTZ
);

CREATE INDEX idx_tasks_project ON tasks (project_id);
CREATE INDEX idx_tasks_state ON tasks (state);
CREATE INDEX idx_tasks_priority ON tasks (effective_priority DESC);
CREATE INDEX idx_tasks_milestone ON tasks (milestone_id) WHERE milestone_id IS NOT NULL;
CREATE INDEX idx_tasks_worker ON tasks (assigned_worker_id) WHERE assigned_worker_id IS NOT NULL;
CREATE INDEX idx_tasks_ready ON tasks (effective_priority DESC) WHERE state = 'ready';
```

## 5. Task Dependencies

```sql
-- ============================================================
-- TASK DEPENDENCIES (DAG edges)
-- ============================================================

CREATE TABLE task_dependencies (
    task_id         UUID NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    depends_on_id   UUID NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    PRIMARY KEY (task_id, depends_on_id),
    CHECK (task_id != depends_on_id)
);

CREATE INDEX idx_task_deps_depends_on ON task_dependencies (depends_on_id);
```

## 6. Task Checkpoints

```sql
-- ============================================================
-- TASK CHECKPOINTS (for crash recovery)
-- ============================================================

CREATE TABLE task_checkpoints (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    task_id         UUID NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    -- Checkpoint data
    state           task_state NOT NULL,
    progress        REAL NOT NULL DEFAULT 0.0,  -- 0.0 to 1.0
    worker_output   JSONB,                       -- partial output
    context         JSONB NOT NULL DEFAULT '{}',
    -- Timestamps
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_task_checkpoints_task ON task_checkpoints (task_id, created_at DESC);
```

## 7. Memories

```sql
-- ============================================================
-- MEMORIES (Four-layer memory system)
-- ============================================================

CREATE TABLE memories (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    -- Classification
    layer           memory_layer NOT NULL,
    memory_type     memory_type NOT NULL,
    -- Content
    content         TEXT NOT NULL,
    summary         TEXT,                        -- condensed version
    -- Associations
    project_id      UUID REFERENCES projects(id) ON DELETE SET NULL,
    task_id         UUID REFERENCES tasks(id) ON DELETE SET NULL,
    -- Embedding (BGE-M3, 1024 dimensions)
    embedding       vector(1024),
    -- Importance & decay
    importance      REAL NOT NULL DEFAULT 0.5,   -- 0.0 to 1.0
    access_count    INTEGER NOT NULL DEFAULT 0,
    decay_rate      REAL NOT NULL DEFAULT 0.01,  -- per day
    -- Tags
    tags            TEXT[] NOT NULL DEFAULT '{}',
    -- Source
    source_model    TEXT,                        -- which LLM generated this
    source_type     TEXT,                        -- conversation, task_output, etc.
    -- Timestamps
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_accessed   TIMESTAMPTZ,
    expires_at      TIMESTAMPTZ                  -- NULL = no expiration
);

-- HNSW index for semantic search (pgvector)
CREATE INDEX idx_memories_embedding ON memories
    USING hnsw (embedding vector_cosine_ops)
    WITH (m = 16, ef_construction = 64);

CREATE INDEX idx_memories_layer ON memories (layer);
CREATE INDEX idx_memories_project ON memories (project_id) WHERE project_id IS NOT NULL;
CREATE INDEX idx_memories_importance ON memories (importance DESC);
CREATE INDEX idx_memories_expires ON memories (expires_at) WHERE expires_at IS NOT NULL;
CREATE INDEX idx_memories_type ON memories (memory_type);
CREATE INDEX idx_memories_tags ON memories USING gin (tags);
CREATE INDEX idx_memories_content_search ON memories USING gin (content gin_trgm_ops);
```

## 8. Memory Relations

```sql
-- ============================================================
-- MEMORY RELATIONS (links between memories)
-- ============================================================

CREATE TABLE memory_relations (
    source_id       UUID NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    target_id       UUID NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    relation_type   TEXT NOT NULL,  -- 'relates_to', 'supersedes', 'contradicts', 'supports'
    strength        REAL NOT NULL DEFAULT 1.0,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    PRIMARY KEY (source_id, target_id, relation_type)
);

CREATE INDEX idx_memory_relations_target ON memory_relations (target_id);
```

## 9. Conversation Summaries

```sql
-- ============================================================
-- CONVERSATION SUMMARIES
-- ============================================================

CREATE TABLE conversation_summaries (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    -- Context
    project_id      UUID REFERENCES projects(id) ON DELETE SET NULL,
    platform        platform,
    session_id      TEXT,
    -- Content
    summary         TEXT NOT NULL,
    message_count   INTEGER NOT NULL,
    -- Key decisions / action items extracted
    decisions       JSONB NOT NULL DEFAULT '[]',
    action_items    JSONB NOT NULL DEFAULT '[]',
    -- Embedding for retrieval
    embedding       vector(1024),
    -- Time range covered
    period_start    TIMESTAMPTZ NOT NULL,
    period_end      TIMESTAMPTZ NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_conv_summaries_project ON conversation_summaries (project_id);
CREATE INDEX idx_conv_summaries_period ON conversation_summaries (period_start, period_end);
CREATE INDEX idx_conv_summaries_embedding ON conversation_summaries
    USING hnsw (embedding vector_cosine_ops)
    WITH (m = 16, ef_construction = 64);
```

## 10. LLM Usage

```sql
-- ============================================================
-- LLM USAGE TRACKING
-- ============================================================

CREATE TABLE llm_usage (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    -- Provider & model
    provider        TEXT NOT NULL,
    model           TEXT NOT NULL,
    model_class     TEXT NOT NULL,       -- reasoning, coding, fast, long_context
    -- Token counts
    input_tokens    INTEGER NOT NULL,
    output_tokens   INTEGER NOT NULL,
    total_tokens    INTEGER NOT NULL,
    -- Cost
    cost_usd        REAL NOT NULL DEFAULT 0.0,
    -- Context
    project_id      UUID REFERENCES projects(id) ON DELETE SET NULL,
    task_id         UUID REFERENCES tasks(id) ON DELETE SET NULL,
    purpose         TEXT,                -- task_execution, decomposition, review, etc.
    -- Performance
    latency_ms      INTEGER,
    success         BOOLEAN NOT NULL DEFAULT true,
    error           TEXT,
    -- Timestamps
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_llm_usage_provider ON llm_usage (provider, created_at);
CREATE INDEX idx_llm_usage_project ON llm_usage (project_id) WHERE project_id IS NOT NULL;
CREATE INDEX idx_llm_usage_created ON llm_usage (created_at);
CREATE INDEX idx_llm_usage_model ON llm_usage (model);

-- Materialized view for cost reporting (refresh periodically)
CREATE MATERIALIZED VIEW llm_usage_daily AS
SELECT
    DATE_TRUNC('day', created_at) AS day,
    provider,
    model,
    project_id,
    COUNT(*) AS request_count,
    SUM(input_tokens) AS total_input_tokens,
    SUM(output_tokens) AS total_output_tokens,
    SUM(cost_usd) AS total_cost_usd,
    AVG(latency_ms) AS avg_latency_ms,
    COUNT(*) FILTER (WHERE success = true) AS success_count,
    COUNT(*) FILTER (WHERE success = false) AS failure_count
FROM llm_usage
GROUP BY day, provider, model, project_id;

CREATE UNIQUE INDEX idx_llm_usage_daily_pk
    ON llm_usage_daily (day, provider, model, project_id);
```

## 11. Workers

```sql
-- ============================================================
-- WORKERS
-- ============================================================

CREATE TABLE workers (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    -- Configuration
    worker_type     worker_type NOT NULL,
    name            TEXT NOT NULL,
    -- State
    state           worker_state NOT NULL DEFAULT 'idle',
    current_task_id UUID REFERENCES tasks(id) ON DELETE SET NULL,
    -- Resource usage
    pid             INTEGER,
    memory_bytes    BIGINT,
    cpu_percent     REAL,
    -- Sandbox
    sandbox_type    TEXT NOT NULL DEFAULT 'directory',  -- directory, docker, none
    workspace_path  TEXT,
    container_id    TEXT,                                -- Docker container ID if applicable
    -- Statistics
    tasks_completed INTEGER NOT NULL DEFAULT 0,
    tasks_failed    INTEGER NOT NULL DEFAULT 0,
    total_uptime_seconds BIGINT NOT NULL DEFAULT 0,
    -- Timestamps
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_heartbeat  TIMESTAMPTZ,
    last_task_at    TIMESTAMPTZ
);

CREATE INDEX idx_workers_state ON workers (state);
CREATE INDEX idx_workers_type ON workers (worker_type);
```

## 12. Deployments

```sql
-- ============================================================
-- DEPLOYMENTS
-- ============================================================

CREATE TABLE deployments (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    project_id      UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    -- Environment & target
    environment     environment NOT NULL,
    target_type     TEXT NOT NULL,           -- docker, vps, vercel, k8s, wechat, custom
    -- Trigger
    trigger_type    JSONB NOT NULL,          -- automatic, owner_request, scheduled, retry
    -- Git info
    git_ref         TEXT NOT NULL,           -- commit SHA
    git_branch      TEXT NOT NULL,
    -- Status
    status          deployment_status NOT NULL DEFAULT 'queued',
    -- Pipeline stages
    stages_json     JSONB NOT NULL DEFAULT '[]',
    -- Rollback
    snapshot_id     UUID,
    rolled_back     BOOLEAN NOT NULL DEFAULT false,
    -- Config snapshot
    config_json     JSONB NOT NULL DEFAULT '{}',
    -- Metadata
    metadata        JSONB NOT NULL DEFAULT '{}',
    -- Timestamps
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    finished_at     TIMESTAMPTZ
);

CREATE INDEX idx_deployments_project ON deployments (project_id, created_at DESC);
CREATE INDEX idx_deployments_status ON deployments (status);
CREATE INDEX idx_deployments_env ON deployments (project_id, environment);
```

## 13. Deployment Snapshots

```sql
-- ============================================================
-- DEPLOYMENT SNAPSHOTS (for rollback)
-- ============================================================

CREATE TABLE deployment_snapshots (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    deployment_id   UUID NOT NULL REFERENCES deployments(id) ON DELETE CASCADE,
    snapshot_type   TEXT NOT NULL,
    snapshot_data   JSONB NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_deployment_snapshots_deployment ON deployment_snapshots (deployment_id);
```

## 14. Deployment Artifacts

```sql
-- ============================================================
-- DEPLOYMENT ARTIFACTS
-- ============================================================

CREATE TABLE deployment_artifacts (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    deployment_id   UUID NOT NULL REFERENCES deployments(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    path            TEXT NOT NULL,
    size_bytes      BIGINT NOT NULL,
    checksum_sha256 TEXT NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_deployment_artifacts_deployment ON deployment_artifacts (deployment_id);
```

## 15. Environment Configs

```sql
-- ============================================================
-- ENVIRONMENT CONFIGS (per-project deployment targets)
-- ============================================================

CREATE TABLE environment_configs (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    project_id      UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    environment     TEXT NOT NULL,
    config_data     JSONB NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    UNIQUE (project_id, environment)
);

CREATE INDEX idx_env_configs_project ON environment_configs (project_id);
```

## 16. MCP Servers

```sql
-- ============================================================
-- MCP SERVERS
-- ============================================================

CREATE TABLE mcp_servers (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    name            TEXT NOT NULL UNIQUE,
    -- Connection
    protocol        TEXT NOT NULL,          -- stdio, sse, streamable_http
    command         TEXT,                    -- for stdio
    args            TEXT[],
    url             TEXT,                    -- for sse/http
    -- Trust & security
    trust_level     trust_level NOT NULL DEFAULT 'untrusted',
    audit_result    JSONB,                   -- security audit verdict
    -- Available tools
    tools           JSONB NOT NULL DEFAULT '[]',
    -- Status
    enabled         BOOLEAN NOT NULL DEFAULT true,
    -- Source
    source          TEXT,                    -- npm package, git repo, builtin, etc.
    version         TEXT,
    -- Timestamps
    installed_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_used       TIMESTAMPTZ,
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_mcp_servers_trust ON mcp_servers (trust_level);
CREATE INDEX idx_mcp_servers_enabled ON mcp_servers (enabled) WHERE enabled = true;
```

## 17. Skills

```sql
-- ============================================================
-- SKILLS
-- ============================================================

CREATE TABLE skills (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    name            TEXT NOT NULL UNIQUE,
    description     TEXT,
    -- Skill type
    skill_type      TEXT NOT NULL,          -- builtin, third_party, self_generated
    -- Source
    source_path     TEXT,                    -- file path for script-based skills
    source_code     TEXT,                    -- inline code for small skills
    language        TEXT,                    -- python, javascript, shell
    -- Permissions
    permissions     JSONB NOT NULL DEFAULT '[]',
    trust_level     trust_level NOT NULL DEFAULT 'untrusted',
    -- Execution
    timeout_seconds INTEGER NOT NULL DEFAULT 300,
    sandbox         BOOLEAN NOT NULL DEFAULT true,
    -- Status
    enabled         BOOLEAN NOT NULL DEFAULT true,
    -- Metadata
    version         TEXT,
    author          TEXT,
    tags            TEXT[] NOT NULL DEFAULT '{}',
    -- Timestamps
    installed_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_used       TIMESTAMPTZ,
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_skills_type ON skills (skill_type);
CREATE INDEX idx_skills_enabled ON skills (enabled) WHERE enabled = true;
CREATE INDEX idx_skills_tags ON skills USING gin (tags);
```

## 18. Key Vault

```sql
-- ============================================================
-- KEY VAULT (encrypted secrets)
-- ============================================================

CREATE TABLE key_vault (
    name            TEXT PRIMARY KEY,
    category        JSONB NOT NULL,
    encrypted_value BYTEA NOT NULL,
    nonce           BYTEA NOT NULL,
    metadata        JSONB NOT NULL DEFAULT '{}',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_accessed   TIMESTAMPTZ
);
```

## 19. Identities (RBAC)

```sql
-- ============================================================
-- IDENTITIES (users / access control)
-- ============================================================

CREATE TABLE identities (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    name            TEXT NOT NULL,
    role            role NOT NULL DEFAULT 'viewer',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_active     TIMESTAMPTZ
);

CREATE TABLE identity_platforms (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    identity_id     UUID NOT NULL REFERENCES identities(id) ON DELETE CASCADE,
    platform        platform NOT NULL,
    platform_user_id TEXT NOT NULL,
    username        TEXT,
    -- Web dashboard auth
    password_hash   TEXT,                    -- bcrypt hash, only for web_dashboard
    -- Timestamps
    linked_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    UNIQUE (platform, platform_user_id)
);

CREATE INDEX idx_identity_platforms_identity ON identity_platforms (identity_id);
CREATE INDEX idx_identity_platforms_lookup ON identity_platforms (platform, platform_user_id);
```

## 20. Configuration Store

```sql
-- ============================================================
-- CONFIGURATION (key-value config store)
-- ============================================================

CREATE TABLE config (
    path            TEXT PRIMARY KEY,
    value           JSONB NOT NULL,
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_config_path_prefix ON config USING btree (path text_pattern_ops);
```

## 21. Config Audit Log

```sql
-- ============================================================
-- CONFIG AUDIT LOG
-- ============================================================

CREATE TABLE config_audit_log (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    timestamp       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    action          TEXT NOT NULL,
    domain          TEXT NOT NULL,
    old_value       JSONB,
    new_value       JSONB,
    description     TEXT NOT NULL,
    source          JSONB NOT NULL
);

CREATE INDEX idx_config_audit_timestamp ON config_audit_log (timestamp DESC);
CREATE INDEX idx_config_audit_domain ON config_audit_log (domain);
```

## 22. System Audit Log

```sql
-- ============================================================
-- AUDIT LOG (system-wide security & activity log)
-- ============================================================

CREATE TABLE audit_log (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    timestamp       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    actor           JSONB NOT NULL,
    action          JSONB NOT NULL,
    resource        JSONB NOT NULL,
    outcome         audit_outcome NOT NULL,
    details         JSONB NOT NULL DEFAULT '{}',
    ip_address      INET,
    session_id      TEXT
);

CREATE INDEX idx_audit_log_timestamp ON audit_log (timestamp DESC);
CREATE INDEX idx_audit_log_action ON audit_log USING gin (action);
CREATE INDEX idx_audit_log_actor ON audit_log USING gin (actor);
CREATE INDEX idx_audit_log_outcome ON audit_log (outcome);

-- Partition by month for performance (optional, for high-volume deployments)
-- CREATE TABLE audit_log (
--     ...
-- ) PARTITION BY RANGE (timestamp);
```

## 23. Owner Profiles

```sql
-- ============================================================
-- OWNER PROFILES
-- ============================================================

CREATE TABLE owner_profiles (
    id                  UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    identity_id         UUID NOT NULL REFERENCES identities(id) ON DELETE CASCADE,
    -- Preferences
    disturbance_level   TEXT NOT NULL DEFAULT 'normal',  -- minimal, normal, verbose
    communication_style TEXT NOT NULL DEFAULT 'concise', -- minimal, concise, detailed, technical
    language            TEXT NOT NULL DEFAULT 'zh-CN',
    timezone            TEXT NOT NULL DEFAULT 'Asia/Shanghai',
    -- Active hours
    active_hours_start  TIME NOT NULL DEFAULT '08:00',
    active_hours_end    TIME NOT NULL DEFAULT '23:00',
    -- Approval
    approval_mode       TEXT NOT NULL DEFAULT 'critical_only', -- all, critical_only, none
    -- Learned preferences (accumulated from interactions)
    learned_preferences JSONB NOT NULL DEFAULT '{}',
    -- Timestamps
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_owner_profiles_identity ON owner_profiles (identity_id);
```

## 24. Evolution History

```sql
-- ============================================================
-- EVOLUTION HISTORY (self-modification records)
-- ============================================================

CREATE TABLE evolution_history (
    id                  UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    description         TEXT NOT NULL,
    category            JSONB NOT NULL,
    status              JSONB NOT NULL,
    branch_name         TEXT NOT NULL,
    -- Changes
    changes_json        JSONB NOT NULL DEFAULT '[]',
    -- Test results
    test_results_json   JSONB,
    -- Approval
    approval_json       JSONB,
    -- Deployment info
    deployment_json     JSONB,
    -- Timestamps
    requested_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at        TIMESTAMPTZ
);

CREATE INDEX idx_evolution_history_status ON evolution_history USING gin (status);
CREATE INDEX idx_evolution_history_time ON evolution_history (requested_at DESC);
```

## 25. Episodes (Task Execution Episodes)

```sql
-- ============================================================
-- EPISODES (detailed execution log per task)
-- ============================================================

CREATE TABLE episodes (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    task_id         UUID NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    worker_id       UUID REFERENCES workers(id) ON DELETE SET NULL,
    -- Execution details
    attempt         INTEGER NOT NULL DEFAULT 1,
    -- Input / output
    input           JSONB NOT NULL DEFAULT '{}',
    output          JSONB,
    -- Status
    status          TEXT NOT NULL,   -- started, running, completed, failed, timeout
    error           TEXT,
    -- Performance
    tokens_used     INTEGER,
    cost_usd        REAL,
    -- Timestamps
    started_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at    TIMESTAMPTZ
);

CREATE INDEX idx_episodes_task ON episodes (task_id, attempt);
CREATE INDEX idx_episodes_worker ON episodes (worker_id) WHERE worker_id IS NOT NULL;
```

## 26. Notification Buffer

```sql
-- ============================================================
-- NOTIFICATION BUFFER (messages queued for digest delivery)
-- ============================================================

CREATE TABLE notification_buffer (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    -- Target
    identity_id     UUID NOT NULL REFERENCES identities(id) ON DELETE CASCADE,
    platform        platform NOT NULL,
    -- Content
    priority        TEXT NOT NULL DEFAULT 'normal',  -- critical, high, normal, low
    title           TEXT NOT NULL,
    body            TEXT NOT NULL,
    metadata        JSONB NOT NULL DEFAULT '{}',
    -- Status
    delivered       BOOLEAN NOT NULL DEFAULT false,
    delivered_at    TIMESTAMPTZ,
    -- Timestamps
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    deliver_after   TIMESTAMPTZ           -- earliest delivery time (for buffering)
);

CREATE INDEX idx_notification_buffer_undelivered
    ON notification_buffer (identity_id, deliver_after)
    WHERE delivered = false;
```

## 27. Web Dashboard Sessions

```sql
-- ============================================================
-- WEB SESSIONS (dashboard authentication)
-- ============================================================

CREATE TABLE web_sessions (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    identity_id     UUID NOT NULL REFERENCES identities(id) ON DELETE CASCADE,
    token_hash      TEXT NOT NULL UNIQUE,        -- bcrypt hash of session token
    ip_address      INET,
    user_agent      TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at      TIMESTAMPTZ NOT NULL,
    last_active     TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_web_sessions_identity ON web_sessions (identity_id);
CREATE INDEX idx_web_sessions_token ON web_sessions (token_hash);
CREATE INDEX idx_web_sessions_expires ON web_sessions (expires_at);
```

## 28. LLM Provider Configuration

```sql
-- ============================================================
-- LLM PROVIDERS (runtime provider configuration)
-- ============================================================

CREATE TABLE llm_providers (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    name            TEXT NOT NULL UNIQUE,        -- openai, anthropic, deepseek, etc.
    display_name    TEXT NOT NULL,
    -- Configuration
    base_url        TEXT,
    api_key_name    TEXT,                         -- reference to key_vault
    models          JSONB NOT NULL DEFAULT '[]',  -- available models
    model_class_map JSONB NOT NULL DEFAULT '{}',  -- model → class mapping
    -- Budget
    monthly_budget_usd REAL,
    current_month_usage REAL NOT NULL DEFAULT 0.0,
    -- Rate limiting
    rpm_limit       INTEGER,                      -- requests per minute
    tpm_limit       INTEGER,                      -- tokens per minute
    -- Failover
    priority        INTEGER NOT NULL DEFAULT 100,  -- lower = higher priority
    enabled         BOOLEAN NOT NULL DEFAULT true,
    -- Key rotation
    api_keys        JSONB NOT NULL DEFAULT '[]',   -- list of key vault references
    current_key_index INTEGER NOT NULL DEFAULT 0,
    -- Timestamps
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_llm_providers_priority ON llm_providers (priority) WHERE enabled = true;
```

## 29. Database Backups

```sql
-- ============================================================
-- DATABASE BACKUPS (metadata tracking)
-- ============================================================

CREATE TABLE database_backups (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    file_path       TEXT NOT NULL,
    size_bytes      BIGINT NOT NULL,
    trigger         TEXT NOT NULL,           -- scheduled, pre_migration, manual
    status          TEXT NOT NULL,           -- completed, failed
    error           TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_database_backups_time ON database_backups (created_at DESC);
```

## 30. Collaboration Sessions

```sql
-- ============================================================
-- COLLABORATION SESSIONS
-- Tracks role-based collaboration sessions for complex tasks.
-- See COLLABORATION.md for full system design.
-- ============================================================

CREATE TABLE collaboration_sessions (
    id                      UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    task_id                 UUID NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    project_id              UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    -- Collaboration config
    mode                    collaboration_mode NOT NULL,
    phase                   collaboration_phase NOT NULL DEFAULT 'architecture',
    -- Structured data
    role_assignments        JSONB NOT NULL DEFAULT '{}',      -- CollaborationRole → RoleAssignment
    review_policy           JSONB NOT NULL DEFAULT '{}',      -- ReviewPolicy struct
    complexity_assessment   JSONB NOT NULL DEFAULT '{}',      -- ComplexityAssessment struct
    -- Progress
    review_rounds_completed INTEGER NOT NULL DEFAULT 0,
    workspace_path          TEXT NOT NULL,                     -- .argus/collab/session-{id}/
    -- Timestamps
    created_at              TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at            TIMESTAMPTZ
);

CREATE INDEX idx_collab_sessions_task ON collaboration_sessions (task_id);
CREATE INDEX idx_collab_sessions_project ON collaboration_sessions (project_id);
CREATE INDEX idx_collab_sessions_phase ON collaboration_sessions (phase)
    WHERE phase NOT IN ('completed', 'failed');
```

## 31. Collaboration Artifacts

```sql
-- ============================================================
-- COLLABORATION ARTIFACTS
-- Files produced by each role during a collaboration session
-- (architecture.md, review-round-N.json, test-report.json, etc.)
-- ============================================================

CREATE TABLE collaboration_artifacts (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    session_id      UUID NOT NULL REFERENCES collaboration_sessions(id) ON DELETE CASCADE,
    role            collaboration_role NOT NULL,
    artifact_type   TEXT NOT NULL,                 -- architecture_doc, review_report, test_report, etc.
    file_path       TEXT NOT NULL,                 -- Relative path within session workspace
    size_bytes      BIGINT NOT NULL DEFAULT 0,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_collab_artifacts_session ON collaboration_artifacts (session_id);
```

## 32. Collaboration Reviews

```sql
-- ============================================================
-- COLLABORATION REVIEWS
-- Individual review round records for audit trail and learning.
-- ============================================================

CREATE TABLE collaboration_reviews (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    session_id      UUID NOT NULL REFERENCES collaboration_sessions(id) ON DELETE CASCADE,
    reviewer_role   collaboration_role NOT NULL,
    reviewer_worker_id UUID REFERENCES workers(id) ON DELETE SET NULL,
    round           INTEGER NOT NULL,
    verdict         TEXT NOT NULL,                 -- approve, request_changes
    issues          JSONB NOT NULL DEFAULT '[]',   -- Array of issue objects
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_collab_reviews_session ON collaboration_reviews (session_id);
CREATE INDEX idx_collab_reviews_verdict ON collaboration_reviews (verdict);
```

## 33. Helper Functions

```sql
-- ============================================================
-- HELPER FUNCTIONS
-- ============================================================

-- Trigger to auto-update updated_at columns
CREATE OR REPLACE FUNCTION update_updated_at()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Apply the trigger to all tables with updated_at
CREATE TRIGGER trg_projects_updated_at
    BEFORE UPDATE ON projects FOR EACH ROW EXECUTE FUNCTION update_updated_at();
CREATE TRIGGER trg_milestones_updated_at
    BEFORE UPDATE ON milestones FOR EACH ROW EXECUTE FUNCTION update_updated_at();
CREATE TRIGGER trg_tasks_updated_at
    BEFORE UPDATE ON tasks FOR EACH ROW EXECUTE FUNCTION update_updated_at();
CREATE TRIGGER trg_mcp_servers_updated_at
    BEFORE UPDATE ON mcp_servers FOR EACH ROW EXECUTE FUNCTION update_updated_at();
CREATE TRIGGER trg_skills_updated_at
    BEFORE UPDATE ON skills FOR EACH ROW EXECUTE FUNCTION update_updated_at();
CREATE TRIGGER trg_env_configs_updated_at
    BEFORE UPDATE ON environment_configs FOR EACH ROW EXECUTE FUNCTION update_updated_at();
CREATE TRIGGER trg_owner_profiles_updated_at
    BEFORE UPDATE ON owner_profiles FOR EACH ROW EXECUTE FUNCTION update_updated_at();
CREATE TRIGGER trg_llm_providers_updated_at
    BEFORE UPDATE ON llm_providers FOR EACH ROW EXECUTE FUNCTION update_updated_at();
CREATE TRIGGER trg_collab_sessions_updated_at
    BEFORE UPDATE ON collaboration_sessions FOR EACH ROW EXECUTE FUNCTION update_updated_at();

-- Memory decay function (run daily via pg_cron or scheduler)
CREATE OR REPLACE FUNCTION decay_memories()
RETURNS INTEGER AS $$
DECLARE
    affected INTEGER;
BEGIN
    -- Reduce importance based on decay_rate and time since last access
    UPDATE memories
    SET importance = GREATEST(
        0.0,
        importance - decay_rate * EXTRACT(EPOCH FROM (NOW() - COALESCE(last_accessed, created_at))) / 86400.0
    )
    WHERE layer IN ('short_term', 'long_term')
      AND importance > 0.0;

    GET DIAGNOSTICS affected = ROW_COUNT;

    -- Delete expired memories
    DELETE FROM memories
    WHERE expires_at IS NOT NULL AND expires_at < NOW();

    -- Delete memories with importance below threshold (except permanent)
    DELETE FROM memories
    WHERE layer != 'permanent'
      AND importance < 0.01
      AND last_accessed < NOW() - INTERVAL '7 days';

    RETURN affected;
END;
$$ LANGUAGE plpgsql;

-- Promote important short-term memories to long-term
CREATE OR REPLACE FUNCTION promote_memories()
RETURNS INTEGER AS $$
DECLARE
    affected INTEGER;
BEGIN
    UPDATE memories
    SET layer = 'long_term',
        expires_at = NULL
    WHERE layer = 'short_term'
      AND importance > 0.7
      AND access_count >= 3;

    GET DIAGNOSTICS affected = ROW_COUNT;
    RETURN affected;
END;
$$ LANGUAGE plpgsql;

-- Clean up old deployment artifacts
CREATE OR REPLACE FUNCTION cleanup_old_artifacts(max_age_days INTEGER DEFAULT 30)
RETURNS INTEGER AS $$
DECLARE
    affected INTEGER;
BEGIN
    DELETE FROM deployment_artifacts
    WHERE created_at < NOW() - (max_age_days || ' days')::INTERVAL;

    GET DIAGNOSTICS affected = ROW_COUNT;
    RETURN affected;
END;
$$ LANGUAGE plpgsql;

-- Refresh LLM usage materialized view
CREATE OR REPLACE FUNCTION refresh_llm_usage_daily()
RETURNS VOID AS $$
BEGIN
    REFRESH MATERIALIZED VIEW CONCURRENTLY llm_usage_daily;
END;
$$ LANGUAGE plpgsql;
```

## 34. Table Summary

| Table | Purpose | Referenced By |
|-------|---------|---------------|
| `projects` | All managed projects | tasks, milestones, memories, deployments, etc. |
| `milestones` | Project milestones | tasks |
| `tasks` | Atomic work units | task_dependencies, task_checkpoints, episodes |
| `task_dependencies` | DAG edges between tasks | — |
| `task_checkpoints` | Crash recovery state | — |
| `memories` | Four-layer memory with embeddings | memory_relations |
| `memory_relations` | Links between memories | — |
| `conversation_summaries` | Condensed conversation history | — |
| `llm_usage` | Token and cost tracking | — |
| `llm_usage_daily` | Aggregated daily stats (materialized view) | — |
| `workers` | Worker pool state | tasks, episodes |
| `deployments` | Deployment records | deployment_snapshots, deployment_artifacts |
| `deployment_snapshots` | Pre-deploy state for rollback | — |
| `deployment_artifacts` | Build outputs | — |
| `environment_configs` | Per-project deploy targets | — |
| `mcp_servers` | MCP server registry | — |
| `skills` | Skills registry | — |
| `key_vault` | Encrypted secrets | — |
| `identities` | User accounts | identity_platforms, owner_profiles |
| `identity_platforms` | Platform-specific user IDs | — |
| `config` | Key-value configuration | — |
| `config_audit_log` | Config change history | — |
| `audit_log` | System-wide security log | — |
| `owner_profiles` | Owner preferences | — |
| `evolution_history` | Self-modification records | — |
| `episodes` | Task execution details | — |
| `notification_buffer` | Queued notifications | — |
| `web_sessions` | Dashboard auth sessions | — |
| `llm_providers` | LLM provider configuration | — |
| `database_backups` | Backup metadata | — |
| `collaboration_sessions` | Role-based collaboration session tracking | collaboration_artifacts, collaboration_reviews |
| `collaboration_artifacts` | Files produced by each collaboration role | — |
| `collaboration_reviews` | Review round records (verdict, issues) | — |

**Total: 16 ENUMs, 32 tables, 1 materialized view, 5 helper functions, 9 triggers**

## 35. Migration Strategy

Migrations are managed via `sqlx-migrate` (sqlx's built-in migration tool):

```
migrations/
├── 0001_extensions.sql          -- uuid-ossp, pgvector, pg_trgm
├── 0002_custom_types.sql        -- All ENUM types
├── 0003_projects.sql            -- projects table
├── 0004_milestones.sql          -- milestones table
├── 0005_tasks.sql               -- tasks, task_dependencies, task_checkpoints
├── 0006_memories.sql            -- memories, memory_relations, conversation_summaries
├── 0007_llm.sql                 -- llm_usage, llm_providers, llm_usage_daily
├── 0008_workers.sql             -- workers table
├── 0009_deployments.sql         -- deployments, snapshots, artifacts, env_configs
├── 0010_mcp_skills.sql          -- mcp_servers, skills
├── 0011_security.sql            -- key_vault, identities, identity_platforms, web_sessions
├── 0012_config.sql              -- config, config_audit_log
├── 0013_audit.sql               -- audit_log
├── 0014_owner.sql               -- owner_profiles
├── 0015_evolution.sql           -- evolution_history
├── 0016_episodes.sql            -- episodes
├── 0017_notifications.sql       -- notification_buffer
├── 0018_backups.sql             -- database_backups
├── 0019_functions.sql           -- helper functions, triggers
├── 0020_indexes.sql             -- additional indexes (if not inline)
└── 0021_collaboration.sql       -- collaboration_sessions, collaboration_artifacts, collaboration_reviews + ENUMs
```

Each migration file is idempotent and uses `IF NOT EXISTS` where possible.
