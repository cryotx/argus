//! Context builder: assembles relevant memories into an LLM-ready context window.
//!
//! The context builder retrieves memories from multiple layers with a token budget
//! and formats them for insertion into an LLM prompt. It prioritizes memories by layer:
//!
//! 1. **Permanent** — safety rules, core identity (always included)
//! 2. **Project long-term** — project-specific knowledge
//! 3. **Cross-project long-term** — general knowledge
//! 4. **Short-term / recent** — recent context
//!
//! Token counting uses a simple `len / 4` heuristic (good enough for planning;
//! exact tokenization depends on the model and will be refined in P2).

use std::sync::Arc;

use sqlx::PgPool;
use uuid::Uuid;

use argus_core::config::MemoryConfig;
use argus_core::event_bus::EventBus;
use argus_core::types::{ArgusResult, MemoryLayer};

use crate::embedding::EmbeddingEngine;
use crate::recall::{RecallEngine, RecallOptions, RetrievedMemory};

// ============================================================
// Types
// ============================================================

/// A fully assembled memory context ready for LLM prompt insertion.
#[derive(Clone, Debug)]
pub struct MemoryContext {
    /// Permanent memories (safety rules, identity).
    pub permanent: Vec<RetrievedMemory>,
    /// Project-specific long-term knowledge.
    pub project_knowledge: Vec<RetrievedMemory>,
    /// Cross-project general knowledge.
    pub general_knowledge: Vec<RetrievedMemory>,
    /// Recent short-term memories.
    pub recent: Vec<RetrievedMemory>,
    /// Total estimated tokens used.
    pub total_tokens: usize,
    /// Token budget that was allocated.
    pub token_budget: usize,
}

impl MemoryContext {
    /// Format the context as a markdown string for LLM prompt insertion.
    pub fn format_for_prompt(&self) -> String {
        let mut sections = Vec::new();

        if !self.permanent.is_empty() {
            let mut s = String::from("## Core Knowledge & Rules\n\n");
            for m in &self.permanent {
                s.push_str(&format!("- {}\n", m.content));
            }
            sections.push(s);
        }

        if !self.project_knowledge.is_empty() {
            let mut s = String::from("## Project Knowledge\n\n");
            for m in &self.project_knowledge {
                let display = m.summary.as_deref().unwrap_or(&m.content);
                s.push_str(&format!("- [{:?}] {}\n", m.memory_type, display));
            }
            sections.push(s);
        }

        if !self.general_knowledge.is_empty() {
            let mut s = String::from("## General Knowledge\n\n");
            for m in &self.general_knowledge {
                let display = m.summary.as_deref().unwrap_or(&m.content);
                s.push_str(&format!("- [{:?}] {}\n", m.memory_type, display));
            }
            sections.push(s);
        }

        if !self.recent.is_empty() {
            let mut s = String::from("## Recent Context\n\n");
            for m in &self.recent {
                s.push_str(&format!("- {}\n", m.content));
            }
            sections.push(s);
        }

        if sections.is_empty() {
            return String::from("(No relevant memories found)");
        }

        sections.join("\n")
    }

    /// Check if the context is empty (no memories in any section).
    pub fn is_empty(&self) -> bool {
        self.permanent.is_empty()
            && self.project_knowledge.is_empty()
            && self.general_knowledge.is_empty()
            && self.recent.is_empty()
    }
}

// ============================================================
// ContextBuilder
// ============================================================

/// Builds LLM-ready context from memories with a token budget.
pub struct ContextBuilder {
    recall: RecallEngine,
    config: MemoryConfig,
}

impl ContextBuilder {
    /// Create a new context builder.
    pub fn new(
        pool: PgPool,
        embedding: Arc<EmbeddingEngine>,
        event_bus: Arc<EventBus>,
        config: MemoryConfig,
    ) -> Self {
        Self {
            recall: RecallEngine::new(pool, embedding, event_bus),
            config,
        }
    }

    /// Build a context from memories relevant to the given query.
    ///
    /// Allocates the token budget across layers with these priorities:
    /// - Permanent: up to 15% of budget
    /// - Project long-term: up to 35% of budget
    /// - Cross-project long-term: up to 30% of budget
    /// - Short-term / recent: up to 20% of budget
    ///
    /// If a layer doesn't use its full allocation, the remainder is
    /// redistributed to subsequent layers.
    pub async fn build_context(
        &self,
        query: &str,
        token_budget: usize,
        project_id: Option<Uuid>,
    ) -> ArgusResult<MemoryContext> {
        let mut remaining_tokens = token_budget;

        // 1. Permanent memories (always included, up to 15%)
        let permanent_budget = token_budget * 15 / 100;
        let permanent = self
            .recall_layer(query, &[MemoryLayer::Permanent], None, permanent_budget)
            .await?;
        let permanent_used = estimate_tokens_for_memories(&permanent);
        remaining_tokens = remaining_tokens.saturating_sub(permanent_used);

        // 2. Project-specific long-term (up to 35% of original, or more if permanent was small)
        let project_budget = std::cmp::min(
            remaining_tokens,
            token_budget * 35 / 100 + (permanent_budget - permanent_used),
        );
        let project_knowledge = if project_id.is_some() {
            self.recall_layer(query, &[MemoryLayer::LongTerm], project_id, project_budget)
                .await?
        } else {
            vec![]
        };
        let project_used = estimate_tokens_for_memories(&project_knowledge);
        remaining_tokens = remaining_tokens.saturating_sub(project_used);

        // 3. Cross-project long-term (up to 30%)
        let general_budget = std::cmp::min(remaining_tokens, token_budget * 30 / 100);
        let general_knowledge = self
            .recall_layer(query, &[MemoryLayer::LongTerm], None, general_budget)
            .await?;
        let general_used = estimate_tokens_for_memories(&general_knowledge);
        remaining_tokens = remaining_tokens.saturating_sub(general_used);

        // 4. Short-term / recent (remaining budget)
        let recent = self
            .recall_layer(
                query,
                &[MemoryLayer::ShortTerm],
                project_id,
                remaining_tokens,
            )
            .await?;
        let recent_used = estimate_tokens_for_memories(&recent);

        let total_tokens = permanent_used + project_used + general_used + recent_used;

        tracing::debug!(
            permanent = permanent.len(),
            project = project_knowledge.len(),
            general = general_knowledge.len(),
            recent = recent.len(),
            total_tokens = total_tokens,
            budget = token_budget,
            "Memory context built"
        );

        Ok(MemoryContext {
            permanent,
            project_knowledge,
            general_knowledge,
            recent,
            total_tokens,
            token_budget,
        })
    }

    /// Recall memories from specific layers within a token budget.
    async fn recall_layer(
        &self,
        query: &str,
        layers: &[MemoryLayer],
        project_id: Option<Uuid>,
        token_budget: usize,
    ) -> ArgusResult<Vec<RetrievedMemory>> {
        if token_budget == 0 {
            return Ok(vec![]);
        }

        let options = RecallOptions {
            layers: layers.to_vec(),
            project_id,
            min_similarity: self.config.similarity_threshold,
            limit: self.config.recall_top_k,
            ..Default::default()
        };

        let mut memories = self.recall.recall(query, options).await?;

        // Trim to fit within token budget
        let mut used_tokens = 0;
        memories.retain(|m| {
            let tokens = estimate_tokens(&m.content);
            if used_tokens + tokens <= token_budget {
                used_tokens += tokens;
                true
            } else {
                false
            }
        });

        Ok(memories)
    }
}

// ============================================================
// Token estimation
// ============================================================

/// Estimate token count for a string using the `len / 4` heuristic.
///
/// This is a rough approximation. English text averages ~4 chars per token
/// for most LLM tokenizers. CJK text would be ~2 chars per token, but
/// we'll refine this in P2 when the LLM provider engine is available.
fn estimate_tokens(text: &str) -> usize {
    // Minimum 1 token per non-empty string
    if text.is_empty() {
        0
    } else {
        std::cmp::max(1, text.len() / 4)
    }
}

/// Estimate total tokens for a slice of memories.
fn estimate_tokens_for_memories(memories: &[RetrievedMemory]) -> usize {
    memories
        .iter()
        .map(|m| {
            let content_tokens = estimate_tokens(&m.content);
            let summary_tokens = m.summary.as_ref().map(|s| estimate_tokens(s)).unwrap_or(0);
            // Use summary if available, otherwise content + a small overhead for formatting
            if summary_tokens > 0 {
                summary_tokens + 10
            } else {
                content_tokens + 10
            }
        })
        .sum()
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_estimate_tokens() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("hi"), 1); // 2 chars / 4 = 0, but min 1
        assert_eq!(estimate_tokens("hello world"), 2); // 11 / 4 = 2
        assert_eq!(estimate_tokens("a".repeat(400).as_str()), 100); // 400 / 4 = 100
    }

    #[test]
    fn test_empty_context_format() {
        let ctx = MemoryContext {
            permanent: vec![],
            project_knowledge: vec![],
            general_knowledge: vec![],
            recent: vec![],
            total_tokens: 0,
            token_budget: 8000,
        };

        assert!(ctx.is_empty());
        assert_eq!(ctx.format_for_prompt(), "(No relevant memories found)");
    }
}
