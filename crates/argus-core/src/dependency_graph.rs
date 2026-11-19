//! Dependency graph for tracking task dependencies.
//!
//! Uses a DAG (Directed Acyclic Graph) to ensure no circular dependencies
//! and efficiently determine which tasks become ready when dependencies complete.

use std::collections::{HashMap, HashSet, VecDeque};
use uuid::Uuid;

/// Dependency graph for tasks.
#[derive(Clone, Debug, Default)]
pub struct DependencyGraph {
    /// Forward edges: task → tasks it blocks
    blocks: HashMap<Uuid, HashSet<Uuid>>,
    /// Reverse edges: task → tasks it depends on
    depends_on: HashMap<Uuid, HashSet<Uuid>>,
    /// Unresolved dependency count per task
    unresolved_count: HashMap<Uuid, usize>,
    /// All tasks in the graph
    tasks: HashSet<Uuid>,
}

impl DependencyGraph {
    /// Create a new empty dependency graph.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a task with its dependencies to the graph.
    ///
    /// Returns true if added successfully, false if task already exists.
    pub fn add_task(&mut self, task_id: Uuid, dependencies: &[Uuid]) -> bool {
        if self.tasks.contains(&task_id) {
            return false;
        }

        self.tasks.insert(task_id);
        self.depends_on
            .insert(task_id, dependencies.iter().cloned().collect());
        self.unresolved_count.insert(task_id, dependencies.len());

        // Update forward edges
        for dep in dependencies {
            self.blocks.entry(*dep).or_default().insert(task_id);
        }

        true
    }

    /// Remove a task from the graph.
    ///
    /// Also cleans up all references to this task from other tasks' dependencies.
    pub fn remove_task(&mut self, task_id: Uuid) -> bool {
        if !self.tasks.contains(&task_id) {
            return false;
        }

        // Remove from depends_on and clean up forward edges
        if let Some(deps) = self.depends_on.remove(&task_id) {
            for dep in deps {
                if let Some(blocked) = self.blocks.get_mut(&dep) {
                    blocked.remove(&task_id);
                }
            }
        }

        // Remove from blocks and update unresolved counts
        if let Some(blocked_tasks) = self.blocks.remove(&task_id) {
            for blocked_id in blocked_tasks {
                if let Some(count) = self.unresolved_count.get_mut(&blocked_id) {
                    // This shouldn't happen in normal operation, but handle it
                    *count = count.saturating_sub(1);
                }
                if let Some(deps) = self.depends_on.get_mut(&blocked_id) {
                    deps.remove(&task_id);
                }
            }
        }

        self.unresolved_count.remove(&task_id);
        self.tasks.remove(&task_id);

        true
    }

    /// Mark a task as completed and return newly ready tasks.
    ///
    /// Newly ready tasks are those whose all dependencies are now resolved.
    pub fn mark_completed(&mut self, task_id: Uuid) -> Vec<Uuid> {
        let mut newly_ready = Vec::new();

        if let Some(blocked_tasks) = self.blocks.get(&task_id) {
            for blocked_id in blocked_tasks.clone() {
                if let Some(count) = self.unresolved_count.get_mut(&blocked_id) {
                    *count = count.saturating_sub(1);
                    if *count == 0 {
                        newly_ready.push(blocked_id);
                    }
                }
            }
        }

        // Keep the task in the graph but mark as resolved
        // (we might want to track completed tasks separately)
        self.unresolved_count.insert(task_id, 0);

        newly_ready
    }

    /// Mark a task as failed and return all transitively blocked tasks.
    ///
    /// When a task fails, all tasks that depend on it (directly or transitively)
    /// become blocked.
    pub fn mark_failed(&mut self, task_id: Uuid) -> Vec<Uuid> {
        let mut blocked = Vec::new();
        let mut queue = vec![task_id];
        let mut visited = HashSet::new();

        while let Some(id) = queue.pop() {
            if !visited.insert(id) {
                continue;
            }

            if let Some(downstream) = self.blocks.get(&id) {
                for blocked_id in downstream.clone() {
                    if !visited.contains(&blocked_id) {
                        blocked.push(blocked_id);
                        queue.push(blocked_id);
                    }
                }
            }
        }

        blocked
    }

    /// Get the number of unresolved dependencies for a task.
    pub fn unresolved_count(&self, task_id: &Uuid) -> usize {
        self.unresolved_count.get(task_id).copied().unwrap_or(0)
    }

    /// Check if a task has all dependencies resolved.
    pub fn is_ready(&self, task_id: &Uuid) -> bool {
        self.unresolved_count(task_id) == 0
    }

    /// Get tasks that are ready (all dependencies resolved).
    pub fn get_ready_tasks(&self) -> Vec<Uuid> {
        self.tasks
            .iter()
            .filter(|id| self.is_ready(id))
            .copied()
            .collect()
    }

    /// Get all tasks that depend on (are blocked by) the given task.
    pub fn get_blocked_by(&self, task_id: &Uuid) -> Vec<Uuid> {
        self.blocks
            .get(task_id)
            .map(|set| set.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Get all dependencies of a task.
    pub fn get_dependencies(&self, task_id: &Uuid) -> Vec<Uuid> {
        self.depends_on
            .get(task_id)
            .map(|set| set.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Validate that the graph has no circular dependencies.
    ///
    /// Uses Kahn's algorithm for topological sorting.
    pub fn validate(&self) -> Result<(), Vec<Uuid>> {
        let mut in_degree: HashMap<Uuid, usize> = self.unresolved_count.clone();
        let mut queue: VecDeque<Uuid> = in_degree
            .iter()
            .filter(|(_, count)| **count == 0)
            .map(|(id, _)| *id)
            .collect();

        let mut visited = 0;
        let mut processed = HashSet::new();

        while let Some(id) = queue.pop_front() {
            if !processed.insert(id) {
                continue;
            }
            visited += 1;

            if let Some(blocked) = self.blocks.get(&id) {
                for blocked_id in blocked {
                    if let Some(count) = in_degree.get_mut(blocked_id) {
                        *count = count.saturating_sub(1);
                        if *count == 0 && !processed.contains(blocked_id) {
                            queue.push_back(*blocked_id);
                        }
                    }
                }
            }
        }

        if visited == self.tasks.len() {
            Ok(())
        } else {
            // Circular dependency detected
            let cyclic: Vec<Uuid> = in_degree
                .iter()
                .filter(|(_, count)| **count > 0)
                .map(|(id, _)| *id)
                .collect();
            Err(cyclic)
        }
    }

    /// Get a topological ordering of all tasks.
    ///
    /// Returns tasks in an order where all dependencies come before dependent tasks.
    pub fn topological_sort(&self) -> Result<Vec<Uuid>, String> {
        self.validate()
            .map_err(|_| "Circular dependency detected".to_string())?;

        let mut in_degree: HashMap<Uuid, usize> = self.unresolved_count.clone();
        let mut queue: VecDeque<Uuid> = in_degree
            .iter()
            .filter(|(_, count)| **count == 0)
            .map(|(id, _)| *id)
            .collect();

        let mut result = Vec::new();
        let mut processed = HashSet::new();

        while let Some(id) = queue.pop_front() {
            if !processed.insert(id) {
                continue;
            }
            result.push(id);

            if let Some(blocked) = self.blocks.get(&id) {
                for blocked_id in blocked {
                    if let Some(count) = in_degree.get_mut(blocked_id) {
                        *count = count.saturating_sub(1);
                        if *count == 0 && !processed.contains(blocked_id) {
                            queue.push_back(*blocked_id);
                        }
                    }
                }
            }
        }

        if result.len() == self.tasks.len() {
            Ok(result)
        } else {
            Err("Topological sort incomplete".to_string())
        }
    }

    /// Check if adding a dependency would create a cycle.
    ///
    /// `from` is the task that would depend on `to`.
    /// A cycle would be created if `to` can already reach `from` through the blocking chain.
    pub fn would_create_cycle(&self, from: Uuid, to: Uuid) -> bool {
        // If 'to' can reach 'from' by following blocks edges,
        // adding from -> to would create a cycle
        let mut visited = HashSet::new();
        let mut stack = vec![to];

        while let Some(current) = stack.pop() {
            if current == from {
                return true;
            }

            if !visited.insert(current) {
                continue;
            }

            // Follow blocks edges (tasks that are blocked by current)
            if let Some(blocked_tasks) = self.blocks.get(&current) {
                for blocked in blocked_tasks {
                    stack.push(*blocked);
                }
            }
        }

        false
    }

    /// Get the total number of tasks in the graph.
    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    /// Check if the graph is empty.
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    /// Check if a task is in the graph.
    pub fn contains(&self, task_id: &Uuid) -> bool {
        self.tasks.contains(task_id)
    }

    /// Get all task IDs in the graph.
    pub fn tasks(&self) -> &HashSet<Uuid> {
        &self.tasks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uuid() -> Uuid {
        Uuid::new_v4()
    }

    #[test]
    fn test_add_task() {
        let mut graph = DependencyGraph::new();
        let t1 = uuid();

        assert!(graph.add_task(t1, &[]));
        assert!(graph.contains(&t1));
        assert_eq!(graph.len(), 1);

        // Duplicate add should fail
        assert!(!graph.add_task(t1, &[]));
    }

    #[test]
    fn test_add_task_with_dependencies() {
        let mut graph = DependencyGraph::new();
        let t1 = uuid();
        let t2 = uuid();
        let t3 = uuid();

        graph.add_task(t1, &[]);
        graph.add_task(t2, &[t1]);
        graph.add_task(t3, &[t1, t2]);

        assert_eq!(graph.unresolved_count(&t1), 0);
        assert_eq!(graph.unresolved_count(&t2), 1);
        assert_eq!(graph.unresolved_count(&t3), 2);

        assert!(graph.is_ready(&t1));
        assert!(!graph.is_ready(&t2));
        assert!(!graph.is_ready(&t3));
    }

    #[test]
    fn test_mark_completed() {
        let mut graph = DependencyGraph::new();
        let t1 = uuid();
        let t2 = uuid();
        let t3 = uuid();

        graph.add_task(t1, &[]);
        graph.add_task(t2, &[t1]);
        graph.add_task(t3, &[t2]);

        let ready = graph.mark_completed(t1);
        assert_eq!(ready, vec![t2]);

        let ready = graph.mark_completed(t2);
        assert_eq!(ready, vec![t3]);

        let ready = graph.mark_completed(t3);
        assert!(ready.is_empty());
    }

    #[test]
    fn test_mark_failed() {
        let mut graph = DependencyGraph::new();
        let t1 = uuid();
        let t2 = uuid();
        let t3 = uuid();
        let t4 = uuid();

        graph.add_task(t1, &[]);
        graph.add_task(t2, &[t1]);
        graph.add_task(t3, &[t2]);
        graph.add_task(t4, &[t1]); // Also depends on t1

        // Complete t1 first
        graph.mark_completed(t1);

        // Now fail t2
        let blocked = graph.mark_failed(t2);
        assert!(blocked.contains(&t3));
        assert!(!blocked.contains(&t4)); // t4 only depends on t1, not t2
    }

    #[test]
    fn test_remove_task() {
        let mut graph = DependencyGraph::new();
        let t1 = uuid();
        let t2 = uuid();

        graph.add_task(t1, &[]);
        graph.add_task(t2, &[t1]);

        assert_eq!(graph.unresolved_count(&t2), 1);

        graph.remove_task(t1);

        assert!(!graph.contains(&t1));
        assert_eq!(graph.unresolved_count(&t2), 0); // Dependency removed
    }

    #[test]
    fn test_validate_no_cycles() {
        let mut graph = DependencyGraph::new();
        let t1 = uuid();
        let t2 = uuid();
        let t3 = uuid();

        graph.add_task(t1, &[]);
        graph.add_task(t2, &[t1]);
        graph.add_task(t3, &[t2]);

        assert!(graph.validate().is_ok());
    }

    #[test]
    fn test_validate_with_cycle() {
        let mut graph = DependencyGraph::new();
        let t1 = uuid();
        let t2 = uuid();
        let t3 = uuid();

        graph.add_task(t1, &[t3]); // t1 depends on t3
        graph.add_task(t2, &[t1]); // t2 depends on t1
        graph.add_task(t3, &[t2]); // t3 depends on t2 - cycle!

        let result = graph.validate();
        assert!(result.is_err());

        let cyclic = result.unwrap_err();
        assert!(cyclic.contains(&t1) || cyclic.contains(&t2) || cyclic.contains(&t3));
    }

    #[test]
    fn test_topological_sort() {
        let mut graph = DependencyGraph::new();
        let t1 = uuid();
        let t2 = uuid();
        let t3 = uuid();

        graph.add_task(t1, &[]);
        graph.add_task(t2, &[t1]);
        graph.add_task(t3, &[t2]);

        let sorted = graph.topological_sort().unwrap();
        let t1_idx = sorted.iter().position(|&id| id == t1).unwrap();
        let t2_idx = sorted.iter().position(|&id| id == t2).unwrap();
        let t3_idx = sorted.iter().position(|&id| id == t3).unwrap();

        assert!(t1_idx < t2_idx);
        assert!(t2_idx < t3_idx);
    }

    #[test]
    fn test_would_create_cycle() {
        let mut graph = DependencyGraph::new();
        let t1 = uuid();
        let t2 = uuid();
        let t3 = uuid();

        graph.add_task(t1, &[]);
        graph.add_task(t2, &[t1]);
        graph.add_task(t3, &[t2]);

        // t1 -> t2 -> t3
        // Adding t3 -> t1 would create a cycle
        assert!(graph.would_create_cycle(t3, t1));

        // Adding t1 -> t3 is fine (already implied by transitivity, but no cycle)
        // Actually, this would create a parallel edge, not a cycle
        // But let's test something that doesn't create a cycle
        let t4 = uuid();
        graph.add_task(t4, &[]);
        assert!(!graph.would_create_cycle(t4, t1));
    }

    #[test]
    fn test_get_ready_tasks() {
        let mut graph = DependencyGraph::new();
        let t1 = uuid();
        let t2 = uuid();
        let t3 = uuid();

        graph.add_task(t1, &[]);
        graph.add_task(t2, &[t1]);
        graph.add_task(t3, &[]);

        let ready = graph.get_ready_tasks();
        assert_eq!(ready.len(), 2);
        assert!(ready.contains(&t1));
        assert!(ready.contains(&t3));
        assert!(!ready.contains(&t2));
    }

    #[test]
    fn test_get_blocked_by() {
        let mut graph = DependencyGraph::new();
        let t1 = uuid();
        let t2 = uuid();
        let t3 = uuid();

        graph.add_task(t1, &[]);
        graph.add_task(t2, &[t1]);
        graph.add_task(t3, &[t1]);

        let blocked = graph.get_blocked_by(&t1);
        assert_eq!(blocked.len(), 2);
        assert!(blocked.contains(&t2));
        assert!(blocked.contains(&t3));
    }

    #[test]
    fn test_get_dependencies() {
        let mut graph = DependencyGraph::new();
        let t1 = uuid();
        let t2 = uuid();
        let t3 = uuid();

        graph.add_task(t1, &[]);
        graph.add_task(t2, &[t1]);
        graph.add_task(t3, &[t1, t2]);

        let deps = graph.get_dependencies(&t3);
        assert_eq!(deps.len(), 2);
        assert!(deps.contains(&t1));
        assert!(deps.contains(&t2));
    }
}
