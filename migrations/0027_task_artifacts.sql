-- 0027: Task Artifacts — durable record of verified Git checkpoints (V0.38 Phase 4)
--
-- A task's Agent output becomes an Artifact only after it has been checked
-- (Quality Gate) and turned into a real Git commit in the task's own
-- worktree — never before. This table is that record: the thing Project
-- State, a future Reviewer, or an Integrator will read, not the task's
-- self-reported output alone.

CREATE TABLE IF NOT EXISTS task_artifacts (
    artifact_id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),

    -- Only one artifact type exists yet (a Git checkpoint commit). Kept as
    -- an explicit column rather than assumed so a future artifact kind
    -- (e.g. a build output) doesn't require a schema rewrite.
    artifact_type       TEXT NOT NULL DEFAULT 'git_commit'
        CHECK (artifact_type IN ('git_commit')),

    -- Provenance: exactly which project/task/execution/workspace produced
    -- this artifact.
    project_id          UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    task_id             UUID NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    execution_id        UUID NOT NULL,
    workspace_id        UUID NOT NULL REFERENCES task_workspaces(workspace_id) ON DELETE CASCADE,

    -- Git checkpoint identity.
    base_revision       TEXT NOT NULL,
    branch_name         TEXT NOT NULL,
    commit_sha          TEXT NOT NULL,
    files_changed       JSONB NOT NULL DEFAULT '[]',

    -- Quality Gate evidence this checkpoint was created from.
    quality_status      TEXT,
    quality_summary     TEXT,

    -- Agent runtime evidence, best-effort (not every driver reports all of
    -- these).
    agent_runtime       TEXT,
    model               TEXT,
    session_id          TEXT,

    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    -- Idempotency (rule: finalize replay must never create a second
    -- artifact for the same execution): at most one artifact of a given
    -- type per execution.
    UNIQUE (execution_id, artifact_type)
);

CREATE INDEX IF NOT EXISTS idx_task_artifacts_task
    ON task_artifacts (task_id);

CREATE INDEX IF NOT EXISTS idx_task_artifacts_project
    ON task_artifacts (project_id);

CREATE INDEX IF NOT EXISTS idx_task_artifacts_workspace
    ON task_artifacts (workspace_id);

-- A given workspace's worktree must never produce two artifacts pointing at
-- the same commit under different executions — that would mean the
-- idempotent-reuse path returned the wrong workspace's commit.
CREATE UNIQUE INDEX IF NOT EXISTS idx_task_artifacts_workspace_commit
    ON task_artifacts (workspace_id, commit_sha);
