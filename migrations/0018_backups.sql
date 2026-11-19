-- 0018: Database backups

CREATE TABLE IF NOT EXISTS database_backups (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    file_path       TEXT NOT NULL,
    size_bytes      BIGINT NOT NULL,
    trigger         TEXT NOT NULL,
    status          TEXT NOT NULL,
    error           TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_database_backups_time ON database_backups (created_at DESC);
