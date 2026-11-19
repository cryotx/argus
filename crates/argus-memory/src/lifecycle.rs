//! Memory lifecycle management: decay, promotion, consolidation, expiration.
//!
//! This module implements the time-based lifecycle operations that keep the
//! memory system healthy and relevant:
//!
//! - **Decay** (every 6h): Gradually reduces importance of long-term memories
//!   based on elapsed time. Memories below threshold 0.05 are deleted.
//! - **Promotion** (every 1h): Moves short-term memories to long-term when
//!   they meet access count or importance thresholds.
//! - **Expiration** (every 30m): Removes memories past their `expires_at`.
//! - **Consolidation** (every 24h): Placeholder for LLM-powered summarization
//!   of similar memory clusters (will be wired in P2/P3).

use std::sync::Arc;

use sqlx::PgPool;
use uuid::Uuid;

use argus_core::config::MemoryConfig;
use argus_core::event_bus::EventBus;
use argus_core::events::{ArgusEvent, EventSource};
use argus_core::types::{ArgusError, ArgusResult};

// ============================================================
// MemoryLifecycle
// ============================================================

/// Manages memory decay, promotion, expiration, and consolidation.
pub struct MemoryLifecycle {
    pool: PgPool,
    event_bus: Arc<EventBus>,
    config: MemoryConfig,
}

impl MemoryLifecycle {
    /// Create a new lifecycle manager.
    pub fn new(pool: PgPool, event_bus: Arc<EventBus>, config: MemoryConfig) -> Self {
        Self {
            pool,
            event_bus,
            config,
        }
    }

    /// Run importance decay on long-term memories.
    ///
    /// Formula: `importance = GREATEST(importance - decay_rate * days_since_last_access, 0.0)`
    ///
    /// Memories that decay below 0.05 are deleted. Permanent memories are never decayed.
    ///
    /// Schedule: every 6 hours.
    pub async fn run_decay(&self) -> ArgusResult<DecayResult> {
        tracing::info!("Running memory decay cycle");

        // Decay long-term memories based on days since last access
        let decay_result = sqlx::query_as::<_, (i64,)>(
            r#"
            UPDATE memories
            SET importance = GREATEST(
                importance - decay_rate * EXTRACT(EPOCH FROM (NOW() - COALESCE(last_accessed, created_at))) / 86400.0,
                0.0
            ),
            updated_at = NOW()
            WHERE layer = 'long_term'
              AND importance > 0.0
            RETURNING id
            "#,
        )
        .fetch_all(&self.pool)
        .await;

        let affected_count = match decay_result {
            Ok(rows) => rows.len() as u32,
            Err(e) => {
                tracing::error!(error = %e, "Failed to run decay");
                return Err(ArgusError::Database(e));
            }
        };

        // Delete memories that have decayed below threshold
        let deleted = sqlx::query(
            r#"
            DELETE FROM memories
            WHERE layer = 'long_term'
              AND importance < 0.05
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(ArgusError::Database)?;

        let deleted_count = deleted.rows_affected() as u32;

        let result = DecayResult {
            affected_count,
            deleted_count,
        };

        tracing::info!(
            affected = affected_count,
            deleted = deleted_count,
            "Memory decay cycle completed"
        );

        self.event_bus.publish(
            EventSource::MemoryEngine,
            ArgusEvent::MemoryDecayRun {
                affected_count,
                deleted_count,
                promoted_count: 0,
            },
        );

        Ok(result)
    }

    /// Promote short-term memories to long-term based on access and importance thresholds.
    ///
    /// Promotion criteria (either):
    /// - `access_count >= 3`
    /// - `importance >= promotion_threshold` (default 0.7)
    ///
    /// Schedule: every 1 hour.
    pub async fn run_promotion(&self) -> ArgusResult<u32> {
        tracing::info!("Running memory promotion cycle");

        let threshold = self.config.promotion_threshold as f32;

        let promoted: Vec<(Uuid,)> = sqlx::query_as(
            r#"
            UPDATE memories
            SET layer = 'long_term',
                expires_at = NULL,
                updated_at = NOW()
            WHERE layer = 'short_term'
              AND (access_count >= 3 OR importance >= $1)
            RETURNING id
            "#,
        )
        .bind(threshold)
        .fetch_all(&self.pool)
        .await
        .map_err(ArgusError::Database)?;

        let count = promoted.len() as u32;

        // Emit individual promotion events
        for (memory_id,) in &promoted {
            self.event_bus.publish(
                EventSource::MemoryEngine,
                ArgusEvent::MemoryPromoted {
                    memory_id: *memory_id,
                    from_layer: "ShortTerm".to_string(),
                    to_layer: "LongTerm".to_string(),
                },
            );
        }

        tracing::info!(promoted = count, "Memory promotion cycle completed");

        Ok(count)
    }

    /// Remove expired memories (where `expires_at < NOW()`).
    ///
    /// Schedule: every 30 minutes.
    pub async fn run_expiration(&self) -> ArgusResult<u32> {
        tracing::info!("Running memory expiration cycle");

        let expired: Vec<(Uuid, String)> = sqlx::query_as(
            r#"
            DELETE FROM memories
            WHERE expires_at IS NOT NULL
              AND expires_at < NOW()
            RETURNING id, layer::text
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(ArgusError::Database)?;

        let count = expired.len() as u32;

        // Emit expiration events
        for (memory_id, layer) in &expired {
            self.event_bus.publish(
                EventSource::MemoryEngine,
                ArgusEvent::MemoryExpired {
                    memory_id: *memory_id,
                    layer: layer.clone(),
                    reason: "TTL expired".to_string(),
                },
            );
        }

        tracing::info!(expired = count, "Memory expiration cycle completed");

        Ok(count)
    }

    /// Consolidation placeholder: find clusters of similar memories for summarization.
    ///
    /// This will be fully implemented when the LLM provider engine is available (P2/P3).
    /// For now, it identifies candidate clusters but does not perform summarization.
    ///
    /// Schedule: every 24 hours.
    pub async fn run_consolidation(&self) -> ArgusResult<u32> {
        tracing::info!("Running memory consolidation cycle (placeholder)");

        // Find clusters of similar long-term memories that could be consolidated.
        // A proper implementation would:
        // 1. Find pairs of memories with cosine similarity > 0.85
        // 2. Group them into clusters
        // 3. Use LLM to generate a consolidated summary
        // 4. Store the summary as a new memory
        // 5. Mark source memories as consolidated

        // For now, just count how many potential candidates exist
        let candidate_count: (i64,) = sqlx::query_as(
            r#"
            SELECT COUNT(*)
            FROM memories
            WHERE layer = 'long_term'
              AND embedding IS NOT NULL
              AND access_count >= 2
            "#,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(ArgusError::Database)?;

        tracing::info!(
            candidates = candidate_count.0,
            "Memory consolidation scan completed (consolidation pending LLM integration)"
        );

        Ok(candidate_count.0 as u32)
    }

    /// Run all lifecycle operations in sequence.
    ///
    /// Typically called from a scheduled job runner. Individual operations
    /// can also be called independently at their own intervals.
    pub async fn run_all(&self) -> ArgusResult<LifecycleReport> {
        let expired = self.run_expiration().await?;
        let promoted = self.run_promotion().await?;
        let decay = self.run_decay().await?;
        let consolidation_candidates = self.run_consolidation().await?;

        let report = LifecycleReport {
            expired,
            promoted,
            decayed: decay.affected_count,
            decay_deleted: decay.deleted_count,
            consolidation_candidates,
        };

        tracing::info!(
            expired = report.expired,
            promoted = report.promoted,
            decayed = report.decayed,
            decay_deleted = report.decay_deleted,
            consolidation_candidates = report.consolidation_candidates,
            "Full lifecycle cycle completed"
        );

        Ok(report)
    }
}

// ============================================================
// Result types
// ============================================================

/// Result of a decay operation.
#[derive(Clone, Debug)]
pub struct DecayResult {
    /// Number of memories whose importance was reduced.
    pub affected_count: u32,
    /// Number of memories deleted because importance dropped below 0.05.
    pub deleted_count: u32,
}

/// Summary of a full lifecycle run.
#[derive(Clone, Debug)]
pub struct LifecycleReport {
    /// Number of memories expired (TTL).
    pub expired: u32,
    /// Number of memories promoted (short-term -> long-term).
    pub promoted: u32,
    /// Number of memories affected by decay.
    pub decayed: u32,
    /// Number of memories deleted by decay (importance < 0.05).
    pub decay_deleted: u32,
    /// Number of consolidation candidates found.
    pub consolidation_candidates: u32,
}
