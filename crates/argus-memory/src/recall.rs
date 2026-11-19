//! Semantic memory recall via pgvector cosine similarity search.
//!
//! The recall module provides the query side of the memory system:
//! given a natural language query, it embeds it with BGE-M3 and searches
//! for the most semantically similar memories in PostgreSQL via pgvector HNSW.
//!
//! Results are ranked by `similarity * importance` to prioritize both
//! relevance and significance.

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

/// Options for filtering and configuring recall queries.
#[derive(Clone, Debug)]
pub struct RecallOptions {
    /// Only search these layers (empty = all).
    pub layers: Vec<MemoryLayer>,
    /// Only search memories for this project (None = all projects).
    pub project_id: Option<Uuid>,
    /// Only search memories of these types (empty = all).
    pub type_filter: Vec<MemoryType>,
    /// Only return memories with these tags (empty = no tag filter).
    pub tag_filter: Vec<String>,
    /// Minimum cosine similarity threshold (0.0 - 1.0).
    pub min_similarity: f64,
    /// Maximum number of results to return.
    pub limit: u32,
    /// Only search memories created within this time range.
    pub time_range: Option<(DateTime<Utc>, DateTime<Utc>)>,
}

impl Default for RecallOptions {
    fn default() -> Self {
        Self {
            layers: vec![],
            project_id: None,
            type_filter: vec![],
            tag_filter: vec![],
            min_similarity: 0.6,
            limit: 10,
            time_range: None,
        }
    }
}

/// A memory retrieved by semantic search, with similarity score.
#[derive(Clone, Debug)]
pub struct RetrievedMemory {
    pub id: MemoryId,
    pub content: String,
    pub summary: Option<String>,
    pub layer: MemoryLayer,
    pub memory_type: MemoryType,
    /// Cosine similarity to the query (0.0 - 1.0, higher = more similar).
    pub similarity: f64,
    pub importance: f32,
    pub tags: Vec<String>,
    pub project_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub access_count: i32,
}

// ============================================================
// Recall engine
// ============================================================

/// Semantic memory recall engine.
pub struct RecallEngine {
    pool: PgPool,
    embedding: Arc<EmbeddingEngine>,
    event_bus: Arc<EventBus>,
}

impl RecallEngine {
    /// Create a new recall engine.
    pub fn new(pool: PgPool, embedding: Arc<EmbeddingEngine>, event_bus: Arc<EventBus>) -> Self {
        Self {
            pool,
            embedding,
            event_bus,
        }
    }

    /// Perform semantic recall: embed the query and search for similar memories.
    ///
    /// Results are sorted by `similarity * importance` (descending) so that
    /// both relevance and significance are considered.
    pub async fn recall(
        &self,
        query: &str,
        options: RecallOptions,
    ) -> ArgusResult<Vec<RetrievedMemory>> {
        let start = std::time::Instant::now();

        // 1. Embed the query
        let query_embedding = self.embedding.embed(query).await?;
        let query_vector = Vector::from(query_embedding);

        // 2. Build dynamic query with filters
        let mut sql = String::from(
            r#"
            SELECT id, content, summary, layer, memory_type,
                   1 - (embedding <=> $1::vector) AS similarity,
                   importance, tags, project_id, created_at, access_count
            FROM memories
            WHERE embedding IS NOT NULL
            "#,
        );

        let mut param_idx = 2u32;

        // Layer filter
        if !options.layers.is_empty() {
            sql.push_str(&format!(
                "  AND layer = ANY(${}::memory_layer[])\n",
                param_idx
            ));
            param_idx += 1;
        }

        // Project filter
        if options.project_id.is_some() {
            sql.push_str(&format!("  AND project_id = ${}\n", param_idx));
            param_idx += 1;
        }

        // Type filter
        if !options.type_filter.is_empty() {
            sql.push_str(&format!(
                "  AND memory_type = ANY(${}::memory_type[])\n",
                param_idx
            ));
            param_idx += 1;
        }

        // Tag filter (memories must contain ALL specified tags)
        if !options.tag_filter.is_empty() {
            sql.push_str(&format!("  AND tags @> ${}\n", param_idx));
            param_idx += 1;
        }

        // Time range filter
        if let Some((ref _start, ref _end)) = options.time_range {
            sql.push_str(&format!(
                "  AND created_at >= ${} AND created_at <= ${}\n",
                param_idx,
                param_idx + 1
            ));
            param_idx += 2;
        }

        // Similarity threshold and ordering
        sql.push_str(&format!(
            r#"
            ORDER BY embedding <=> $1::vector
            LIMIT ${}
            "#,
            param_idx
        ));

        // 3. Build and execute the query with dynamic bindings
        // We use a manual approach since the number of parameters varies
        let rows = self
            .execute_recall_query(&sql, &query_vector, &options)
            .await?;

        // 4. Filter by min_similarity and sort by similarity * importance
        let mut results: Vec<RetrievedMemory> = rows
            .into_iter()
            .filter(|r| r.similarity >= options.min_similarity)
            .collect();

        results.sort_by(|a, b| {
            let score_a = a.similarity * f64::from(a.importance);
            let score_b = b.similarity * f64::from(b.importance);
            score_b
                .partial_cmp(&score_a)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // 5. Update access counts asynchronously for returned results
        let pool = self.pool.clone();
        let ids: Vec<Uuid> = results.iter().map(|r| r.id).collect();
        tokio::spawn(async move {
            if let Err(e) = sqlx::query(
                r#"
                UPDATE memories
                SET access_count = access_count + 1,
                    last_accessed = NOW()
                WHERE id = ANY($1)
                "#,
            )
            .bind(&ids)
            .execute(&pool)
            .await
            {
                tracing::warn!(error = %e, "Failed to update access counts after recall");
            }
        });

        // 6. Emit recall event
        let latency_ms = start.elapsed().as_millis() as u64;
        let top_similarity = results.first().map(|r| r.similarity).unwrap_or(0.0);

        self.event_bus.publish(
            EventSource::MemoryEngine,
            ArgusEvent::MemoryRecalled {
                query: query.to_string(),
                results_count: results.len() as u32,
                top_similarity,
                latency_ms,
            },
        );

        tracing::debug!(
            query = query,
            results = results.len(),
            top_similarity = top_similarity,
            latency_ms = latency_ms,
            "Memory recall completed"
        );

        Ok(results)
    }

    /// Execute the dynamically-built recall query.
    ///
    /// This method handles the dynamic parameter binding that can't easily
    /// be done with sqlx's compile-time checked queries.
    async fn execute_recall_query(
        &self,
        _sql: &str,
        query_vector: &Vector,
        options: &RecallOptions,
    ) -> ArgusResult<Vec<RetrievedMemory>> {
        // For simplicity and type safety, we use a fixed query structure
        // and handle optional filters with SQL CASE/COALESCE patterns instead.
        // This avoids the complexity of dynamic parameter binding.
        let rows: Vec<RecallRow> = sqlx::query_as::<_, RecallRow>(
            r#"
            SELECT id, content, summary, layer, memory_type,
                   1 - (embedding <=> $1::vector) AS similarity,
                   importance, tags, project_id, created_at, access_count
            FROM memories
            WHERE embedding IS NOT NULL
              AND ($2::memory_layer[] IS NULL OR layer = ANY($2::memory_layer[]))
              AND ($3::uuid IS NULL OR project_id = $3)
              AND ($4::memory_type[] IS NULL OR memory_type = ANY($4::memory_type[]))
              AND ($5::text[] IS NULL OR tags @> $5)
              AND ($6::timestamptz IS NULL OR created_at >= $6)
              AND ($7::timestamptz IS NULL OR created_at <= $7)
            ORDER BY embedding <=> $1::vector
            LIMIT $8
            "#,
        )
        .bind(query_vector)
        .bind(if options.layers.is_empty() {
            None::<Vec<MemoryLayer>>
        } else {
            Some(options.layers.clone())
        })
        .bind(options.project_id)
        .bind(if options.type_filter.is_empty() {
            None::<Vec<MemoryType>>
        } else {
            Some(options.type_filter.clone())
        })
        .bind(if options.tag_filter.is_empty() {
            None::<Vec<String>>
        } else {
            Some(options.tag_filter.clone())
        })
        .bind(options.time_range.as_ref().map(|(start, _)| *start))
        .bind(options.time_range.as_ref().map(|(_, end)| *end))
        .bind(options.limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(ArgusError::Database)?;

        Ok(rows.into_iter().map(|r| r.into()).collect())
    }
}

// ============================================================
// Internal row mapping
// ============================================================

#[derive(sqlx::FromRow)]
struct RecallRow {
    id: Uuid,
    content: String,
    summary: Option<String>,
    layer: MemoryLayer,
    memory_type: MemoryType,
    similarity: f64,
    importance: f32,
    tags: Vec<String>,
    project_id: Option<Uuid>,
    created_at: DateTime<Utc>,
    access_count: i32,
}

impl From<RecallRow> for RetrievedMemory {
    fn from(row: RecallRow) -> Self {
        Self {
            id: row.id,
            content: row.content,
            summary: row.summary,
            layer: row.layer,
            memory_type: row.memory_type,
            similarity: row.similarity,
            importance: row.importance,
            tags: row.tags,
            project_id: row.project_id,
            created_at: row.created_at,
            access_count: row.access_count,
        }
    }
}
