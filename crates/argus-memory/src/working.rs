//! In-memory working memory manager.
//!
//! Working memory is the fastest layer — pure RAM, keyed by task ID.
//! It acts as a scratchpad for tasks in progress, holding intermediate
//! results, relevant recalled memories, and task-specific notes.
//!
//! When a task completes, working memory is flushed: important entries
//! are persisted to short-term memory in PostgreSQL, and the in-memory
//! data is dropped.

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use uuid::Uuid;

use argus_core::types::{ArgusResult, MemoryLayer, MemoryType, TaskId};

use crate::recall::RetrievedMemory;
use crate::store::{MemoryStore, NewMemory};

// ============================================================
// Types
// ============================================================

/// A single entry in working memory (key-value scratchpad item).
#[derive(Clone, Debug)]
pub struct WorkingMemoryEntry {
    /// Semantic key (e.g., "current_approach", "error_context", "user_intent").
    pub key: String,
    /// The actual content.
    pub value: String,
    /// When this entry was added.
    pub added_at: DateTime<Utc>,
    /// Estimated token count for this entry.
    pub tokens: usize,
}

/// Working memory for a single task.
#[derive(Clone, Debug)]
pub struct WorkingMemory {
    /// Task this working memory belongs to.
    pub task_id: TaskId,
    /// Description of the task (for context when flushing).
    pub task_description: String,
    /// Optional project association.
    pub project_id: Option<Uuid>,
    /// Key-value scratchpad entries.
    pub entries: Vec<WorkingMemoryEntry>,
    /// Recalled memories relevant to this task (cached to avoid re-querying).
    pub relevant_memories: Vec<RetrievedMemory>,
    /// Total estimated tokens used.
    pub total_tokens: usize,
    /// Maximum token budget for this working memory.
    pub max_tokens: usize,
    /// When this working memory was created.
    pub created_at: DateTime<Utc>,
}

// ============================================================
// WorkingMemoryManager
// ============================================================

/// Manages in-memory working memories for active tasks.
///
/// Thread-safe via `DashMap` — can be accessed from multiple tokio tasks.
pub struct WorkingMemoryManager {
    /// Active working memories keyed by task ID.
    memories: DashMap<TaskId, WorkingMemory>,
    /// Default max tokens for new working memories.
    default_max_tokens: usize,
}

impl WorkingMemoryManager {
    /// Create a new working memory manager.
    pub fn new(default_max_tokens: usize) -> Self {
        Self {
            memories: DashMap::new(),
            default_max_tokens,
        }
    }

    /// Create a new working memory for a task.
    ///
    /// If a working memory already exists for this task, it is replaced.
    pub fn create(
        &self,
        task_id: TaskId,
        task_description: &str,
        project_id: Option<Uuid>,
    ) -> WorkingMemory {
        let wm = WorkingMemory {
            task_id,
            task_description: task_description.to_string(),
            project_id,
            entries: Vec::new(),
            relevant_memories: Vec::new(),
            total_tokens: 0,
            max_tokens: self.default_max_tokens,
            created_at: Utc::now(),
        };

        self.memories.insert(task_id, wm.clone());

        tracing::debug!(
            task_id = %task_id,
            max_tokens = self.default_max_tokens,
            "Working memory created"
        );

        wm
    }

    /// Add a scratchpad entry to a task's working memory.
    ///
    /// If the key already exists, its value is replaced.
    /// Returns `false` if the task has no working memory.
    pub fn add_entry(&self, task_id: TaskId, key: &str, value: &str) -> bool {
        let mut entry = match self.memories.get_mut(&task_id) {
            Some(entry) => entry,
            None => return false,
        };

        let tokens = estimate_tokens(value);

        // Check if key already exists — replace if so
        if let Some(idx) = entry.entries.iter().position(|e| e.key == key) {
            let old_tokens = entry.entries[idx].tokens;
            entry.total_tokens = entry.total_tokens.saturating_sub(old_tokens);
            entry.entries[idx].value = value.to_string();
            entry.entries[idx].added_at = Utc::now();
            entry.entries[idx].tokens = tokens;
            entry.total_tokens += tokens;
        } else {
            entry.entries.push(WorkingMemoryEntry {
                key: key.to_string(),
                value: value.to_string(),
                added_at: Utc::now(),
                tokens,
            });
            entry.total_tokens += tokens;
        }

        // If over budget, evict oldest entries (FIFO)
        while entry.total_tokens > entry.max_tokens && !entry.entries.is_empty() {
            let evicted = entry.entries.remove(0);
            entry.total_tokens = entry.total_tokens.saturating_sub(evicted.tokens);
            tracing::debug!(
                task_id = %task_id,
                evicted_key = evicted.key,
                "Working memory entry evicted (over token budget)"
            );
        }

        true
    }

    /// Add recalled memories to a task's working memory cache.
    ///
    /// These are cached to avoid repeated database queries during a task.
    pub fn set_relevant_memories(&self, task_id: TaskId, memories: Vec<RetrievedMemory>) -> bool {
        match self.memories.get_mut(&task_id) {
            Some(mut entry) => {
                entry.relevant_memories = memories;
                true
            }
            None => false,
        }
    }

    /// Get a snapshot of a task's working memory.
    pub fn get(&self, task_id: TaskId) -> Option<WorkingMemory> {
        self.memories.get(&task_id).map(|entry| entry.clone())
    }

    /// Remove a specific entry by key from a task's working memory.
    pub fn remove_entry(&self, task_id: TaskId, key: &str) -> bool {
        match self.memories.get_mut(&task_id) {
            Some(mut entry) => {
                let initial_len = entry.entries.len();
                let tokens_to_remove: usize = entry
                    .entries
                    .iter()
                    .filter(|e| e.key == key)
                    .map(|e| e.tokens)
                    .sum();
                entry.entries.retain(|e| e.key != key);
                entry.total_tokens = entry.total_tokens.saturating_sub(tokens_to_remove);
                entry.entries.len() < initial_len
            }
            None => false,
        }
    }

    /// Flush a task's working memory: persist important entries to short-term
    /// memory and remove the in-memory data.
    ///
    /// Entries are stored as `Experience` memories in the short-term layer.
    /// The task description provides context for what was learned.
    pub async fn flush(&self, task_id: TaskId, store: &MemoryStore) -> ArgusResult<u32> {
        let wm = match self.memories.remove(&task_id) {
            Some((_, wm)) => wm,
            None => return Ok(0),
        };

        let mut stored_count = 0u32;

        for entry in &wm.entries {
            // Only persist entries with meaningful content (> 20 chars)
            if entry.value.len() < 20 {
                continue;
            }

            let content = format!(
                "[Task: {}] {}: {}",
                wm.task_description, entry.key, entry.value
            );

            let memory = NewMemory {
                content,
                layer: MemoryLayer::ShortTerm,
                memory_type: MemoryType::Experience,
                importance: 0.5,
                project_id: wm.project_id,
                task_id: Some(task_id),
                tags: vec![format!("task:{}", task_id), format!("wm_key:{}", entry.key)],
                source_type: Some("working_memory_flush".to_string()),
                source_model: None,
            };

            match store.store(memory).await {
                Ok(_) => stored_count += 1,
                Err(e) => {
                    tracing::warn!(
                        task_id = %task_id,
                        key = entry.key,
                        error = %e,
                        "Failed to persist working memory entry"
                    );
                }
            }
        }

        tracing::info!(
            task_id = %task_id,
            entries = wm.entries.len(),
            stored = stored_count,
            "Working memory flushed"
        );

        Ok(stored_count)
    }

    /// Drop a task's working memory without persisting (e.g., task cancelled).
    pub fn drop_memory(&self, task_id: TaskId) -> bool {
        self.memories.remove(&task_id).is_some()
    }

    /// Get the number of active working memories.
    pub fn active_count(&self) -> usize {
        self.memories.len()
    }

    /// Get all active task IDs with working memories.
    pub fn active_task_ids(&self) -> Vec<TaskId> {
        self.memories.iter().map(|entry| *entry.key()).collect()
    }
}

// ============================================================
// Token estimation (same heuristic as context.rs)
// ============================================================

fn estimate_tokens(text: &str) -> usize {
    if text.is_empty() {
        0
    } else {
        std::cmp::max(1, text.len() / 4)
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_working_memory() {
        let mgr = WorkingMemoryManager::new(8000);
        let task_id = Uuid::new_v4();

        let wm = mgr.create(task_id, "Test task", None);
        assert_eq!(wm.task_id, task_id);
        assert_eq!(wm.task_description, "Test task");
        assert!(wm.entries.is_empty());
        assert_eq!(wm.total_tokens, 0);
        assert_eq!(mgr.active_count(), 1);
    }

    #[test]
    fn test_add_and_get_entry() {
        let mgr = WorkingMemoryManager::new(8000);
        let task_id = Uuid::new_v4();
        mgr.create(task_id, "Test task", None);

        assert!(mgr.add_entry(task_id, "approach", "Use recursive descent parser"));

        let wm = mgr.get(task_id).unwrap();
        assert_eq!(wm.entries.len(), 1);
        assert_eq!(wm.entries[0].key, "approach");
        assert_eq!(wm.entries[0].value, "Use recursive descent parser");
        assert!(wm.total_tokens > 0);
    }

    #[test]
    fn test_replace_existing_key() {
        let mgr = WorkingMemoryManager::new(8000);
        let task_id = Uuid::new_v4();
        mgr.create(task_id, "Test task", None);

        mgr.add_entry(task_id, "status", "In progress");
        mgr.add_entry(task_id, "status", "Completed successfully");

        let wm = mgr.get(task_id).unwrap();
        assert_eq!(wm.entries.len(), 1);
        assert_eq!(wm.entries[0].value, "Completed successfully");
    }

    #[test]
    fn test_remove_entry() {
        let mgr = WorkingMemoryManager::new(8000);
        let task_id = Uuid::new_v4();
        mgr.create(task_id, "Test task", None);

        mgr.add_entry(task_id, "key1", "value1");
        mgr.add_entry(task_id, "key2", "value2");

        assert!(mgr.remove_entry(task_id, "key1"));
        let wm = mgr.get(task_id).unwrap();
        assert_eq!(wm.entries.len(), 1);
        assert_eq!(wm.entries[0].key, "key2");

        // Non-existent key
        assert!(!mgr.remove_entry(task_id, "key3"));
    }

    #[test]
    fn test_drop_memory() {
        let mgr = WorkingMemoryManager::new(8000);
        let task_id = Uuid::new_v4();
        mgr.create(task_id, "Test task", None);

        assert_eq!(mgr.active_count(), 1);
        assert!(mgr.drop_memory(task_id));
        assert_eq!(mgr.active_count(), 0);
        assert!(mgr.get(task_id).is_none());
    }

    #[test]
    fn test_nonexistent_task() {
        let mgr = WorkingMemoryManager::new(8000);
        let task_id = Uuid::new_v4();

        assert!(!mgr.add_entry(task_id, "key", "value"));
        assert!(mgr.get(task_id).is_none());
        assert!(!mgr.drop_memory(task_id));
    }

    #[test]
    fn test_token_budget_eviction() {
        // Very small budget to force eviction
        let mgr = WorkingMemoryManager::new(10);
        let task_id = Uuid::new_v4();
        mgr.create(task_id, "Test task", None);

        // Add entries that exceed the budget
        mgr.add_entry(task_id, "big1", &"x".repeat(40)); // 10 tokens
        mgr.add_entry(task_id, "big2", &"y".repeat(40)); // 10 tokens — should evict big1

        let wm = mgr.get(task_id).unwrap();
        // big1 should have been evicted
        assert!(wm.total_tokens <= 10);
    }

    #[test]
    fn test_active_task_ids() {
        let mgr = WorkingMemoryManager::new(8000);
        let t1 = Uuid::new_v4();
        let t2 = Uuid::new_v4();
        let t3 = Uuid::new_v4();

        mgr.create(t1, "Task 1", None);
        mgr.create(t2, "Task 2", None);
        mgr.create(t3, "Task 3", None);

        let ids = mgr.active_task_ids();
        assert_eq!(ids.len(), 3);
        assert!(ids.contains(&t1));
        assert!(ids.contains(&t2));
        assert!(ids.contains(&t3));
    }

    #[test]
    fn test_set_relevant_memories() {
        let mgr = WorkingMemoryManager::new(8000);
        let task_id = Uuid::new_v4();
        mgr.create(task_id, "Test task", None);

        // Setting on non-existent task returns false
        assert!(!mgr.set_relevant_memories(Uuid::new_v4(), vec![]));

        // Setting on existing task returns true
        assert!(mgr.set_relevant_memories(task_id, vec![]));
    }
}
