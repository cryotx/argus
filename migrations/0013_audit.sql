-- 0013: System audit log

CREATE TABLE IF NOT EXISTS audit_log (
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

CREATE INDEX IF NOT EXISTS idx_audit_log_timestamp ON audit_log (timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_audit_log_action ON audit_log USING gin (action);
CREATE INDEX IF NOT EXISTS idx_audit_log_actor ON audit_log USING gin (actor);
CREATE INDEX IF NOT EXISTS idx_audit_log_outcome ON audit_log (outcome);
