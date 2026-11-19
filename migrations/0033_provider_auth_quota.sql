-- 0033: Provider / Auth / Subscription / Quota (V0.44)
--
-- Maps V0.42 capability requirements onto concrete runtime providers, models,
-- auth profiles, and quotas. Providers, models, auth, subscriptions, and API
-- billing are separate concepts. No raw secret is ever stored here — only a
-- *reference* (locator) to a credential owned by the environment, OS keyring,
-- or a CLI.

CREATE TABLE IF NOT EXISTS providers (
    provider_id     TEXT PRIMARY KEY,
    enabled         BOOLEAN NOT NULL DEFAULT TRUE,
    priority        INTEGER NOT NULL DEFAULT 100,
    config          JSONB NOT NULL DEFAULT '{}',
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS auth_profiles (
    auth_profile_id  UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    provider_id      TEXT NOT NULL,
    auth_kind        TEXT NOT NULL,
    account_label    TEXT,
    workspace_label  TEXT,
    status           TEXT NOT NULL DEFAULT 'unknown',
    -- A locator only (e.g. `env:OPENAI_API_KEY`, `cli:claude`), never a value.
    secret_reference TEXT,
    credential_source TEXT,
    expires_at       TIMESTAMPTZ,
    last_verified_at TIMESTAMPTZ,
    metadata         JSONB NOT NULL DEFAULT '{}',
    created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_auth_profiles_provider ON auth_profiles (provider_id);

CREATE TABLE IF NOT EXISTS quota_snapshots (
    quota_snapshot_id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    provider_id       TEXT NOT NULL,
    auth_profile_id   UUID REFERENCES auth_profiles(auth_profile_id) ON DELETE CASCADE,
    quota_kind        TEXT NOT NULL,
    availability      TEXT NOT NULL,
    remaining         DOUBLE PRECISION,
    quota_limit       DOUBLE PRECISION,
    unit              TEXT NOT NULL DEFAULT 'unknown',
    reset_at          TIMESTAMPTZ,
    observed_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    source            TEXT NOT NULL,
    confidence        TEXT NOT NULL,
    raw_metadata      JSONB NOT NULL DEFAULT '{}'
);

CREATE INDEX IF NOT EXISTS idx_quota_snapshots_auth
    ON quota_snapshots (auth_profile_id, observed_at DESC);

CREATE TABLE IF NOT EXISTS project_provider_policies (
    project_id          UUID PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
    allowed_providers   JSONB NOT NULL DEFAULT '[]',
    denied_providers    JSONB NOT NULL DEFAULT '[]',
    subscription_first  BOOLEAN NOT NULL DEFAULT TRUE,
    api_fallback        TEXT NOT NULL DEFAULT 'automatic_within_budget',
    max_tier            TEXT,
    preferred_runtime   TEXT,
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS provider_routing_selections (
    selection_id        UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    project_id          UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    routing_decision_id UUID,
    provider_id         TEXT NOT NULL,
    runtime_provider    TEXT NOT NULL,
    model_id            TEXT,
    auth_profile_id     UUID,
    billing_mode        TEXT NOT NULL,
    execution_mode      TEXT NOT NULL,
    required_tier       TEXT NOT NULL,
    reasoning_effort    TEXT NOT NULL,
    policy              JSONB NOT NULL DEFAULT '{}',
    quota_snapshot_ref  UUID,
    reasons             JSONB NOT NULL DEFAULT '[]',
    router_version      TEXT NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_provider_selections_project
    ON provider_routing_selections (project_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_provider_selections_routing
    ON provider_routing_selections (routing_decision_id);

CREATE TABLE IF NOT EXISTS provider_routing_attempts (
    attempt_id          UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    project_id          UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    routing_decision_id UUID,
    candidate_provider  TEXT NOT NULL,
    candidate_model     TEXT,
    outcome             TEXT NOT NULL,
    reason              TEXT NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_provider_attempts_project
    ON provider_routing_attempts (project_id, created_at DESC);
