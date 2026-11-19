//! Priority queue for scheduling tasks.
//!
//! Uses a binary heap ordered by effective priority.
//! Tasks with higher effective priority are popped first.
//! For equal priorities, tasks that have been waiting longer are prioritized.

use chrono::{DateTime, Utc};
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use uuid::Uuid;

use crate::task_types::Task;

/// A schedulable task entry in the priority queue.
#[derive(Clone, Debug)]
pub struct SchedulableTask {
    /// Task ID
    pub task_id: Uuid,
    /// Effective priority (higher = more urgent)
    pub effective_priority: f64,
    /// When the task was added to the queue
    pub enqueued_at: DateTime<Utc>,
}

impl SchedulableTask {
    /// Create a new schedulable task entry.
    pub fn new(task_id: Uuid, effective_priority: f64) -> Self {
        Self {
            task_id,
            effective_priority,
            enqueued_at: Utc::now(),
        }
    }

    /// Create from a Task, computing effective priority.
    pub fn from_task(task: &Task) -> Self {
        Self::new(task.id, task.effective_priority)
    }

    /// Compute effective priority with age and dependency boosts.
    ///
    /// # Arguments
    /// * `base_priority` - The task's base priority value
    /// * `age_minutes` - How long the task has been waiting
    /// * `blocked_count` - How many tasks depend on this task
    /// * `attempt` - Current attempt number
    pub fn compute_effective_priority(
        base_priority: f64,
        age_minutes: f64,
        blocked_count: usize,
        attempt: u32,
    ) -> f64 {
        // Age boost: +1 priority point per minute, capped at +20
        let age_boost = (age_minutes * 1.0).min(20.0);

        // Dependency boost: tasks that block many others get priority
        // +5 points per blocked task, capped at +30
        let dep_boost = (blocked_count as f64 * 5.0).min(30.0);

        // Retry penalty: failed tasks get slightly lower priority
        // -5 points per previous failure
        let retry_penalty = (attempt.saturating_sub(1) as f64) * 5.0;

        base_priority + age_boost + dep_boost - retry_penalty
    }
}

impl PartialEq for SchedulableTask {
    fn eq(&self, other: &Self) -> bool {
        // Treat as equal only if same priority and enqueued at same time
        // (unlikely, but makes the trait implementation consistent)
        self.effective_priority == other.effective_priority && self.enqueued_at == other.enqueued_at
    }
}

impl Eq for SchedulableTask {}

impl PartialOrd for SchedulableTask {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SchedulableTask {
    fn cmp(&self, other: &Self) -> Ordering {
        // BinaryHeap::pop returns the MAXIMUM element
        // So we want HIGHER priority to be GREATER
        let priority_cmp = self
            .effective_priority
            .partial_cmp(&other.effective_priority)
            .unwrap_or(Ordering::Equal);

        // If priorities are equal, earlier enqueued_at comes first
        // (earlier time should be GREATER to come out first in max-heap)
        priority_cmp.then_with(|| other.enqueued_at.cmp(&self.enqueued_at))
    }
}

/// Priority queue for task scheduling.
#[derive(Clone, Debug, Default)]
pub struct TaskPriorityQueue {
    heap: BinaryHeap<SchedulableTask>,
    /// Track which tasks are in the queue to avoid duplicates
    in_queue: std::collections::HashSet<Uuid>,
}

impl TaskPriorityQueue {
    /// Create a new empty priority queue.
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a task into the queue.
    ///
    /// Returns true if the task was added, false if already in queue.
    pub fn push(&mut self, task: SchedulableTask) -> bool {
        if self.in_queue.insert(task.task_id) {
            self.heap.push(task);
            true
        } else {
            false
        }
    }

    /// Pop the highest priority task from the queue.
    pub fn pop(&mut self) -> Option<SchedulableTask> {
        while let Some(task) = self.heap.pop() {
            if self.in_queue.remove(&task.task_id) {
                return Some(task);
            }
        }
        None
    }

    /// Peek at the highest priority task without removing it.
    pub fn peek(&self) -> Option<&SchedulableTask> {
        self.heap.peek()
    }

    /// Check if a task is in the queue.
    pub fn contains(&self, task_id: &Uuid) -> bool {
        self.in_queue.contains(task_id)
    }

    /// Get the number of tasks in the queue.
    pub fn len(&self) -> usize {
        self.in_queue.len()
    }

    /// Check if the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.in_queue.is_empty()
    }

    /// Remove a specific task from the queue.
    ///
    /// Note: This is O(n) because we need to find the task in the heap.
    /// For better performance, consider using a different data structure
    /// or lazily removing during pop.
    pub fn remove(&mut self, task_id: Uuid) -> bool {
        if self.in_queue.remove(&task_id) {
            // We can't efficiently remove from BinaryHeap, so we'll
            // lazily remove it during pop. Just mark it as not in queue.
            true
        } else {
            false
        }
    }

    /// Get all task IDs currently in the queue.
    pub fn task_ids(&self) -> &std::collections::HashSet<Uuid> {
        &self.in_queue
    }

    /// Clear all tasks from the queue.
    pub fn clear(&mut self) {
        self.heap.clear();
        self.in_queue.clear();
    }

    /// Recompute effective priorities for all tasks.
    ///
    /// This is useful when tasks have been waiting for a long time
    /// and need their priority boosted.
    pub fn recompute_priorities<F>(&mut self, mut compute_fn: F)
    where
        F: FnMut(Uuid) -> f64,
    {
        // We need to rebuild the heap since priorities changed
        let mut new_heap = BinaryHeap::with_capacity(self.heap.len());

        for task in &self.heap {
            let new_priority = compute_fn(task.task_id);
            new_heap.push(SchedulableTask {
                task_id: task.task_id,
                effective_priority: new_priority,
                enqueued_at: task.enqueued_at,
            });
        }

        self.heap = new_heap;
    }

    /// Get statistics about the queue.
    pub fn stats(&self) -> QueueStats {
        let mut min_priority = f64::INFINITY;
        let mut max_priority = f64::NEG_INFINITY;
        let mut total_priority = 0.0;
        let count = self.in_queue.len();

        for task in &self.heap {
            if self.in_queue.contains(&task.task_id) {
                min_priority = min_priority.min(task.effective_priority);
                max_priority = max_priority.max(task.effective_priority);
                total_priority += task.effective_priority;
            }
        }

        QueueStats {
            count,
            min_priority: if count > 0 { Some(min_priority) } else { None },
            max_priority: if count > 0 { Some(max_priority) } else { None },
            avg_priority: if count > 0 {
                Some(total_priority / count as f64)
            } else {
                None
            },
        }
    }
}

/// Statistics about the priority queue.
#[derive(Clone, Debug, Default)]
pub struct QueueStats {
    pub count: usize,
    pub min_priority: Option<f64>,
    pub max_priority: Option<f64>,
    pub avg_priority: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task_types::{Priority, Task};

    fn create_task_with_priority(id: Uuid, priority: Priority, blocks: Vec<Uuid>) -> Task {
        let mut task = Task::new(Uuid::new_v4(), "Test", "Test");
        task.id = id;
        task.priority = priority;
        task.effective_priority = priority.value() as f64;
        task.blocks = blocks;
        task
    }

    #[test]
    fn test_schedulable_task_ordering() {
        let t1 = SchedulableTask::new(Uuid::new_v4(), 100.0);
        let t2 = SchedulableTask::new(Uuid::new_v4(), 50.0);

        // In a max-heap, the maximum element comes out first
        // So t1 (100) should be "greater" than t2 (50)
        assert!(t1 > t2);
        assert!(t2 < t1);
    }

    #[test]
    fn test_schedulable_task_equal_priority() {
        let now = Utc::now();
        let earlier = now - chrono::Duration::seconds(10);
        let later = now;

        let t1 = SchedulableTask {
            task_id: Uuid::new_v4(),
            effective_priority: 50.0,
            enqueued_at: earlier,
        };
        let t2 = SchedulableTask {
            task_id: Uuid::new_v4(),
            effective_priority: 50.0,
            enqueued_at: later,
        };

        // Earlier task should come first (be "greater" in max-heap terms)
        // when priorities are equal
        assert!(t1 > t2);
    }

    #[test]
    fn test_compute_effective_priority() {
        let priority = SchedulableTask::compute_effective_priority(
            40.0, // base
            10.0, // 10 minutes age
            2,    // blocks 2 tasks
            1,    // first attempt
        );

        // Expected: 40 + 10 + 10 - 0 = 60
        assert!((priority - 60.0).abs() < 0.001);

        // Test with retry
        let priority_with_retry = SchedulableTask::compute_effective_priority(
            40.0, 10.0, 2, 3, // third attempt
        );

        // Expected: 40 + 10 + 10 - 10 = 50
        assert!((priority_with_retry - 50.0).abs() < 0.001);
    }

    #[test]
    fn test_priority_queue_push_pop() {
        let mut queue = TaskPriorityQueue::new();
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        let id3 = Uuid::new_v4();

        queue.push(SchedulableTask::new(id1, 50.0));
        queue.push(SchedulableTask::new(id2, 100.0));
        queue.push(SchedulableTask::new(id3, 75.0));

        assert_eq!(queue.len(), 3);

        // Should pop in priority order: 100, 75, 50
        let first = queue.pop().unwrap();
        assert_eq!(first.task_id, id2);
        assert_eq!(first.effective_priority, 100.0);

        let second = queue.pop().unwrap();
        assert_eq!(second.task_id, id3);

        let third = queue.pop().unwrap();
        assert_eq!(third.task_id, id1);

        assert!(queue.is_empty());
    }

    #[test]
    fn test_priority_queue_duplicate_rejection() {
        let mut queue = TaskPriorityQueue::new();
        let id = Uuid::new_v4();

        assert!(queue.push(SchedulableTask::new(id, 100.0)));
        assert!(!queue.push(SchedulableTask::new(id, 50.0))); // Duplicate

        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn test_priority_queue_peek() {
        let mut queue = TaskPriorityQueue::new();
        let id = Uuid::new_v4();

        queue.push(SchedulableTask::new(id, 100.0));

        let peeked = queue.peek().unwrap();
        assert_eq!(peeked.task_id, id);

        // Peek shouldn't remove
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn test_priority_queue_from_task() {
        let task = create_task_with_priority(Uuid::new_v4(), Priority::High, vec![]);
        let schedulable = SchedulableTask::from_task(&task);

        assert_eq!(schedulable.task_id, task.id);
        assert_eq!(schedulable.effective_priority, 60.0);
    }

    #[test]
    fn test_priority_queue_contains() {
        let mut queue = TaskPriorityQueue::new();
        let id = Uuid::new_v4();

        assert!(!queue.contains(&id));
        queue.push(SchedulableTask::new(id, 100.0));
        assert!(queue.contains(&id));
    }

    #[test]
    fn test_priority_queue_remove() {
        let mut queue = TaskPriorityQueue::new();
        let id = Uuid::new_v4();

        queue.push(SchedulableTask::new(id, 100.0));
        assert!(queue.remove(id));
        assert!(!queue.contains(&id));
        assert_eq!(queue.len(), 0);
    }

    #[test]
    fn test_priority_queue_clear() {
        let mut queue = TaskPriorityQueue::new();

        queue.push(SchedulableTask::new(Uuid::new_v4(), 100.0));
        queue.push(SchedulableTask::new(Uuid::new_v4(), 50.0));

        queue.clear();
        assert!(queue.is_empty());
        assert_eq!(queue.len(), 0);
    }

    #[test]
    fn test_queue_stats() {
        let mut queue = TaskPriorityQueue::new();

        queue.push(SchedulableTask::new(Uuid::new_v4(), 100.0));
        queue.push(SchedulableTask::new(Uuid::new_v4(), 50.0));
        queue.push(SchedulableTask::new(Uuid::new_v4(), 75.0));

        let stats = queue.stats();
        assert_eq!(stats.count, 3);
        assert_eq!(stats.min_priority, Some(50.0));
        assert_eq!(stats.max_priority, Some(100.0));
        assert!((stats.avg_priority.unwrap() - 75.0).abs() < 0.001);
    }
}
