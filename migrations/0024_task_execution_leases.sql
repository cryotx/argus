-- Stable logical execution identity prevents duplicate scheduler claims.
ALTER TABLE tasks ADD COLUMN IF NOT EXISTS execution_id UUID;
ALTER TABLE tasks ADD COLUMN IF NOT EXISTS resume_count INTEGER NOT NULL DEFAULT 0;

CREATE TABLE IF NOT EXISTS task_execution_attempts (
    execution_id UUID PRIMARY KEY,
    task_id UUID NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    attempt INTEGER NOT NULL,
    status TEXT NOT NULL,
    claimed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (task_id, attempt)
);

CREATE INDEX IF NOT EXISTS idx_task_execution_attempts_task
    ON task_execution_attempts (task_id, attempt DESC);
