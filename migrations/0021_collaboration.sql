-- 0021: Collaboration sessions, artifacts, reviews

CREATE TABLE IF NOT EXISTS collaboration_sessions (
    id                      UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    task_id                 UUID NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    project_id              UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    -- Collaboration config
    mode                    collaboration_mode NOT NULL,
    phase                   collaboration_phase NOT NULL DEFAULT 'architecture',
    -- Structured data
    role_assignments        JSONB NOT NULL DEFAULT '{}',
    review_policy           JSONB NOT NULL DEFAULT '{}',
    complexity_assessment   JSONB NOT NULL DEFAULT '{}',
    -- Progress
    review_rounds_completed INTEGER NOT NULL DEFAULT 0,
    workspace_path          TEXT NOT NULL,
    -- Timestamps
    created_at              TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at            TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_collab_sessions_task ON collaboration_sessions (task_id);
CREATE INDEX IF NOT EXISTS idx_collab_sessions_project ON collaboration_sessions (project_id);
CREATE INDEX IF NOT EXISTS idx_collab_sessions_phase ON collaboration_sessions (phase)
    WHERE phase NOT IN ('completed', 'failed');

-- Collaboration Artifacts
CREATE TABLE IF NOT EXISTS collaboration_artifacts (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    session_id      UUID NOT NULL REFERENCES collaboration_sessions(id) ON DELETE CASCADE,
    role            collaboration_role NOT NULL,
    artifact_type   TEXT NOT NULL,
    file_path       TEXT NOT NULL,
    size_bytes      BIGINT NOT NULL DEFAULT 0,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_collab_artifacts_session ON collaboration_artifacts (session_id);

-- Collaboration Reviews
CREATE TABLE IF NOT EXISTS collaboration_reviews (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    session_id      UUID NOT NULL REFERENCES collaboration_sessions(id) ON DELETE CASCADE,
    reviewer_role   collaboration_role NOT NULL,
    reviewer_worker_id UUID REFERENCES workers(id) ON DELETE SET NULL,
    round           INTEGER NOT NULL,
    verdict         TEXT NOT NULL,
    issues          JSONB NOT NULL DEFAULT '[]',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_collab_reviews_session ON collaboration_reviews (session_id);
CREATE INDEX IF NOT EXISTS idx_collab_reviews_verdict ON collaboration_reviews (verdict);

-- Trigger for collaboration sessions updated_at
CREATE TRIGGER trg_collab_sessions_updated_at
    BEFORE UPDATE ON collaboration_sessions FOR EACH ROW EXECUTE FUNCTION update_updated_at();
