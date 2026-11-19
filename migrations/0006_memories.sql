-- 0006: Memories, memory_relations, conversation_summaries

CREATE TABLE IF NOT EXISTS memories (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    -- Classification
    layer           memory_layer NOT NULL,
    memory_type     memory_type NOT NULL,
    -- Content
    content         TEXT NOT NULL,
    summary         TEXT,
    -- Associations
    project_id      UUID REFERENCES projects(id) ON DELETE SET NULL,
    task_id         UUID REFERENCES tasks(id) ON DELETE SET NULL,
    -- Embedding (BGE-M3, 1024 dimensions)
    embedding       vector(1024),
    -- Importance & decay
    importance      REAL NOT NULL DEFAULT 0.5,
    access_count    INTEGER NOT NULL DEFAULT 0,
    decay_rate      REAL NOT NULL DEFAULT 0.01,
    -- Tags
    tags            TEXT[] NOT NULL DEFAULT '{}',
    -- Source
    source_model    TEXT,
    source_type     TEXT,
    -- Timestamps
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_accessed   TIMESTAMPTZ,
    expires_at      TIMESTAMPTZ
);

-- HNSW index for semantic search (pgvector)
CREATE INDEX IF NOT EXISTS idx_memories_embedding ON memories
    USING hnsw (embedding vector_cosine_ops)
    WITH (m = 16, ef_construction = 64);

CREATE INDEX IF NOT EXISTS idx_memories_layer ON memories (layer);
CREATE INDEX IF NOT EXISTS idx_memories_project ON memories (project_id) WHERE project_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_memories_importance ON memories (importance DESC);
CREATE INDEX IF NOT EXISTS idx_memories_expires ON memories (expires_at) WHERE expires_at IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_memories_type ON memories (memory_type);
CREATE INDEX IF NOT EXISTS idx_memories_tags ON memories USING gin (tags);
CREATE INDEX IF NOT EXISTS idx_memories_content_search ON memories USING gin (content gin_trgm_ops);

-- Memory Relations
CREATE TABLE IF NOT EXISTS memory_relations (
    source_id       UUID NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    target_id       UUID NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    relation_type   TEXT NOT NULL,
    strength        REAL NOT NULL DEFAULT 1.0,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    PRIMARY KEY (source_id, target_id, relation_type)
);

CREATE INDEX IF NOT EXISTS idx_memory_relations_target ON memory_relations (target_id);

-- Conversation Summaries
CREATE TABLE IF NOT EXISTS conversation_summaries (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    -- Context
    project_id      UUID REFERENCES projects(id) ON DELETE SET NULL,
    platform        platform,
    session_id      TEXT,
    -- Content
    summary         TEXT NOT NULL,
    message_count   INTEGER NOT NULL,
    -- Key decisions / action items extracted
    decisions       JSONB NOT NULL DEFAULT '[]',
    action_items    JSONB NOT NULL DEFAULT '[]',
    -- Embedding for retrieval
    embedding       vector(1024),
    -- Time range covered
    period_start    TIMESTAMPTZ NOT NULL,
    period_end      TIMESTAMPTZ NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_conv_summaries_project ON conversation_summaries (project_id);
CREATE INDEX IF NOT EXISTS idx_conv_summaries_period ON conversation_summaries (period_start, period_end);
CREATE INDEX IF NOT EXISTS idx_conv_summaries_embedding ON conversation_summaries
    USING hnsw (embedding vector_cosine_ops)
    WITH (m = 16, ef_construction = 64);
