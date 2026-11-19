-- 0032: Planner Harness / Protocol (V0.43)
--
-- A provider-neutral planning protocol. A Planner (a web model, a future API
-- provider, or a test fake) produces a structured *proposal*; ARGUS validates,
-- diffs, and only mutates Project State on explicit approval + apply. Nothing
-- here names a provider, model, auth profile, or quota — that is V0.44.

-- ============================================================
-- Minimal optimistic-concurrency revisions, scoped per project
-- ============================================================
CREATE TABLE IF NOT EXISTS project_revisions (
    project_id       UUID PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
    state_revision   BIGINT NOT NULL DEFAULT 0,
    roadmap_revision BIGINT NOT NULL DEFAULT 0,
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS planning_requests (
    planning_request_id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    project_id          UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,

    planning_kind       TEXT NOT NULL,
    role                TEXT NOT NULL,
    purpose             TEXT NOT NULL,

    -- Bound context (never copied as an opaque blob): the planner's input is
    -- exactly the ContextPacket with this id/hash/compiler version.
    context_packet_ref  UUID,
    context_hash        TEXT NOT NULL,
    compiler_version    TEXT NOT NULL,
    work_item_id        UUID REFERENCES work_items(work_item_id) ON DELETE SET NULL,

    task_profile_ref    UUID,
    routing_decision_ref UUID,

    state_revision      BIGINT NOT NULL DEFAULT 0,
    roadmap_revision    BIGINT NOT NULL DEFAULT 0,

    objective           TEXT NOT NULL,
    constraints         JSONB NOT NULL DEFAULT '[]',
    requested_outputs   JSONB NOT NULL DEFAULT '[]',

    protocol_version    TEXT NOT NULL,
    request_hash        TEXT NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    UNIQUE (project_id, request_hash)
);

CREATE INDEX IF NOT EXISTS idx_planning_requests_project
    ON planning_requests (project_id, created_at DESC);

CREATE TABLE IF NOT EXISTS planning_results (
    planning_result_id  UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    planning_request_id UUID NOT NULL REFERENCES planning_requests(planning_request_id) ON DELETE CASCADE,
    project_id          UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,

    status              TEXT NOT NULL,
    summary             TEXT,
    result_json         JSONB NOT NULL,

    planner_metadata    JSONB NOT NULL DEFAULT '{}',
    protocol_version    TEXT NOT NULL,
    context_hash        TEXT NOT NULL,
    request_hash        TEXT NOT NULL,
    result_hash         TEXT NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    -- Duplicate import of the same result must not create a second result.
    UNIQUE (project_id, result_hash)
);

CREATE INDEX IF NOT EXISTS idx_planning_results_request
    ON planning_results (planning_request_id, created_at DESC);

CREATE TABLE IF NOT EXISTS plan_proposals (
    proposal_id         UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    planning_result_id  UUID NOT NULL REFERENCES planning_results(planning_result_id) ON DELETE CASCADE,
    planning_request_id UUID NOT NULL REFERENCES planning_requests(planning_request_id) ON DELETE CASCADE,
    project_id          UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,

    status              TEXT NOT NULL DEFAULT 'draft',
    proposal_json       JSONB NOT NULL,
    validation_errors   JSONB NOT NULL DEFAULT '[]',

    context_hash        TEXT NOT NULL,
    request_hash        TEXT NOT NULL,
    result_hash         TEXT NOT NULL,

    -- Revisions at validation/approval time; checked again inside the apply
    -- transaction to detect stale proposals.
    state_revision      BIGINT NOT NULL DEFAULT 0,
    roadmap_revision    BIGINT NOT NULL DEFAULT 0,

    applied_at          TIMESTAMPTZ,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    -- One proposal per result.
    UNIQUE (project_id, result_hash)
);

CREATE INDEX IF NOT EXISTS idx_plan_proposals_project
    ON plan_proposals (project_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_plan_proposals_status
    ON plan_proposals (project_id, status);

CREATE TRIGGER trg_plan_proposals_updated_at
    BEFORE UPDATE ON plan_proposals FOR EACH ROW EXECUTE FUNCTION update_updated_at();

-- Proposal lifecycle / approval audit trail.
CREATE TABLE IF NOT EXISTS proposal_lifecycle_events (
    event_id    UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    proposal_id UUID NOT NULL REFERENCES plan_proposals(proposal_id) ON DELETE CASCADE,
    project_id  UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    event_type  TEXT NOT NULL,
    detail      TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_proposal_lifecycle_proposal
    ON proposal_lifecycle_events (proposal_id, created_at ASC);
