-- V0.45: immutable verification reports and explicit operator acceptance.
CREATE TABLE work_item_evaluations (
    evaluation_id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    work_item_id UUID NOT NULL REFERENCES work_items(work_item_id) ON DELETE CASCADE,
    input_hash TEXT NOT NULL,
    evaluator_version TEXT NOT NULL,
    outcome TEXT NOT NULL CHECK (outcome IN ('passed', 'failed', 'indeterminate')),
    report JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_evaluations_work_item
    ON work_item_evaluations(work_item_id, created_at DESC);

CREATE TABLE work_item_acceptances (
    acceptance_id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    work_item_id UUID NOT NULL REFERENCES work_items(work_item_id) ON DELETE CASCADE,
    evaluation_id UUID NOT NULL REFERENCES work_item_evaluations(evaluation_id),
    actor TEXT NOT NULL CHECK (length(trim(actor)) > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (work_item_id, evaluation_id)
);
