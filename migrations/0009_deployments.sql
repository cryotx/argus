-- 0009: Deployments, snapshots, artifacts, environment configs

CREATE TABLE IF NOT EXISTS deployments (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    project_id      UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    -- Environment & target
    environment     environment NOT NULL,
    target_type     TEXT NOT NULL,
    -- Trigger
    trigger_type    JSONB NOT NULL,
    -- Git info
    git_ref         TEXT NOT NULL,
    git_branch      TEXT NOT NULL,
    -- Status
    status          deployment_status NOT NULL DEFAULT 'queued',
    -- Pipeline stages
    stages_json     JSONB NOT NULL DEFAULT '[]',
    -- Rollback
    snapshot_id     UUID,
    rolled_back     BOOLEAN NOT NULL DEFAULT false,
    -- Config snapshot
    config_json     JSONB NOT NULL DEFAULT '{}',
    -- Metadata
    metadata        JSONB NOT NULL DEFAULT '{}',
    -- Timestamps
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    finished_at     TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_deployments_project ON deployments (project_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_deployments_status ON deployments (status);
CREATE INDEX IF NOT EXISTS idx_deployments_env ON deployments (project_id, environment);

-- Deployment Snapshots (for rollback)
CREATE TABLE IF NOT EXISTS deployment_snapshots (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    deployment_id   UUID NOT NULL REFERENCES deployments(id) ON DELETE CASCADE,
    snapshot_type   TEXT NOT NULL,
    snapshot_data   JSONB NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_deployment_snapshots_deployment ON deployment_snapshots (deployment_id);

-- Deployment Artifacts
CREATE TABLE IF NOT EXISTS deployment_artifacts (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    deployment_id   UUID NOT NULL REFERENCES deployments(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    path            TEXT NOT NULL,
    size_bytes      BIGINT NOT NULL,
    checksum_sha256 TEXT NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_deployment_artifacts_deployment ON deployment_artifacts (deployment_id);

-- Environment Configs (per-project deployment targets)
CREATE TABLE IF NOT EXISTS environment_configs (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    project_id      UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    environment     TEXT NOT NULL,
    config_data     JSONB NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    UNIQUE (project_id, environment)
);

CREATE INDEX IF NOT EXISTS idx_env_configs_project ON environment_configs (project_id);
