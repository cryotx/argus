-- 0012: Config store, Config audit log

CREATE TABLE IF NOT EXISTS config (
    path            TEXT PRIMARY KEY,
    value           JSONB NOT NULL,
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_config_path_prefix ON config USING btree (path text_pattern_ops);

-- Config Audit Log
CREATE TABLE IF NOT EXISTS config_audit_log (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    timestamp       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    action          TEXT NOT NULL,
    domain          TEXT NOT NULL,
    old_value       JSONB,
    new_value       JSONB,
    description     TEXT NOT NULL,
    source          JSONB NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_config_audit_timestamp ON config_audit_log (timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_config_audit_domain ON config_audit_log (domain);
