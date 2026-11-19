-- 0031: Task Intelligence & Model Routing (V0.42)
--
-- Deterministic, explainable task classification and capability routing.
-- No model/provider is named here: a TaskProfile produces a *capability tier*
-- (T0..T3) and a RoutingDecision states a minimum tier; V0.44 later maps tiers
-- to concrete providers/models. Additive on top of V0.39–V0.41.

CREATE TABLE IF NOT EXISTS task_profiles (
    profile_id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    project_id              UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    work_item_id            UUID REFERENCES work_items(work_item_id) ON DELETE SET NULL,
    task_id                 UUID REFERENCES tasks(id) ON DELETE SET NULL,

    task_kind               TEXT NOT NULL,
    secondary_flags         JSONB NOT NULL DEFAULT '[]',

    -- Normalized signals in 0.0..=1.0.
    complexity_score        REAL NOT NULL,
    risk_score              REAL NOT NULL,
    ambiguity_score         REAL NOT NULL,
    reasoning_score         REAL NOT NULL,
    scope_score             REAL NOT NULL,
    verification_score      REAL NOT NULL,
    context_score           REAL NOT NULL,
    reversibility_score     REAL NOT NULL,

    recommended_tier        TEXT NOT NULL,
    verification_requirement TEXT NOT NULL,
    required_capabilities   JSONB NOT NULL DEFAULT '[]',
    confidence              REAL NOT NULL,
    reasons                 JSONB NOT NULL DEFAULT '[]',

    classifier_version      TEXT NOT NULL,
    -- Deterministic hash of the normalized classification input; makes a
    -- repeated classify idempotent without re-writing a profile.
    input_hash              TEXT NOT NULL,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    UNIQUE (project_id, input_hash)
);

CREATE INDEX IF NOT EXISTS idx_task_profiles_project ON task_profiles (project_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_task_profiles_work_item
    ON task_profiles (work_item_id) WHERE work_item_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_task_profiles_kind ON task_profiles (project_id, task_kind);

CREATE TABLE IF NOT EXISTS routing_decisions (
    routing_id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    profile_id              UUID NOT NULL REFERENCES task_profiles(profile_id) ON DELETE CASCADE,
    project_id              UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    work_item_id            UUID REFERENCES work_items(work_item_id) ON DELETE SET NULL,
    task_id                 UUID REFERENCES tasks(id) ON DELETE SET NULL,

    role                    TEXT NOT NULL,
    purpose                 TEXT NOT NULL,

    required_tier           TEXT NOT NULL,
    preferred_tier          TEXT NOT NULL,
    required_capabilities   JSONB NOT NULL DEFAULT '[]',
    reasoning_effort        TEXT NOT NULL,
    context_requirement     TEXT NOT NULL,
    verification_requirement TEXT NOT NULL,
    retry_policy            JSONB NOT NULL DEFAULT '{}',
    escalation_policy       JSONB NOT NULL DEFAULT '{}',
    cost_preference         TEXT NOT NULL DEFAULT 'cheapest_sufficient',
    reasons                 JSONB NOT NULL DEFAULT '[]',

    router_version          TEXT NOT NULL,
    input_hash              TEXT NOT NULL,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    UNIQUE (project_id, input_hash)
);

CREATE INDEX IF NOT EXISTS idx_routing_decisions_project
    ON routing_decisions (project_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_routing_decisions_work_item
    ON routing_decisions (work_item_id) WHERE work_item_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS routing_outcomes (
    outcome_id      UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    routing_id      UUID NOT NULL REFERENCES routing_decisions(routing_id) ON DELETE CASCADE,
    project_id      UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    work_item_id    UUID REFERENCES work_items(work_item_id) ON DELETE SET NULL,
    task_id         UUID REFERENCES tasks(id) ON DELETE SET NULL,

    selected_tier   TEXT NOT NULL,
    success         BOOLEAN NOT NULL,
    failure_class   TEXT,
    attempts        INTEGER NOT NULL DEFAULT 1,
    duration_ms     INTEGER,
    artifact_id     UUID,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_routing_outcomes_project
    ON routing_outcomes (project_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_routing_outcomes_routing ON routing_outcomes (routing_id);
CREATE INDEX IF NOT EXISTS idx_routing_outcomes_work_item
    ON routing_outcomes (project_id, work_item_id) WHERE work_item_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS escalation_decisions (
    escalation_id   UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    routing_id      UUID REFERENCES routing_decisions(routing_id) ON DELETE CASCADE,
    project_id      UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,

    action          TEXT NOT NULL,
    from_tier       TEXT NOT NULL,
    to_tier         TEXT,
    failure_class   TEXT NOT NULL,
    reason          TEXT NOT NULL,
    retry_after     TIMESTAMPTZ,
    requires_human  BOOLEAN NOT NULL DEFAULT FALSE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_escalation_decisions_project
    ON escalation_decisions (project_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_escalation_decisions_routing ON escalation_decisions (routing_id);
