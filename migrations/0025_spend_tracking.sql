-- 0025: Spend tracking and budget enforcement

-- Extend llm_usage for Agent execution tracking
ALTER TABLE IF EXISTS llm_usage
ADD COLUMN IF NOT EXISTS execution_id UUID,
ADD COLUMN IF NOT EXISTS attempt_number SMALLINT DEFAULT 1,
ADD COLUMN IF NOT EXISTS agent_type TEXT,
ADD COLUMN IF NOT EXISTS external_session_id TEXT,
ADD COLUMN IF NOT EXISTS duration_ms INTEGER;

-- Create unique index for idempotent spend tracking
-- Prevents double-counting same execution attempt
CREATE UNIQUE INDEX IF NOT EXISTS idx_llm_usage_execution_idempotent
ON llm_usage (execution_id, attempt_number, task_id)
WHERE execution_id IS NOT NULL;

-- Create index for daily spend queries (performance-critical).
--
-- NOTE: this was originally `(DATE_TRUNC('day', created_at), project_id)`,
-- which Postgres rejects — `DATE_TRUNC(text, timestamptz)` is STABLE, not
-- IMMUTABLE (it depends on the session timezone), and index expressions
-- must be IMMUTABLE. That made every `argus run` fail at startup applying
-- this migration, so nothing past migration 24 (including V0.38's
-- `task_workspaces`) ever actually reached the database. A plain
-- `(project_id, created_at)` index serves the same "spend for this project,
-- recent first" queries without the expression.
CREATE INDEX IF NOT EXISTS idx_llm_usage_created_cost
ON llm_usage (project_id, created_at)
INCLUDE (cost_usd);

-- Budget state table (persistent, replaces in-memory BudgetState)
CREATE TABLE IF NOT EXISTS budget_ledger (
    id                  BIGSERIAL PRIMARY KEY,
    -- Scope
    scope_type          TEXT NOT NULL CHECK (scope_type IN ('global', 'project', 'provider')),
    scope_id            UUID,  -- project_id if scope_type='project'
    scope_name          TEXT,  -- provider name if scope_type='provider'

    -- Budget definition
    daily_limit_usd     REAL,
    monthly_limit_usd   REAL,

    -- Current state (updated by triggers or background job)
    today_spent_usd     REAL NOT NULL DEFAULT 0.0,
    month_spent_usd     REAL NOT NULL DEFAULT 0.0,
    last_reset_at       TIMESTAMPTZ,

    -- Auditing
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    UNIQUE(scope_type, scope_id, scope_name)
);

CREATE INDEX IF NOT EXISTS idx_budget_ledger_scope
ON budget_ledger (scope_type, scope_id);

-- Budget decision log (for audit trail)
CREATE TABLE IF NOT EXISTS budget_decisions (
    id                  UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    task_id             UUID REFERENCES tasks(id) ON DELETE CASCADE,
    execution_id        UUID,
    decision            TEXT NOT NULL CHECK (decision IN ('allowed', 'blocked')),
    reason              TEXT,
    -- State at decision time
    budget_scope        TEXT,
    spent_today_usd     REAL,
    limit_daily_usd     REAL,
    spent_month_usd     REAL,
    limit_monthly_usd   REAL,
    -- Timestamps
    checked_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_budget_decisions_task
ON budget_decisions (task_id, checked_at DESC);

CREATE INDEX IF NOT EXISTS idx_budget_decisions_execution
ON budget_decisions (execution_id) WHERE execution_id IS NOT NULL;

-- Update materialized view to include agent tracking
DROP MATERIALIZED VIEW IF EXISTS llm_usage_daily CASCADE;

CREATE MATERIALIZED VIEW llm_usage_daily AS
SELECT
    DATE_TRUNC('day', created_at)::DATE AS day,
    provider,
    model,
    agent_type,
    project_id,
    COUNT(*) AS request_count,
    COUNT(DISTINCT execution_id) FILTER (WHERE execution_id IS NOT NULL) AS execution_count,
    SUM(input_tokens) AS total_input_tokens,
    SUM(output_tokens) AS total_output_tokens,
    SUM(cost_usd) AS total_cost_usd,
    AVG(latency_ms) AS avg_latency_ms,
    AVG(duration_ms) AS avg_duration_ms,
    COUNT(*) FILTER (WHERE success = true) AS success_count,
    COUNT(*) FILTER (WHERE success = false) AS failure_count
FROM llm_usage
GROUP BY day, provider, model, agent_type, project_id;

CREATE UNIQUE INDEX IF NOT EXISTS idx_llm_usage_daily_pk
    ON llm_usage_daily (day, provider, model, agent_type, project_id);

-- Refreshable view for current daily spend (cache for speed)
CREATE OR REPLACE VIEW current_daily_spend AS
SELECT
    project_id,
    SUM(cost_usd) FILTER (
        WHERE DATE_TRUNC('day', created_at) = DATE_TRUNC('day', NOW())
    ) AS today_usd,
    SUM(cost_usd) FILTER (
        WHERE DATE_TRUNC('month', created_at) = DATE_TRUNC('month', NOW())
    ) AS month_usd
FROM llm_usage
WHERE success = true
GROUP BY project_id;
