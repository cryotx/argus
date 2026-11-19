-- 0005: Tasks, task_dependencies, task_checkpoints

CREATE TABLE IF NOT EXISTS tasks (
    id                  UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    project_id          UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    milestone_id        UUID REFERENCES milestones(id) ON DELETE SET NULL,
    -- Task definition
    title               TEXT NOT NULL,
    description         TEXT,
    task_type           TEXT NOT NULL,
    -- Execution
    state               task_state NOT NULL DEFAULT 'pending',
    priority            task_priority NOT NULL DEFAULT 'medium',
    effective_priority  REAL NOT NULL DEFAULT 0.0,
    assigned_worker_id  UUID,
    -- Retry logic
    retry_count         INTEGER NOT NULL DEFAULT 0,
    max_retries         INTEGER NOT NULL DEFAULT 3,
    -- Time tracking
    timeout_seconds     INTEGER NOT NULL DEFAULT 3600,
    estimated_seconds   INTEGER,
    -- Results
    output              JSONB,
    error               TEXT,
    -- Context
    context             JSONB NOT NULL DEFAULT '{}',
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

CREATE INDEX IF NOT EXISTS idx_tasks_project ON tasks (project_id);
CREATE INDEX IF NOT EXISTS idx_tasks_state ON tasks (state);
CREATE INDEX IF NOT EXISTS idx_tasks_priority ON tasks (effective_priority DESC);
CREATE INDEX IF NOT EXISTS idx_tasks_milestone ON tasks (milestone_id) WHERE milestone_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_tasks_worker ON tasks (assigned_worker_id) WHERE assigned_worker_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_tasks_ready ON tasks (effective_priority DESC) WHERE state = 'ready';

-- Task Dependencies (DAG edges)
CREATE TABLE IF NOT EXISTS task_dependencies (
    task_id         UUID NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    depends_on_id   UUID NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    PRIMARY KEY (task_id, depends_on_id),
    CHECK (task_id != depends_on_id)
);

CREATE INDEX IF NOT EXISTS idx_task_deps_depends_on ON task_dependencies (depends_on_id);

-- Task Checkpoints (for crash recovery)
CREATE TABLE IF NOT EXISTS task_checkpoints (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    task_id         UUID NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    -- Checkpoint data
    state           task_state NOT NULL,
    progress        REAL NOT NULL DEFAULT 0.0,
    worker_output   JSONB,
    context         JSONB NOT NULL DEFAULT '{}',
    -- Timestamps
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_task_checkpoints_task ON task_checkpoints (task_id, created_at DESC);
