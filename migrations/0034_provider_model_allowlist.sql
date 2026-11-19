-- 0034: Model allow/deny in the project provider policy (V0.44)
--
-- V0.44 §53 requires per-project model allow/deny alongside provider
-- allow/deny. Model ids are stored as data and are never hard-coded into the
-- router, so a model can be retired or added without a code change.

ALTER TABLE project_provider_policies
    ADD COLUMN IF NOT EXISTS allowed_models JSONB NOT NULL DEFAULT '[]';

ALTER TABLE project_provider_policies
    ADD COLUMN IF NOT EXISTS denied_models JSONB NOT NULL DEFAULT '[]';
