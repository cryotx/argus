-- 0028: Project State / Memory foundation (V0.39)
--
-- Additive only. V0.38 tables (tasks, task_workspaces, task_artifacts,
-- llm_usage, milestones) are never altered destructively and never dropped.
-- This migration gives ARGUS a structured, durable source of truth for
-- project-level facts: what a project is, what it has completed, what is
-- active, what is blocked, and why. It is *not* conversation/vector memory.
--
--   Conversation = Evidence
--   Project State = Source of Truth

-- ============================================================
-- Extend projects with Project State fields
-- ============================================================
-- `name`, `active`, `priority`, and the Git columns stay exactly as V0.38
-- left them. `slug` is a stable external handle (bootstrap/API lookups);
-- `status` is the project lifecycle, deliberately distinct from task state.

ALTER TABLE projects ADD COLUMN IF NOT EXISTS slug TEXT;
ALTER TABLE projects ADD COLUMN IF NOT EXISTS status TEXT NOT NULL DEFAULT 'active';

-- Backfill existing rows with a deterministic, unique slug. Appending the
-- id prefix guarantees uniqueness even if two names slugify identically.
UPDATE projects
SET slug = lower(regexp_replace(name, '[^a-zA-Z0-9]+', '-', 'g'))
           || '-' || left(replace(id::text, '-', ''), 8)
WHERE slug IS NULL;

ALTER TABLE projects ALTER COLUMN slug SET NOT NULL;

ALTER TABLE projects
    ADD CONSTRAINT projects_status_check
    CHECK (status IN ('active', 'paused', 'archived', 'completed'));

CREATE UNIQUE INDEX IF NOT EXISTS idx_projects_slug ON projects (slug);
CREATE INDEX IF NOT EXISTS idx_projects_status ON projects (status);

-- ============================================================
-- Project Charter — the stable, long-lived definition of a project
-- ============================================================
CREATE TABLE IF NOT EXISTS project_charters (
    project_id       UUID PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
    vision           TEXT NOT NULL,
    goal             TEXT NOT NULL,
    success_criteria JSONB NOT NULL DEFAULT '[]',
    constraints      JSONB NOT NULL DEFAULT '[]',
    non_goals        JSONB NOT NULL DEFAULT '[]',
    -- Bumped only when charter content actually changes, so "the charter
    -- changed" is a queryable fact rather than an implicit edit.
    revision         INTEGER NOT NULL DEFAULT 1,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TRIGGER trg_project_charters_updated_at
    BEFORE UPDATE ON project_charters FOR EACH ROW EXECUTE FUNCTION update_updated_at();

-- ============================================================
-- Extend milestones with roadmap-tree fields
-- ============================================================
-- The table already exists (0004) but was never written by production code.
-- `name` remains the title column; we add only what V0.39 needs.
ALTER TABLE milestones ADD COLUMN IF NOT EXISTS parent_id UUID
    REFERENCES milestones(id) ON DELETE SET NULL;
ALTER TABLE milestones ADD COLUMN IF NOT EXISTS definition_of_done TEXT;
ALTER TABLE milestones ALTER COLUMN status SET DEFAULT 'planned';

ALTER TABLE milestones
    ADD CONSTRAINT milestones_status_check
    CHECK (status IN ('planned', 'active', 'blocked', 'deferred', 'completed', 'cancelled'));

CREATE INDEX IF NOT EXISTS idx_milestones_project_status
    ON milestones (project_id, status);
CREATE INDEX IF NOT EXISTS idx_milestones_parent
    ON milestones (parent_id) WHERE parent_id IS NOT NULL;

-- ============================================================
-- WorkItem — what needs to be achieved (distinct from Task)
-- ============================================================
-- WorkItem = what needs to be achieved.
-- Task     = one concrete execution attempt/work package.
-- One WorkItem may produce 0..N Tasks (see work_item_tasks).
CREATE TABLE IF NOT EXISTS work_items (
    work_item_id        UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    project_id          UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    milestone_id        UUID REFERENCES milestones(id) ON DELETE SET NULL,
    title               TEXT NOT NULL,
    description         TEXT,
    status              TEXT NOT NULL DEFAULT 'planned',
    priority            TEXT NOT NULL DEFAULT 'normal',
    definition_of_done  TEXT,
    source              TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at        TIMESTAMPTZ,

    -- ReportedComplete is never VerifiedComplete: an Agent claiming done is
    -- not the same as ARGUS having verified it.
    CHECK (status IN (
        'planned', 'active', 'blocked', 'deferred',
        'reported_complete', 'verified_complete', 'accepted', 'cancelled'
    )),
    CHECK (priority IN ('critical', 'urgent', 'high', 'normal', 'low', 'background'))
);

CREATE INDEX IF NOT EXISTS idx_work_items_project_status
    ON work_items (project_id, status);
CREATE INDEX IF NOT EXISTS idx_work_items_milestone
    ON work_items (milestone_id) WHERE milestone_id IS NOT NULL;

CREATE TRIGGER trg_work_items_updated_at
    BEFORE UPDATE ON work_items FOR EACH ROW EXECUTE FUNCTION update_updated_at();

-- WorkItem <-> Task association (0..N tasks per work item).
CREATE TABLE IF NOT EXISTS work_item_tasks (
    work_item_id UUID NOT NULL REFERENCES work_items(work_item_id) ON DELETE CASCADE,
    task_id      UUID NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (work_item_id, task_id)
);

CREATE INDEX IF NOT EXISTS idx_work_item_tasks_task ON work_item_tasks (task_id);

-- WorkItem completion evidence — the substrate a future Evaluation Harness
-- reads instead of trusting a self-reported status.
CREATE TABLE IF NOT EXISTS work_item_evidence (
    evidence_id   UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    work_item_id  UUID NOT NULL REFERENCES work_items(work_item_id) ON DELETE CASCADE,
    evidence_type TEXT NOT NULL,
    artifact_id   UUID REFERENCES task_artifacts(artifact_id) ON DELETE SET NULL,
    task_id       UUID REFERENCES tasks(id) ON DELETE SET NULL,
    execution_id  UUID,
    note          TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    CHECK (evidence_type IN ('artifact', 'task', 'execution', 'note')),
    CHECK (evidence_type <> 'artifact' OR artifact_id IS NOT NULL)
);

CREATE INDEX IF NOT EXISTS idx_work_item_evidence_work_item
    ON work_item_evidence (work_item_id, created_at DESC);

-- ============================================================
-- Decision Log — append-only; superseded, never overwritten
-- ============================================================
CREATE TABLE IF NOT EXISTS project_decisions (
    decision_id   UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    project_id    UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    title         TEXT NOT NULL,
    context       TEXT,
    decision      TEXT NOT NULL,
    reason        TEXT,
    status        TEXT NOT NULL DEFAULT 'accepted',
    source        TEXT,
    superseded_by UUID REFERENCES project_decisions(decision_id) ON DELETE SET NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    CHECK (status IN ('proposed', 'accepted', 'superseded', 'rejected'))
);

CREATE INDEX IF NOT EXISTS idx_project_decisions_project
    ON project_decisions (project_id, created_at DESC);

-- ============================================================
-- Risk — something that *might* happen
-- ============================================================
CREATE TABLE IF NOT EXISTS project_risks (
    risk_id      UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    project_id   UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    milestone_id UUID REFERENCES milestones(id) ON DELETE SET NULL,
    title        TEXT NOT NULL,
    description  TEXT,
    severity     TEXT NOT NULL DEFAULT 'medium',
    likelihood   TEXT NOT NULL DEFAULT 'medium',
    status       TEXT NOT NULL DEFAULT 'open',
    mitigation   TEXT,
    owner        TEXT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    resolved_at  TIMESTAMPTZ,

    CHECK (severity IN ('low', 'medium', 'high', 'critical')),
    CHECK (likelihood IN ('low', 'medium', 'high')),
    CHECK (status IN ('open', 'mitigated', 'accepted', 'resolved'))
);

CREATE INDEX IF NOT EXISTS idx_project_risks_project_status
    ON project_risks (project_id, status);

CREATE TRIGGER trg_project_risks_updated_at
    BEFORE UPDATE ON project_risks FOR EACH ROW EXECUTE FUNCTION update_updated_at();

-- ============================================================
-- Blocker — something that *is* preventing work right now
-- ============================================================
CREATE TABLE IF NOT EXISTS project_blockers (
    blocker_id   UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    project_id   UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    work_item_id UUID REFERENCES work_items(work_item_id) ON DELETE SET NULL,
    title        TEXT NOT NULL,
    description  TEXT,
    status       TEXT NOT NULL DEFAULT 'open',
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    resolved_at  TIMESTAMPTZ,

    CHECK (status IN ('open', 'resolved'))
);

CREATE INDEX IF NOT EXISTS idx_project_blockers_project_status
    ON project_blockers (project_id, status);

-- ============================================================
-- Deferred Improvement — structured form of docs DEF-XXX
-- ============================================================
CREATE TABLE IF NOT EXISTS deferred_improvements (
    improvement_id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    project_id     UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    -- Human-stable key from the Markdown docs, e.g. "DEF-011". Nullable:
    -- not every improvement has one yet.
    external_key   TEXT,
    title          TEXT NOT NULL,
    description    TEXT,
    priority       TEXT NOT NULL DEFAULT 'medium',
    status         TEXT NOT NULL DEFAULT 'deferred',
    source         TEXT,
    why_deferred   TEXT,
    required_before TEXT,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    CHECK (priority IN ('low', 'medium', 'high', 'critical')),
    CHECK (status IN ('deferred', 'in_progress', 'resolved', 'dropped')),
    -- NULL external_key rows never collide (Postgres treats NULLs as
    -- distinct), so keyless improvements are allowed alongside keyed ones.
    UNIQUE (project_id, external_key)
);

CREATE INDEX IF NOT EXISTS idx_deferred_improvements_project_status
    ON deferred_improvements (project_id, status);

CREATE TRIGGER trg_deferred_improvements_updated_at
    BEFORE UPDATE ON deferred_improvements FOR EACH ROW EXECUTE FUNCTION update_updated_at();

-- ============================================================
-- Project Activity — structured long-term timeline (not EventBus)
-- ============================================================
-- EventBus is runtime transport. ProjectActivity is a durable business fact.
-- Only selected runtime events should ever be promoted into this table.
CREATE TABLE IF NOT EXISTS project_activities (
    activity_id   UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    project_id    UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    activity_type TEXT NOT NULL,
    entity_type   TEXT NOT NULL,
    entity_id     UUID,
    summary       TEXT NOT NULL,
    occurred_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    metadata      JSONB NOT NULL DEFAULT '{}'
);

CREATE INDEX IF NOT EXISTS idx_project_activities_project_time
    ON project_activities (project_id, occurred_at DESC);
