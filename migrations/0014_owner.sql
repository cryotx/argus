-- 0014: Owner profiles

CREATE TABLE IF NOT EXISTS owner_profiles (
    id                  UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    identity_id         UUID NOT NULL REFERENCES identities(id) ON DELETE CASCADE,
    -- Preferences
    disturbance_level   TEXT NOT NULL DEFAULT 'normal',
    communication_style TEXT NOT NULL DEFAULT 'concise',
    language            TEXT NOT NULL DEFAULT 'zh-CN',
    timezone            TEXT NOT NULL DEFAULT 'Asia/Shanghai',
    -- Active hours
    active_hours_start  TIME NOT NULL DEFAULT '08:00',
    active_hours_end    TIME NOT NULL DEFAULT '23:00',
    -- Approval
    approval_mode       TEXT NOT NULL DEFAULT 'critical_only',
    -- Learned preferences
    learned_preferences JSONB NOT NULL DEFAULT '{}',
    -- Timestamps
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_owner_profiles_identity ON owner_profiles (identity_id);
