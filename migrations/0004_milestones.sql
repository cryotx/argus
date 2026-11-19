-- 0004: Milestones table
CREATE TABLE IF NOT EXISTS milestones (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    project_id      UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    description     TEXT,
    -- Ordering
    sequence        INTEGER NOT NULL,
    -- Status
    status          TEXT NOT NULL DEFAULT 'pending',
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

CREATE INDEX IF NOT EXISTS idx_milestones_project ON milestones (project_id);
