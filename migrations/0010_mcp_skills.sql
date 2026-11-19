-- 0010: MCP Servers, Skills

CREATE TABLE IF NOT EXISTS mcp_servers (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    name            TEXT NOT NULL UNIQUE,
    -- Connection
    protocol        TEXT NOT NULL,
    command         TEXT,
    args            TEXT[],
    url             TEXT,
    -- Trust & security
    trust_level     trust_level NOT NULL DEFAULT 'untrusted',
    audit_result    JSONB,
    -- Available tools
    tools           JSONB NOT NULL DEFAULT '[]',
    -- Status
    enabled         BOOLEAN NOT NULL DEFAULT true,
    -- Source
    source          TEXT,
    version         TEXT,
    -- Timestamps
    installed_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_used       TIMESTAMPTZ,
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_mcp_servers_trust ON mcp_servers (trust_level);
CREATE INDEX IF NOT EXISTS idx_mcp_servers_enabled ON mcp_servers (enabled) WHERE enabled = true;

-- Skills
CREATE TABLE IF NOT EXISTS skills (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    name            TEXT NOT NULL UNIQUE,
    description     TEXT,
    -- Skill type
    skill_type      TEXT NOT NULL,
    -- Source
    source_path     TEXT,
    source_code     TEXT,
    language        TEXT,
    -- Permissions
    permissions     JSONB NOT NULL DEFAULT '[]',
    trust_level     trust_level NOT NULL DEFAULT 'untrusted',
    -- Execution
    timeout_seconds INTEGER NOT NULL DEFAULT 300,
    sandbox         BOOLEAN NOT NULL DEFAULT true,
    -- Status
    enabled         BOOLEAN NOT NULL DEFAULT true,
    -- Metadata
    version         TEXT,
    author          TEXT,
    tags            TEXT[] NOT NULL DEFAULT '{}',
    -- Timestamps
    installed_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_used       TIMESTAMPTZ,
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_skills_type ON skills (skill_type);
CREATE INDEX IF NOT EXISTS idx_skills_enabled ON skills (enabled) WHERE enabled = true;
CREATE INDEX IF NOT EXISTS idx_skills_tags ON skills USING gin (tags);
