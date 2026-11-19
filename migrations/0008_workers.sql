-- 0008: Workers table

CREATE TABLE IF NOT EXISTS workers (
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
    sandbox_type    TEXT NOT NULL DEFAULT 'directory',
    workspace_path  TEXT,
    container_id    TEXT,
    -- Statistics
    tasks_completed INTEGER NOT NULL DEFAULT 0,
    tasks_failed    INTEGER NOT NULL DEFAULT 0,
    total_uptime_seconds BIGINT NOT NULL DEFAULT 0,
    -- Timestamps
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_heartbeat  TIMESTAMPTZ,
    last_task_at    TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_workers_state ON workers (state);
CREATE INDEX IF NOT EXISTS idx_workers_type ON workers (worker_type);
