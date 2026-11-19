-- Persist external idempotency keys across scheduler restarts.
CREATE TABLE IF NOT EXISTS task_request_keys (
    request_key TEXT PRIMARY KEY,
    task_id UUID NOT NULL UNIQUE REFERENCES tasks(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

