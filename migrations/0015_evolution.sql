-- 0015: Evolution history

CREATE TABLE IF NOT EXISTS evolution_history (
    id                  UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    description         TEXT NOT NULL,
    category            JSONB NOT NULL,
    status              JSONB NOT NULL,
    branch_name         TEXT NOT NULL,
    -- Changes
    changes_json        JSONB NOT NULL DEFAULT '[]',
    -- Test results
    test_results_json   JSONB,
    -- Approval
    approval_json       JSONB,
    -- Deployment info
    deployment_json     JSONB,
    -- Timestamps
    requested_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at        TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_evolution_history_status ON evolution_history USING gin (status);
CREATE INDEX IF NOT EXISTS idx_evolution_history_time ON evolution_history (requested_at DESC);
