-- 0016: Episodes (task execution episodes)

CREATE TABLE IF NOT EXISTS episodes (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    task_id         UUID NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    worker_id       UUID REFERENCES workers(id) ON DELETE SET NULL,
    -- Execution details
    attempt         INTEGER NOT NULL DEFAULT 1,
    -- Input / output
    input           JSONB NOT NULL DEFAULT '{}',
    output          JSONB,
    -- Status
    status          TEXT NOT NULL,
    error           TEXT,
    -- Performance
    tokens_used     INTEGER,
    cost_usd        REAL,
    -- Timestamps
    started_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at    TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_episodes_task ON episodes (task_id, attempt);
CREATE INDEX IF NOT EXISTS idx_episodes_worker ON episodes (worker_id) WHERE worker_id IS NOT NULL;
