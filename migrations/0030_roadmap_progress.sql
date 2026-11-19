-- 0030: Roadmap & Progress Engine foundation (V0.40)
--
-- Additive on top of V0.39. Gives the structured Project State an explicit
-- four-level roadmap (Project -> Milestone -> Phase -> WorkItem), a minimal
-- per-node weight, stable `external_key` handles, roadmap-level WorkItem
-- dependencies, and deferred-improvement linkage.
--
-- Design note: a Phase is its own table rather than a `milestones` row with
-- `node_type='phase'`. Reusing `milestones.parent_id` would make the existing
-- `UNIQUE (project_id, sequence)` ambiguous (a phase and a milestone could
-- both claim sequence 1) and would overload `work_items.milestone_id` to mean
-- "milestone or phase". A dedicated table keeps sequence scoping natural and
-- the hierarchy unambiguous, while still bounding depth to the fixed four
-- levels below — no arbitrary tree.

-- ============================================================
-- Milestones: stable handle + weight
-- ============================================================
ALTER TABLE milestones ADD COLUMN IF NOT EXISTS external_key TEXT;
ALTER TABLE milestones ADD COLUMN IF NOT EXISTS weight INTEGER NOT NULL DEFAULT 1;

ALTER TABLE milestones
    ADD CONSTRAINT milestones_weight_check CHECK (weight >= 0);

CREATE UNIQUE INDEX IF NOT EXISTS idx_milestones_external_key
    ON milestones (project_id, external_key) WHERE external_key IS NOT NULL;

-- ============================================================
-- Phases — the level between Milestone and WorkItem
-- ============================================================
CREATE TABLE IF NOT EXISTS phases (
    phase_id            UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    project_id          UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    milestone_id        UUID NOT NULL REFERENCES milestones(id) ON DELETE CASCADE,
    title               TEXT NOT NULL,
    description         TEXT,
    status              TEXT NOT NULL DEFAULT 'planned',
    sequence            INTEGER NOT NULL,
    external_key        TEXT,
    definition_of_done  TEXT,
    weight              INTEGER NOT NULL DEFAULT 1,
    started_at          TIMESTAMPTZ,
    completed_at        TIMESTAMPTZ,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    CHECK (status IN ('planned', 'active', 'blocked', 'deferred', 'completed', 'cancelled')),
    CHECK (weight >= 0),
    UNIQUE (milestone_id, sequence)
);

CREATE INDEX IF NOT EXISTS idx_phases_milestone ON phases (milestone_id);
CREATE INDEX IF NOT EXISTS idx_phases_project_status ON phases (project_id, status);

CREATE UNIQUE INDEX IF NOT EXISTS idx_phases_external_key
    ON phases (project_id, external_key) WHERE external_key IS NOT NULL;

CREATE TRIGGER trg_phases_updated_at
    BEFORE UPDATE ON phases FOR EACH ROW EXECUTE FUNCTION update_updated_at();

-- ============================================================
-- WorkItems: phase linkage + stable handle + weight
-- ============================================================
ALTER TABLE work_items ADD COLUMN IF NOT EXISTS phase_id UUID
    REFERENCES phases(phase_id) ON DELETE SET NULL;
ALTER TABLE work_items ADD COLUMN IF NOT EXISTS external_key TEXT;
ALTER TABLE work_items ADD COLUMN IF NOT EXISTS weight INTEGER NOT NULL DEFAULT 1;

ALTER TABLE work_items
    ADD CONSTRAINT work_items_weight_check CHECK (weight >= 0);

CREATE INDEX IF NOT EXISTS idx_work_items_phase
    ON work_items (phase_id) WHERE phase_id IS NOT NULL;

CREATE UNIQUE INDEX IF NOT EXISTS idx_work_items_external_key
    ON work_items (project_id, external_key) WHERE external_key IS NOT NULL;

-- ============================================================
-- Roadmap dependencies — WorkItem A before WorkItem B
-- ============================================================
-- Deliberately separate from the runtime TaskScheduler task DAG
-- (`task_dependencies`). Roadmap ordering is planning metadata; it must never
-- be confused with an execution dependency.
CREATE TABLE IF NOT EXISTS work_item_dependencies (
    work_item_id  UUID NOT NULL REFERENCES work_items(work_item_id) ON DELETE CASCADE,
    depends_on_id UUID NOT NULL REFERENCES work_items(work_item_id) ON DELETE CASCADE,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    PRIMARY KEY (work_item_id, depends_on_id),
    CHECK (work_item_id <> depends_on_id)
);

CREATE INDEX IF NOT EXISTS idx_work_item_dependencies_depends_on
    ON work_item_dependencies (depends_on_id);

-- ============================================================
-- Deferred improvements may attach to a milestone or a phase
-- ============================================================
ALTER TABLE deferred_improvements ADD COLUMN IF NOT EXISTS milestone_id UUID
    REFERENCES milestones(id) ON DELETE SET NULL;
ALTER TABLE deferred_improvements ADD COLUMN IF NOT EXISTS phase_id UUID
    REFERENCES phases(phase_id) ON DELETE SET NULL;
