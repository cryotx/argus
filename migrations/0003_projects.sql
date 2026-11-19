-- 0003: Projects table
CREATE TABLE IF NOT EXISTS projects (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    name            TEXT NOT NULL UNIQUE,
    description     TEXT,
    -- Git
    git_repo_url    TEXT,
    git_local_path  TEXT NOT NULL,
    default_branch  TEXT NOT NULL DEFAULT 'main',
    -- Build & test
    language        TEXT,
    framework       TEXT,
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

CREATE INDEX IF NOT EXISTS idx_projects_active ON projects (active) WHERE active = true;
CREATE INDEX IF NOT EXISTS idx_projects_name ON projects USING gin (name gin_trgm_ops);
