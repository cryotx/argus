-- 0007: LLM usage tracking, providers, daily materialized view

CREATE TABLE IF NOT EXISTS llm_usage (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    -- Provider & model
    provider        TEXT NOT NULL,
    model           TEXT NOT NULL,
    model_class     TEXT NOT NULL,
    -- Token counts
    input_tokens    INTEGER NOT NULL,
    output_tokens   INTEGER NOT NULL,
    total_tokens    INTEGER NOT NULL,
    -- Cost
    cost_usd        REAL NOT NULL DEFAULT 0.0,
    -- Context
    project_id      UUID REFERENCES projects(id) ON DELETE SET NULL,
    task_id         UUID REFERENCES tasks(id) ON DELETE SET NULL,
    purpose         TEXT,
    -- Performance
    latency_ms      INTEGER,
    success         BOOLEAN NOT NULL DEFAULT true,
    error           TEXT,
    -- Timestamps
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_llm_usage_provider ON llm_usage (provider, created_at);
CREATE INDEX IF NOT EXISTS idx_llm_usage_project ON llm_usage (project_id) WHERE project_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_llm_usage_created ON llm_usage (created_at);
CREATE INDEX IF NOT EXISTS idx_llm_usage_model ON llm_usage (model);

-- LLM Providers
CREATE TABLE IF NOT EXISTS llm_providers (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    name            TEXT NOT NULL UNIQUE,
    display_name    TEXT NOT NULL,
    -- Configuration
    base_url        TEXT,
    api_key_name    TEXT,
    models          JSONB NOT NULL DEFAULT '[]',
    model_class_map JSONB NOT NULL DEFAULT '{}',
    -- Budget
    monthly_budget_usd REAL,
    current_month_usage REAL NOT NULL DEFAULT 0.0,
    -- Rate limiting
    rpm_limit       INTEGER,
    tpm_limit       INTEGER,
    -- Failover
    priority        INTEGER NOT NULL DEFAULT 100,
    enabled         BOOLEAN NOT NULL DEFAULT true,
    -- Key rotation
    api_keys        JSONB NOT NULL DEFAULT '[]',
    current_key_index INTEGER NOT NULL DEFAULT 0,
    -- Timestamps
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_llm_providers_priority ON llm_providers (priority) WHERE enabled = true;

-- Materialized view for cost reporting
CREATE MATERIALIZED VIEW IF NOT EXISTS llm_usage_daily AS
SELECT
    DATE_TRUNC('day', created_at) AS day,
    provider,
    model,
    project_id,
    COUNT(*) AS request_count,
    SUM(input_tokens) AS total_input_tokens,
    SUM(output_tokens) AS total_output_tokens,
    SUM(cost_usd) AS total_cost_usd,
    AVG(latency_ms) AS avg_latency_ms,
    COUNT(*) FILTER (WHERE success = true) AS success_count,
    COUNT(*) FILTER (WHERE success = false) AS failure_count
FROM llm_usage
GROUP BY day, provider, model, project_id;

CREATE UNIQUE INDEX IF NOT EXISTS idx_llm_usage_daily_pk
    ON llm_usage_daily (day, provider, model, project_id);
