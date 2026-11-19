-- 0011: Key Vault, Identities, Identity Platforms, Web Sessions

-- Key Vault (encrypted secrets)
CREATE TABLE IF NOT EXISTS key_vault (
    name            TEXT PRIMARY KEY,
    category        JSONB NOT NULL,
    encrypted_value BYTEA NOT NULL,
    nonce           BYTEA NOT NULL,
    metadata        JSONB NOT NULL DEFAULT '{}',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_accessed   TIMESTAMPTZ
);

-- Identities (users / access control)
CREATE TABLE IF NOT EXISTS identities (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    name            TEXT NOT NULL,
    role            role NOT NULL DEFAULT 'viewer',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_active     TIMESTAMPTZ
);

CREATE TABLE IF NOT EXISTS identity_platforms (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    identity_id     UUID NOT NULL REFERENCES identities(id) ON DELETE CASCADE,
    platform        platform NOT NULL,
    platform_user_id TEXT NOT NULL,
    username        TEXT,
    -- Web dashboard auth
    password_hash   TEXT,
    -- Timestamps
    linked_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    UNIQUE (platform, platform_user_id)
);

CREATE INDEX IF NOT EXISTS idx_identity_platforms_identity ON identity_platforms (identity_id);
CREATE INDEX IF NOT EXISTS idx_identity_platforms_lookup ON identity_platforms (platform, platform_user_id);

-- Web Sessions (dashboard authentication)
CREATE TABLE IF NOT EXISTS web_sessions (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    identity_id     UUID NOT NULL REFERENCES identities(id) ON DELETE CASCADE,
    token_hash      TEXT NOT NULL UNIQUE,
    ip_address      INET,
    user_agent      TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at      TIMESTAMPTZ NOT NULL,
    last_active     TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_web_sessions_identity ON web_sessions (identity_id);
CREATE INDEX IF NOT EXISTS idx_web_sessions_token ON web_sessions (token_hash);
CREATE INDEX IF NOT EXISTS idx_web_sessions_expires ON web_sessions (expires_at);
