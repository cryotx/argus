-- 0026: Git worktree isolation and workspace ownership

-- Task workspace isolation tracking
CREATE TABLE IF NOT EXISTS task_workspaces (
    workspace_id        UUID PRIMARY KEY DEFAULT gen_random_uuid(),

    -- Ownership
    project_id          UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    task_id             UUID NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    execution_id        UUID NOT NULL,

    -- Repository reference
    repository_path     TEXT NOT NULL,

    -- Worktree details
    worktree_path       TEXT NOT NULL,
    branch_name         TEXT NOT NULL,
    base_revision       TEXT NOT NULL,  -- Commit SHA worktree was based on

    -- Lifecycle
    status              TEXT NOT NULL
        CHECK (status IN ('preparing', 'active', 'completed', 'failed', 'orphaned', 'recovery_needed'))
        DEFAULT 'preparing',
    failure_reason      TEXT,

    -- Timestamps
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    activated_at        TIMESTAMPTZ,
    completed_at        TIMESTAMPTZ,
    released_at         TIMESTAMPTZ,

    -- Constraints
    UNIQUE(execution_id),  -- One workspace per execution
    UNIQUE(worktree_path)  -- Worktree paths must be unique
);

-- Indexes for quick lookup
CREATE INDEX IF NOT EXISTS idx_workspace_project
    ON task_workspaces (project_id);

CREATE INDEX IF NOT EXISTS idx_workspace_task
    ON task_workspaces (task_id);

CREATE INDEX IF NOT EXISTS idx_workspace_status
    ON task_workspaces (status);

CREATE INDEX IF NOT EXISTS idx_workspace_active
    ON task_workspaces (status)
    WHERE status = 'active';

-- Extend llm_usage for workspace tracking
ALTER TABLE IF EXISTS llm_usage
ADD COLUMN IF NOT EXISTS workspace_id UUID REFERENCES task_workspaces(workspace_id) ON DELETE SET NULL;

-- Create index for workspace spend queries
CREATE INDEX IF NOT EXISTS idx_llm_usage_workspace
    ON llm_usage (workspace_id);
