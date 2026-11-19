# Task Scheduler — Detailed Design

The Task Scheduler is the orchestration engine of Argus. It manages the lifecycle of all tasks across all projects, resolves dependencies, assigns work to workers, and ensures crash recovery through checkpointing.

## 1. Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                     Task Scheduler                               │
│                                                                  │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │                  Priority Queue                             │ │
│  │  BinaryHeap<SchedulableTask> sorted by effective_priority   │ │
│  └────────────────────────┬───────────────────────────────────┘ │
│                           │                                      │
│  ┌────────────────────────┴───────────────────────────────────┐ │
│  │              Dependency Graph (DAG)                          │ │
│  │  Tracks task dependencies and determines readiness           │ │
│  └────────────────────────┬───────────────────────────────────┘ │
│                           │                                      │
│  ┌──────────┐  ┌──────────┴─────────┐  ┌─────────────────────┐ │
│  │Checkpoint│  │Scheduling Loop     │  │Iteration Manager    │ │
│  │Manager   │  │(tokio interval)    │  │(retry, re-decompose)│ │
│  └──────────┘  └────────────────────┘  └─────────────────────┘ │
│                                                                  │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │              Task Registry (in-memory + PostgreSQL)          │ │
│  │  All tasks indexed by id, project_id, state, priority        │ │
│  └────────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

## 2. Task Data Model

```rust
struct Task {
    // === Identity ===
    id: Uuid,
    project_id: Uuid,
    milestone_id: Uuid,
    
    // === Content ===
    title: String,
    description: String,
    acceptance_criteria: Vec<String>,
    test_commands: Vec<String>,
    
    // === Scheduling ===
    state: TaskState,
    priority: Priority,
    effective_priority: f64,         // Computed: base priority + age boost + dependency boost
    
    // === Dependencies ===
    depends_on: Vec<Uuid>,           // Task IDs this task depends on
    blocked_by: Vec<Uuid>,           // Subset of depends_on that are not yet completed
    blocks: Vec<Uuid>,               // Tasks that depend on this task
    
    // === Assignment ===
    assigned_worker_id: Option<Uuid>,
    worker_type_hint: Option<WorkerType>,  // Preferred worker (opencode, claude-code, etc.)
    
    // === Execution ===
    attempt: u32,                    // Current attempt number (starts at 1)
    max_attempts: u32,               // Default: 3
    timeout: Duration,               // Default: 30 minutes
    started_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
    
    // === Results ===
    output: Option<TaskOutput>,
    error: Option<TaskError>,
    
    // === Checkpointing ===
    checkpoint: Option<TaskCheckpoint>,
    
    // === Metadata ===
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    estimated_minutes: Option<u32>,
    actual_minutes: Option<u32>,
    tags: Vec<String>,
    
    // === Resources ===
    resources_needed: Vec<ResourceKey>,
}

enum TaskState {
    /// Created but dependencies not yet resolved
    Pending,
    
    /// All dependencies met, ready to be scheduled
    Ready,
    
    /// In the priority queue, waiting for a worker
    Queued,
    
    /// Assigned to a worker, not yet started
    Assigned,
    
    /// Worker is actively executing this task
    InProgress,
    
    /// Task output is being tested/reviewed
    Testing,
    
    /// Task completed successfully
    Completed,
    
    /// Task failed after all retries exhausted
    Failed,
    
    /// Task cancelled (by owner or system)
    Cancelled,
    
    /// Task is paused (project paused or manual pause)
    Paused,
    
    /// Task is blocked by a failed dependency
    Blocked,
}

enum Priority {
    Critical,    // 100 - System health, data loss prevention
    Urgent,      //  80 - Owner-requested, blocking other work
    High,        //  60 - Important feature work
    Normal,      //  40 - Regular tasks (default)
    Low,         //  20 - Nice-to-have, improvements
    Background,  //   5 - Self-improvement, cleanup, optimization
}

struct TaskOutput {
    /// Files modified
    files_changed: Vec<String>,
    
    /// Test results
    test_results: Option<TestResults>,
    
    /// Code review result
    review_result: Option<ReviewResult>,
    
    /// Git commit info
    commit: Option<CommitInfo>,
    
    /// LLM tokens used
    tokens_used: TokenUsage,
    
    /// Worker's final message
    summary: String,
}

struct TaskError {
    error_type: TaskErrorType,
    message: String,
    stack_trace: Option<String>,
    attempt: u32,
    occurred_at: DateTime<Utc>,
}

enum TaskErrorType {
    CompilationFailed,
    TestsFailed,
    ReviewRejected,
    WorkerCrashed,
    Timeout,
    DependencyFailed,
    ResourceExhausted,
    SecurityViolation,
    Unknown(String),
}
```

## 3. State Machine

```
                    ┌──────────┐
                    │ Pending  │ ◄── Task created by Brain Engine
                    └────┬─────┘
                         │ All dependencies completed
                         ▼
                    ┌──────────┐
                    │  Ready   │
                    └────┬─────┘
                         │ Added to priority queue
                         ▼
                    ┌──────────┐
                    │  Queued  │
                    └────┬─────┘
                         │ Worker available, assigned
                         ▼
                    ┌──────────┐
                    │ Assigned │
                    └────┬─────┘
                         │ Worker starts execution
                         ▼
                    ┌──────────┐
              ┌────►│InProgress│◄────────────────────────┐
              │     └────┬─────┘                          │
              │          │ Worker reports completion       │
              │          ▼                                │
              │     ┌──────────┐                          │
              │     │ Testing  │                          │
              │     └────┬─────┘                          │
              │          │                                │
              │     ┌────┴────────┐                       │
              │     │             │                       │
              │   Pass          Fail                      │
              │     │             │                       │
              │     ▼             ▼                       │
              │ ┌──────────┐ ┌───────┐   attempt < max?  │
              │ │Completed │ │ Retry │───── Yes ──────────┘
              │ └──────────┘ └───┬───┘
              │                  │ No
              │                  ▼
              │             ┌──────────┐
              │             │  Failed  │
              │             └──────────┘
              │
              │ (Resume from checkpoint after crash)
              │
         ┌────┴─────┐
         │ Paused   │ ◄── Project paused or manual pause
         └──────────┘

         ┌──────────┐
         │ Blocked  │ ◄── Dependency failed
         └──────────┘

         ┌──────────┐
         │Cancelled │ ◄── Owner or system cancellation
         └──────────┘
```

### State Transitions

| From | To | Trigger |
|------|-----|---------|
| Pending | Ready | All `depends_on` tasks are Completed |
| Pending | Blocked | A dependency entered Failed state |
| Ready | Queued | Scheduler picks up the task |
| Queued | Assigned | Worker Manager assigns a worker |
| Assigned | InProgress | Worker begins execution |
| InProgress | Testing | Worker reports task output |
| InProgress | Failed | Worker crashes or timeout |
| Testing | Completed | Tests pass + review OK |
| Testing | InProgress (retry) | Tests fail, attempt < max_attempts |
| Testing | Failed | Tests fail, attempt >= max_attempts |
| Any | Paused | Project paused or manual pause |
| Paused | Ready/Queued/InProgress | Resume (returns to pre-pause state) |
| Any | Cancelled | Owner cancels or system cleanup |

## 4. Dependency Graph

The dependency graph is a Directed Acyclic Graph (DAG) that tracks which tasks must complete before others can begin.

```rust
struct DependencyGraph {
    /// Forward edges: task → tasks it blocks
    blocks: HashMap<Uuid, HashSet<Uuid>>,
    
    /// Reverse edges: task → tasks it depends on
    depends_on: HashMap<Uuid, HashSet<Uuid>>,
    
    /// Unresolved dependency count per task
    unresolved_count: HashMap<Uuid, usize>,
}

impl DependencyGraph {
    /// Add a task with its dependencies
    fn add_task(&mut self, task_id: Uuid, dependencies: &[Uuid]) {
        self.depends_on.insert(task_id, dependencies.iter().cloned().collect());
        self.unresolved_count.insert(task_id, dependencies.len());
        
        for dep in dependencies {
            self.blocks.entry(*dep).or_default().insert(task_id);
        }
    }
    
    /// Mark a task as completed and return newly ready tasks
    fn mark_completed(&mut self, task_id: Uuid) -> Vec<Uuid> {
        let mut newly_ready = Vec::new();
        
        if let Some(blocked_tasks) = self.blocks.get(&task_id) {
            for blocked_id in blocked_tasks {
                if let Some(count) = self.unresolved_count.get_mut(blocked_id) {
                    *count -= 1;
                    if *count == 0 {
                        newly_ready.push(*blocked_id);
                    }
                }
            }
        }
        
        newly_ready
    }
    
    /// Mark a task as failed and return all transitively blocked tasks
    fn mark_failed(&mut self, task_id: Uuid) -> Vec<Uuid> {
        let mut blocked = Vec::new();
        let mut queue = vec![task_id];
        
        while let Some(id) = queue.pop() {
            if let Some(downstream) = self.blocks.get(&id) {
                for blocked_id in downstream {
                    blocked.push(*blocked_id);
                    queue.push(*blocked_id); // Transitively block
                }
            }
        }
        
        blocked
    }
    
    /// Validate no circular dependencies
    fn validate(&self) -> Result<(), Vec<Uuid>> {
        // Topological sort using Kahn's algorithm
        let mut in_degree = self.unresolved_count.clone();
        let mut queue: VecDeque<Uuid> = in_degree.iter()
            .filter(|(_, &count)| count == 0)
            .map(|(id, _)| *id)
            .collect();
        
        let mut visited = 0;
        while let Some(id) = queue.pop_front() {
            visited += 1;
            if let Some(blocked) = self.blocks.get(&id) {
                for blocked_id in blocked {
                    if let Some(count) = in_degree.get_mut(blocked_id) {
                        *count -= 1;
                        if *count == 0 {
                            queue.push_back(*blocked_id);
                        }
                    }
                }
            }
        }
        
        if visited == in_degree.len() {
            Ok(())
        } else {
            // Circular dependency detected
            let cyclic: Vec<Uuid> = in_degree.iter()
                .filter(|(_, &count)| count > 0)
                .map(|(id, _)| *id)
                .collect();
            Err(cyclic)
        }
    }
}
```

## 5. Priority Queue

```rust
struct PriorityQueue {
    heap: BinaryHeap<SchedulableTask>,
}

struct SchedulableTask {
    task_id: Uuid,
    effective_priority: f64,
    enqueued_at: DateTime<Utc>,
}

impl SchedulableTask {
    /// Compute effective priority with aging boost
    fn compute_effective_priority(task: &Task) -> f64 {
        let base = task.priority.value() as f64;
        
        // Age boost: tasks waiting longer get priority boost
        // +1 priority point per minute waiting (capped at +20)
        let age_minutes = (Utc::now() - task.created_at).num_minutes() as f64;
        let age_boost = (age_minutes * 1.0).min(20.0);
        
        // Dependency boost: tasks that block many other tasks get priority
        let blocked_count = task.blocks.len() as f64;
        let dep_boost = (blocked_count * 5.0).min(30.0);
        
        // Retry penalty: tasks that have failed before get slightly lower priority
        let retry_penalty = (task.attempt as f64 - 1.0) * 5.0;
        
        base + age_boost + dep_boost - retry_penalty
    }
}

impl Ord for SchedulableTask {
    fn cmp(&self, other: &Self) -> Ordering {
        self.effective_priority
            .partial_cmp(&other.effective_priority)
            .unwrap_or(Ordering::Equal)
            .then_with(|| self.enqueued_at.cmp(&other.enqueued_at).reverse()) // Earlier first
    }
}
```

## 6. Scheduling Loop

The main scheduling loop runs on a tokio interval:

```rust
impl TaskScheduler {
    /// Main scheduling loop — runs every 500ms
    async fn run_scheduling_loop(&self) {
        let mut interval = tokio::time::interval(Duration::from_millis(500));
        
        loop {
            interval.tick().await;
            
            // 1. Check for newly ready tasks (dependencies resolved)
            self.promote_ready_tasks().await;
            
            // 2. Check for timed-out tasks
            self.check_timeouts().await;
            
            // 3. Try to assign queued tasks to available workers
            self.assign_tasks().await;
            
            // 4. Update effective priorities (aging)
            self.update_priorities().await;
        }
    }
    
    /// Move Pending tasks to Ready when dependencies are met
    async fn promote_ready_tasks(&self) {
        let pending = self.registry.get_tasks_by_state(TaskState::Pending).await;
        
        for task in pending {
            if self.dep_graph.unresolved_count(&task.id) == 0 {
                self.transition(task.id, TaskState::Ready).await;
                self.queue.push(SchedulableTask::from(&task));
                self.event_bus.emit(ArgusEvent::TaskQueued(task.id)).await;
            }
        }
    }
    
    /// Assign queued tasks to available workers
    async fn assign_tasks(&self) {
        while let Some(schedulable) = self.queue.peek() {
            // Ask Worker Manager for an available worker
            let worker = match self.worker_manager.request_worker(
                &self.registry.get_task(schedulable.task_id).await,
            ).await {
                Some(w) => w,
                None => break, // No workers available
            };
            
            let task = self.queue.pop().unwrap();
            self.transition(task.task_id, TaskState::Assigned).await;
            
            // Assign to worker
            self.worker_manager.assign_task(worker.id, task.task_id).await;
            self.event_bus.emit(
                ArgusEvent::TaskAssigned(task.task_id, worker.id)
            ).await;
        }
    }
    
    /// Detect and handle timed-out tasks
    async fn check_timeouts(&self) {
        let in_progress = self.registry.get_tasks_by_state(TaskState::InProgress).await;
        
        for task in in_progress {
            if let Some(started) = task.started_at {
                if Utc::now() - started > task.timeout {
                    tracing::warn!("Task {} timed out after {:?}", task.id, task.timeout);
                    self.handle_task_failure(task.id, TaskError {
                        error_type: TaskErrorType::Timeout,
                        message: format!("Task exceeded timeout of {:?}", task.timeout),
                        ..Default::default()
                    }).await;
                }
            }
        }
    }
}
```

## 7. Task Completion Handling

```rust
impl TaskScheduler {
    /// Handle task completion from worker
    async fn handle_task_completed(&self, task_id: Uuid, output: TaskOutput) {
        // 1. Update task state
        let task = self.registry.get_task(task_id).await;
        self.transition(task_id, TaskState::Testing).await;
        
        // 2. Run acceptance tests
        let test_ok = self.run_acceptance_tests(&task, &output).await;
        
        if !test_ok {
            self.handle_task_failure(task_id, TaskError {
                error_type: TaskErrorType::TestsFailed,
                message: "Acceptance tests failed".into(),
                attempt: task.attempt,
                ..Default::default()
            }).await;
            return;
        }
        
        // 3. Mark as completed
        self.transition(task_id, TaskState::Completed).await;
        self.registry.set_output(task_id, output.clone()).await;
        
        // 4. Resolve dependencies — find newly ready tasks
        let newly_ready = self.dep_graph.mark_completed(task_id);
        for ready_id in newly_ready {
            self.transition(ready_id, TaskState::Ready).await;
            let ready_task = self.registry.get_task(ready_id).await;
            self.queue.push(SchedulableTask::from(&ready_task));
        }
        
        // 5. Record actual time
        if let Some(started) = task.started_at {
            let actual = (Utc::now() - started).num_minutes() as u32;
            self.registry.set_actual_minutes(task_id, actual).await;
        }
        
        // 6. Emit event
        self.event_bus.emit(ArgusEvent::TaskCompleted(task_id, output)).await;
        
        // 7. Check if milestone is complete
        self.check_milestone_completion(task.milestone_id).await;
    }
    
    /// Handle task failure
    async fn handle_task_failure(&self, task_id: Uuid, error: TaskError) {
        let task = self.registry.get_task(task_id).await;
        
        if task.attempt < task.max_attempts {
            // Retry
            tracing::info!(
                "Task {} failed (attempt {}/{}), retrying",
                task_id, task.attempt, task.max_attempts
            );
            
            self.registry.increment_attempt(task_id).await;
            self.registry.set_error(task_id, error.clone()).await;
            
            // Save checkpoint before retry
            self.checkpoint_manager.save(task_id).await;
            
            // Brief delay before retry (exponential backoff)
            let delay = Duration::from_secs(2u64.pow(task.attempt));
            tokio::time::sleep(delay).await;
            
            // Re-queue
            self.transition(task_id, TaskState::Ready).await;
            let task = self.registry.get_task(task_id).await;
            self.queue.push(SchedulableTask::from(&task));
            
            self.event_bus.emit(
                ArgusEvent::TaskRetrying(task_id, task.attempt + 1)
            ).await;
        } else {
            // All retries exhausted
            tracing::error!(
                "Task {} failed permanently after {} attempts",
                task_id, task.max_attempts
            );
            
            self.transition(task_id, TaskState::Failed).await;
            self.registry.set_error(task_id, error.clone()).await;
            
            // Block downstream tasks
            let blocked = self.dep_graph.mark_failed(task_id);
            for blocked_id in &blocked {
                self.transition(*blocked_id, TaskState::Blocked).await;
            }
            
            self.event_bus.emit(ArgusEvent::TaskFailed(task_id, error)).await;
            
            // Ask Brain Engine to analyze failure and potentially re-decompose
            self.brain_engine.analyze_task_failure(task_id).await;
        }
    }
}
```

## 8. Checkpointing

Checkpointing enables crash recovery — if Argus crashes mid-task, it can resume from the last checkpoint.

```rust
struct TaskCheckpoint {
    task_id: Uuid,
    checkpoint_at: DateTime<Utc>,
    
    /// Task state at checkpoint time
    state: TaskState,
    attempt: u32,
    
    /// Worker state
    worker_id: Option<Uuid>,
    worker_type: Option<WorkerType>,
    
    /// Files modified so far
    files_changed: Vec<String>,
    
    /// Git state
    last_commit: Option<String>,
    branch: Option<String>,
    
    /// Progress indicator
    progress_pct: f32,
    
    /// Serialized worker-specific state
    worker_state: Option<serde_json::Value>,
}

struct CheckpointManager {
    pool: PgPool,
}

impl CheckpointManager {
    /// Save a checkpoint for a task
    async fn save(&self, task_id: Uuid) -> Result<()> {
        let task = self.registry.get_task(task_id).await;
        let worker_state = self.worker_manager.get_worker_state(task_id).await;
        
        let checkpoint = TaskCheckpoint {
            task_id,
            checkpoint_at: Utc::now(),
            state: task.state,
            attempt: task.attempt,
            worker_id: task.assigned_worker_id,
            worker_type: task.worker_type_hint,
            files_changed: task.output.as_ref()
                .map(|o| o.files_changed.clone())
                .unwrap_or_default(),
            last_commit: self.git_manager.last_commit(task.project_id).await.ok(),
            branch: self.git_manager.current_branch(task.project_id).await.ok(),
            progress_pct: task.progress_pct(),
            worker_state,
        };
        
        sqlx::query!(
            r#"
            INSERT INTO task_checkpoints (task_id, checkpoint_at, state_json)
            VALUES ($1, $2, $3)
            ON CONFLICT (task_id) DO UPDATE SET
                checkpoint_at = $2,
                state_json = $3
            "#,
            task_id,
            checkpoint.checkpoint_at,
            serde_json::to_value(&checkpoint)?,
        )
        .execute(&self.pool)
        .await?;
        
        Ok(())
    }
    
    /// Restore tasks from checkpoints after crash recovery
    async fn restore_all(&self) -> Vec<RestoredTask> {
        let checkpoints = sqlx::query_as!(
            CheckpointRow,
            "SELECT * FROM task_checkpoints WHERE state_json->>'state' IN ('InProgress', 'Assigned', 'Queued')"
        )
        .fetch_all(&self.pool)
        .await?;
        
        let mut restored = Vec::new();
        
        for cp in checkpoints {
            let checkpoint: TaskCheckpoint = serde_json::from_value(cp.state_json)?;
            
            tracing::info!(
                "Restoring task {} from checkpoint (was {:?}, attempt {})",
                checkpoint.task_id, checkpoint.state, checkpoint.attempt
            );
            
            // Reset to Ready state for re-scheduling
            // The worker state is lost on crash, so we restart the task
            self.scheduler.transition(checkpoint.task_id, TaskState::Ready).await;
            
            restored.push(RestoredTask {
                task_id: checkpoint.task_id,
                previous_state: checkpoint.state,
                previous_progress: checkpoint.progress_pct,
            });
        }
        
        restored
    }
}
```

## 9. Iteration Manager

When a task fails permanently or the Brain determines the approach needs changing, the Iteration Manager handles re-decomposition:

```rust
struct IterationManager {
    brain: Arc<BrainEngine>,
    scheduler: Arc<TaskScheduler>,
}

impl IterationManager {
    /// Re-decompose a failed task into smaller or different subtasks
    async fn iterate_on_failure(&self, task_id: Uuid) {
        let task = self.scheduler.registry.get_task(task_id).await;
        let error = task.error.as_ref().unwrap();
        
        // Ask Brain to analyze and re-decompose
        let analysis = self.brain.analyze_failure(
            &task.description,
            &error.message,
            &task.output,
        ).await;
        
        match analysis.recommendation {
            FailureAction::Retry { modified_description } => {
                // Create a new task with modified approach
                let new_task = Task {
                    id: Uuid::new_v4(),
                    description: modified_description,
                    depends_on: task.depends_on.clone(),
                    blocks: task.blocks.clone(),
                    attempt: 0,
                    ..task.clone()
                };
                self.scheduler.add_task(new_task).await;
            }
            
            FailureAction::Decompose { subtasks } => {
                // Break into smaller tasks
                for subtask in &subtasks {
                    self.scheduler.add_task(subtask.clone()).await;
                }
                // Update dependency graph: downstream tasks now depend on subtasks
                self.scheduler.dep_graph.replace_with_subtasks(
                    task_id,
                    subtasks.iter().map(|t| t.id).collect(),
                ).await;
            }
            
            FailureAction::Escalate => {
                // Cannot be resolved automatically — notify owner
                self.event_bus.emit(ArgusEvent::TaskFailed(task_id, error.clone())).await;
            }
            
            FailureAction::Skip => {
                // Mark as cancelled and unblock downstream
                self.scheduler.transition(task_id, TaskState::Cancelled).await;
            }
        }
    }
}
```

## 10. Milestone Tracking

Tasks are grouped into milestones. Milestone completion triggers project-level events:

```rust
struct Milestone {
    id: Uuid,
    project_id: Uuid,
    name: String,
    description: String,
    order: u32,                      // Execution order within project
    
    // Derived from child tasks
    total_tasks: u32,
    completed_tasks: u32,
    failed_tasks: u32,
    
    state: MilestoneState,
    started_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
}

enum MilestoneState {
    Pending,       // No tasks started yet
    InProgress,    // Some tasks started
    Completed,     // All tasks completed
    PartialFail,   // Some tasks failed, but milestone can proceed
    Failed,        // Critical tasks failed, milestone blocked
}

impl TaskScheduler {
    async fn check_milestone_completion(&self, milestone_id: Uuid) {
        let milestone = self.registry.get_milestone(milestone_id).await;
        let tasks = self.registry.get_tasks_by_milestone(milestone_id).await;
        
        let all_done = tasks.iter().all(|t| 
            t.state == TaskState::Completed || t.state == TaskState::Cancelled
        );
        
        if all_done {
            self.registry.set_milestone_state(milestone_id, MilestoneState::Completed).await;
            
            // Notify
            self.event_bus.emit(ArgusEvent::ProjectUpdated(ProjectUpdatedEvent {
                project_id: milestone.project_id,
                update_type: "milestone_completed".into(),
                details: format!("Milestone '{}' completed", milestone.name),
            })).await;
            
            // Check if all milestones are done → project complete
            self.check_project_completion(milestone.project_id).await;
        }
    }
}
```

## 11. Cross-Project Conflict Detection

When multiple projects are running simultaneously, the scheduler detects and prevents conflicts:

```rust
struct ConflictDetector {
    active_resources: HashMap<ResourceKey, (Uuid, Uuid)>,  // resource → (project_id, task_id)
}

enum ResourceKey {
    Port(u16),                        // Network port
    FilePath(PathBuf),                // File being modified
    DockerContainer(String),          // Container name
    DatabaseName(String),             // Database name
    GitRepo(String),                  // Repository path
}

impl ConflictDetector {
    /// Check if a task would conflict with currently running tasks
    fn check_conflicts(&self, task: &Task) -> Vec<Conflict> {
        let mut conflicts = Vec::new();
        
        for resource in &task.resources_needed {
            if let Some((project_id, other_task_id)) = self.active_resources.get(resource) {
                if *project_id != task.project_id {
                    conflicts.push(Conflict {
                        resource: resource.clone(),
                        conflicting_project: *project_id,
                        conflicting_task: *other_task_id,
                    });
                }
            }
        }
        
        conflicts
    }
    
    /// Register resources being used by a task
    fn register(&mut self, task: &Task) {
        for resource in &task.resources_needed {
            self.active_resources.insert(
                resource.clone(),
                (task.project_id, task.id),
            );
        }
    }
    
    /// Release resources when a task completes
    fn release(&mut self, task: &Task) {
        for resource in &task.resources_needed {
            self.active_resources.remove(resource);
        }
    }
}
```

## 12. Scheduler Statistics

```rust
struct SchedulerStats {
    // Queue metrics
    queue_depth: usize,
    tasks_pending: usize,
    tasks_in_progress: usize,
    tasks_completed_today: usize,
    tasks_failed_today: usize,
    
    // Timing
    avg_wait_time: Duration,         // From Ready → InProgress
    avg_execution_time: Duration,    // From InProgress → Completed
    avg_total_time: Duration,        // From Created → Completed
    
    // Accuracy
    estimation_accuracy: f64,        // estimated_minutes vs actual_minutes
    
    // By project
    per_project: HashMap<Uuid, ProjectSchedulerStats>,
}
```

## 13. Conversational Task Management

```
User: "暂停项目X的所有任务"
Argus: "已暂停项目X的任务:
        - 2个正在执行的任务已发送暂停信号
        - 5个排队中的任务已移出队列
        - 3个等待中的任务保持不变
        恢复时请说'继续项目X'"

User: "最高优先级处理那个登录bug"
Argus: "已将任务 'Fix login redirect loop' 优先级提升到 Urgent(80)。
        当前前面还有1个Critical任务在执行，预计5分钟后开始处理。"

User: "那个任务做到哪了？"
Argus: "任务 'Implement payment integration':
        状态: InProgress (尝试 1/3)
        进度: ~60% (worker: opencode)
        已完成: API接口定义、数据库schema、基础SDK集成
        进行中: 支付回调处理
        预计剩余: 15分钟"
```
