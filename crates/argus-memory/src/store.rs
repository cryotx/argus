//! Memory storage with embedding and deduplication.
//!
//! `MemoryStore` is the primary interface for persisting memories to PostgreSQL.
//! On store, it:
//! 1. Generates a BGE-M3 embedding for the content
//! 2. Checks for near-duplicates (cosine similarity > 0.95)
//! 3. If duplicate found: updates existing memory (bumps importance, merges tags)
//! 4. If no duplicate: inserts new memory with embedding
//! 5. Emits `MemoryStored` event via the event bus

use std::sync::Arc;

use chrono::{DateTime, Utc};
use pgvector::Vector;
use sqlx::PgPool;
use uuid::Uuid;

use argus_core::event_bus::EventBus;
use argus_core::events::{ArgusEvent, EventSource};
use argus_core::types::{ArgusError, ArgusResult, MemoryId, MemoryLayer, MemoryType};

use crate::embedding::EmbeddingEngine;

// ============================================================
// Types
// ============================================================

/// Input for storing a new memory.
#[derive(Clone, Debug)]
pub struct NewMemory {
    /// The text content to store and embed.
    pub content: String,
    /// Which memory layer to store in.
    pub layer: MemoryLayer,
    /// What kind of memory this is.
    pub memory_type: MemoryType,
    /// Importance score (0.0 - 1.0). Higher = more resistant to decay.
    pub importance: f32,
    /// Optional project association.
    pub project_id: Option<Uuid>,
    /// Optional task association.
    pub task_id: Option<Uuid>,
    /// Searchable tags.
    pub tags: Vec<String>,
    /// Where this memory came from (e.g., "llm_response", "user_input", "tool_output").
    pub source_type: Option<String>,
    /// Which LLM model generated this memory (if applicable).
    pub source_model: Option<String>,
}

/// A memory record as stored in the database.
#[derive(Clone, Debug)]
pub struct MemoryRecord {
    pub id: MemoryId,
    pub layer: MemoryLayer,
    pub memory_type: MemoryType,
    pub content: String,
    pub summary: Option<String>,
    pub project_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub importance: f32,
    pub access_count: i32,
    pub decay_rate: f32,
    pub tags: Vec<String>,
    pub source_type: Option<String>,
    pub source_model: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_accessed: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
}

/// Deduplication threshold: if cosine similarity exceeds this, treat as duplicate.
const DEDUP_SIMILARITY_THRESHOLD: f64 = 0.95;

// ============================================================
// MemoryStore
// ============================================================

/// Primary interface for persisting and retrieving memories.
pub struct MemoryStore {
    pool: PgPool,
    embedding: Arc<EmbeddingEngine>,
    event_bus: Arc<EventBus>,
}

impl MemoryStore {
    /// Create a new memory store.
    pub fn new(pool: PgPool, embedding: Arc<EmbeddingEngine>, event_bus: Arc<EventBus>) -> Self {
        Self {
            pool,
            embedding,
            event_bus,
        }
    }

    /// Store a new memory with automatic embedding and deduplication.
    ///
    /// If a near-duplicate exists (cosine similarity > 0.95 in the same layer),
    /// the existing memory is updated instead of creating a new one.
    ///
    /// Returns the memory ID (new or existing).
    pub async fn store(&self, memory: NewMemory) -> ArgusResult<MemoryId> {
        // 1. Generate embedding
        let embedding = self.embedding.embed(&memory.content).await?;
        let vector = Vector::from(embedding);

        // 2. Check for near-duplicates in the same layer
        let duplicate: Option<(Uuid, f64)> = sqlx::query_as(
            r#"
            SELECT id, 1 - (embedding <=> $1::vector) AS similarity
            FROM memories
            WHERE layer = $2
              AND embedding IS NOT NULL
            ORDER BY embedding <=> $1::vector
            LIMIT 1
            "#,
        )
        .bind(&vector)
        .bind(&memory.layer)
        .fetch_optional(&self.pool)
        .await
        .map_err(ArgusError::Database)?;

        if let Some((existing_id, similarity)) = duplicate {
            if similarity > DEDUP_SIMILARITY_THRESHOLD {
                // Update existing memory: bump importance, merge tags, update content if newer
                tracing::debug!(
                    existing_id = %existing_id,
                    similarity = similarity,
                    "Near-duplicate detected, updating existing memory"
                );

                // Merge tags (union of existing + new)
                sqlx::query(
                    r#"
                    UPDATE memories
                    SET importance = GREATEST(importance, $2),
                        access_count = access_count + 1,
                        last_accessed = NOW(),
                        updated_at = NOW(),
                        tags = (
                            SELECT ARRAY(
                                SELECT DISTINCT unnest(tags || $3)
                            )
                        )
                    WHERE id = $1
                    "#,
                )
                .bind(existing_id)
                .bind(memory.importance)
                .bind(&memory.tags)
                .execute(&self.pool)
                .await
                .map_err(ArgusError::Database)?;

                self.event_bus.publish(
                    EventSource::MemoryEngine,
                    ArgusEvent::MemoryStored {
                        memory_id: existing_id,
                        layer: format!("{:?}", memory.layer),
                        memory_type: format!("{:?}", memory.memory_type),
                        project_id: memory.project_id,
                    },
                );

                return Ok(existing_id);
            }
        }

        // 3. Calculate expiration for short-term memories
        let expires_at = match memory.layer {
            MemoryLayer::ShortTerm => Some(Utc::now() + chrono::Duration::hours(72)),
            _ => None,
        };

        // 4. Insert new memory
        let row: (Uuid,) = sqlx::query_as(
            r#"
            INSERT INTO memories (
                layer, memory_type, content, embedding, importance,
                project_id, task_id, tags, source_type, source_model,
                expires_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            RETURNING id
            "#,
        )
        .bind(&memory.layer)
        .bind(&memory.memory_type)
        .bind(&memory.content)
        .bind(&vector)
        .bind(memory.importance)
        .bind(memory.project_id)
        .bind(memory.task_id)
        .bind(&memory.tags)
        .bind(&memory.source_type)
        .bind(&memory.source_model)
        .bind(expires_at)
        .fetch_one(&self.pool)
        .await
        .map_err(ArgusError::Database)?;

        let memory_id = row.0;

        tracing::info!(
            memory_id = %memory_id,
            layer = ?memory.layer,
            memory_type = ?memory.memory_type,
            "Memory stored"
        );

        self.event_bus.publish(
            EventSource::MemoryEngine,
            ArgusEvent::MemoryStored {
                memory_id,
                layer: format!("{:?}", memory.layer),
                memory_type: format!("{:?}", memory.memory_type),
                project_id: memory.project_id,
            },
        );

        Ok(memory_id)
    }

    /// Increment access count and update last_accessed timestamp.
    pub async fn increment_access(&self, id: MemoryId) -> ArgusResult<()> {
        sqlx::query(
            r#"
            UPDATE memories
            SET access_count = access_count + 1,
                last_accessed = NOW()
            WHERE id = $1
            "#,
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(ArgusError::Database)?;

        Ok(())
    }

    /// Get a memory by ID.
    pub async fn get_by_id(&self, id: MemoryId) -> ArgusResult<Option<MemoryRecord>> {
        let row = sqlx::query_as::<_, MemoryRow>(
            r#"
            SELECT id, layer, memory_type, content, summary, project_id, task_id,
                   importance, access_count, decay_rate, tags, source_type, source_model,
                   created_at, updated_at, last_accessed, expires_at
            FROM memories
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(ArgusError::Database)?;

        Ok(row.map(|r| r.into()))
    }

    /// Delete a memory by ID (hard delete).
    pub async fn delete(&self, id: MemoryId) -> ArgusResult<bool> {
        let result = sqlx::query("DELETE FROM memories WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(ArgusError::Database)?;

        Ok(result.rows_affected() > 0)
    }

    /// Get a reference to the embedding engine (for recall module).
    pub fn embedding_engine(&self) -> &Arc<EmbeddingEngine> {
        &self.embedding
    }

    /// Get a reference to the database pool (for recall/lifecycle modules).
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Get a reference to the event bus.
    pub fn event_bus(&self) -> &Arc<EventBus> {
        &self.event_bus
    }
}

// ============================================================
// Internal row mapping
// ============================================================

/// Internal row type for sqlx query_as mapping.
#[derive(sqlx::FromRow)]
struct MemoryRow {
    id: Uuid,
    layer: MemoryLayer,
    memory_type: MemoryType,
    content: String,
    summary: Option<String>,
    project_id: Option<Uuid>,
    task_id: Option<Uuid>,
    importance: f32,
    access_count: i32,
    decay_rate: f32,
    tags: Vec<String>,
    source_type: Option<String>,
    source_model: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    last_accessed: Option<DateTime<Utc>>,
    expires_at: Option<DateTime<Utc>>,
}

impl From<MemoryRow> for MemoryRecord {
    fn from(row: MemoryRow) -> Self {
        Self {
            id: row.id,
            layer: row.layer,
            memory_type: row.memory_type,
            content: row.content,
            summary: row.summary,
            project_id: row.project_id,
            task_id: row.task_id,
            importance: row.importance,
            access_count: row.access_count,
            decay_rate: row.decay_rate,
            tags: row.tags,
            source_type: row.source_type,
            source_model: row.source_model,
            created_at: row.created_at,
            updated_at: row.updated_at,
            last_accessed: row.last_accessed,
            expires_at: row.expires_at,
        }
    }
}
