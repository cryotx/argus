//! Task types for the Task Scheduler.
//!
//! Defines the Task data model, states, priorities, and related structures.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::time::Duration;
use uuid::Uuid;

use crate::worker_types::WorkerType;

/// Task priority levels with numeric values.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, sqlx::Type,
)]
#[serde(rename_all = "snake_case")]
#[sqlx(rename_all = "snake_case")]
pub enum Priority {
    /// System health, data loss prevention (100)
    Critical = 100,
    /// Owner-requested, blocking other work (80)
    Urgent = 80,
    /// Important feature work (60)
    High = 60,
    /// Regular tasks (40)
    #[default]
    Normal = 40,
    /// Nice-to-have, improvements (20)
    Low = 20,
    /// Self-improvement, cleanup, optimization (5)
    Background = 5,
}

impl Priority {
    /// Get the numeric value of this priority.
    pub fn value(self) -> i32 {
        self as i32
    }

    /// Create a Priority from a numeric value, clamping to valid range.
    pub fn from_value(value: i32) -> Self {
        match value {
            90..=i32::MAX => Priority::Critical,
            70..=89 => Priority::Urgent,
            50..=69 => Priority::High,
            30..=49 => Priority::Normal,
            10..=29 => Priority::Low,
            _ => Priority::Background,
        }
    }
}

/// Task execution state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "task_state", rename_all = "snake_case")]
pub enum TaskState {
    /// Created but dependencies not yet resolved
    #[default]
    Pending,
    /// All dependencies met, ready to be scheduled
    Ready,
    /// In the priority queue, waiting for a worker
    Queued,
    /// Assigned to a worker, not yet started
    Assigned,
    /// Worker is actively executing this task
    Running,
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
    /// Task exceeded its execution deadline
    TimedOut,
}

impl TaskState {
    /// Check if this is a terminal state (cannot transition further).
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            TaskState::Completed | TaskState::Failed | TaskState::Cancelled | TaskState::TimedOut
        )
    }

    /// Check if this state allows transitioning to a new state.
    pub fn can_transition_to(self, new_state: TaskState) -> bool {
        use TaskState::*;

        match (self, new_state) {
            // Terminal states cannot transition
            (s, _) if s.is_terminal() => false,

            // Pending can go to Ready (deps resolved), Blocked (dep failed), or Paused
            (Pending, Ready) | (Pending, Blocked) | (Pending, Paused) => true,

            // Ready can go to Queued, Testing, Paused, Failed, or back to Pending
            (Ready, Queued)
            | (Ready, Testing)
            | (Ready, Paused)
            | (Ready, Failed)
            | (Ready, Pending) => true,

            // Queued can go to Assigned, Paused, or back to Ready
            (Queued, Assigned) | (Queued, Paused) | (Queued, Ready) => true,

            // Assigned can go to Running, Paused, or back to Queued
            (Assigned, Running) | (Assigned, Paused) | (Assigned, Queued) => true,

            // Running can go to Testing or a terminal execution state.
            (Running, Testing)
            | (Running, Ready)
            | (Running, Failed)
            | (Running, TimedOut)
            | (Running, Paused) => true,

            // Testing can go to Completed, Running (retry), or Failed
            (Testing, Completed) | (Testing, Running) | (Testing, Ready) | (Testing, Failed) => {
                true
            }

            // Paused can resume to previous state (simplified: to Ready)
            (Paused, Ready) | (Paused, Pending) => true,

            // Blocked can go to Failed or be unblocked (to Pending)
            (Blocked, Failed) | (Blocked, Pending) => true,

            // Any other transition is invalid
            _ => false,
        }
    }
}

impl fmt::Display for TaskState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Pending => "pending",
            Self::Ready => "ready",
            Self::Queued => "queued",
            Self::Assigned => "assigned",
            Self::Running => "running",
            Self::Testing => "testing",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Paused => "paused",
            Self::Blocked => "blocked",
            Self::TimedOut => "timed_out",
        };
        f.write_str(value)
    }
}

/// Task error types.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(rename_all = "snake_case")]
pub enum TaskErrorType {
    CompilationFailed,
    TestsFailed,
    ReviewRejected,
    WorkerCrashed,
    Timeout,
    DependencyFailed,
    ResourceExhausted,
    SecurityViolation,
    /// V0.38 Phase 4: the quality gate passed but the Git checkpoint or its
    /// Artifact record could not be durably created (git commit failed, no
    /// actual changes existed to checkpoint, or artifact persistence
    /// failed after a successful commit). Distinct from `TestsFailed`
    /// because the Agent's work was verified — only turning it into a
    /// durable record failed.
    CheckpointFailed,
    Unknown,
}

/// Error information for a failed task.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskError {
    pub error_type: TaskErrorType,
    pub message: String,
    pub stack_trace: Option<String>,
    pub attempt: u32,
    pub occurred_at: DateTime<Utc>,
}

impl TaskError {
    /// Create a new task error.
    pub fn new(error_type: TaskErrorType, message: impl Into<String>, attempt: u32) -> Self {
        Self {
            error_type,
            message: message.into(),
            stack_trace: None,
            attempt,
            occurred_at: Utc::now(),
        }
    }

    /// Add stack trace to the error.
    pub fn with_stack_trace(mut self, trace: impl Into<String>) -> Self {
        self.stack_trace = Some(trace.into());
        self
    }
}

/// Task execution output.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TaskOutput {
    /// Files modified
    pub files_changed: Vec<String>,
    /// Files created
    pub files_created: Vec<String>,
    /// Files deleted
    pub files_deleted: Vec<String>,
    /// Test results summary
    pub test_summary: Option<String>,
    /// Code review result
    pub review_result: Option<String>,
    /// Git commit SHA if committed
    pub commit_sha: Option<String>,
    /// LLM tokens used
    pub tokens_input: u32,
    pub tokens_output: u32,
    /// Worker's final message/summary
    pub summary: String,
    /// Additional metadata
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

/// Core task data structure.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Task {
    // === Identity ===
    pub id: Uuid,
    pub project_id: Uuid,
    pub milestone_id: Option<Uuid>,

    // === Content ===
    pub title: String,
    pub description: String,
    #[serde(default = "default_task_type")]
    pub task_type: String,
    pub acceptance_criteria: Vec<String>,
    pub test_commands: Vec<String>,

    // === Scheduling ===
    pub state: TaskState,
    pub priority: Priority,
    pub effective_priority: f64,

    // === Dependencies ===
    pub depends_on: Vec<Uuid>,
    #[serde(default)]
    pub blocked_by: Vec<Uuid>,
    #[serde(default)]
    pub blocks: Vec<Uuid>,

    // === Assignment ===
    pub assigned_worker_id: Option<Uuid>,
    pub worker_type_hint: Option<WorkerType>,
    /// Stable logical execution identifier reused after crash recovery.
    #[serde(default)]
    pub execution_id: Option<Uuid>,
    /// Number of times an interrupted execution has been resumed.
    #[serde(default)]
    pub resume_count: u32,

    // === Deferred scheduling (V0.38.1) ===
    /// When this `Ready` task next becomes eligible to rejoin the live
    /// queue. Set by an admission-gate block (budget/accounting/workspace)
    /// or a failure retry backoff — never by a blocking sleep in
    /// `ExecutionLoop`. `None` means immediately eligible. Persisted (via
    /// the scheduler's existing task-context JSON) so a deferral survives a
    /// daemon restart instead of becoming immediately schedulable again.
    #[serde(default)]
    pub retry_at: Option<DateTime<Utc>>,
    /// How many times this task has been deferred. Used only to size
    /// backoff delays (bounded exponential); it never gates `max_attempts`.
    #[serde(default)]
    pub defer_count: u32,

    // === Execution ===
    #[serde(default = "default_attempt")]
    pub attempt: u32,
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,
    #[serde(with = "serde_duration", default = "default_timeout")]
    pub timeout: Duration,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,

    // === Results ===
    pub output: Option<TaskOutput>,
    pub error: Option<TaskError>,

    // === Metadata ===
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub estimated_minutes: Option<u32>,
    pub actual_minutes: Option<u32>,
    #[serde(default)]
    pub tags: Vec<String>,
}

fn default_attempt() -> u32 {
    1
}

fn default_task_type() -> String {
    "generic".to_string()
}
fn default_max_attempts() -> u32 {
    3
}
fn default_timeout() -> Duration {
    Duration::from_secs(30 * 60)
}

/// Serde helpers for Duration
mod serde_duration {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u64(duration.as_secs())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let secs = u64::deserialize(deserializer)?;
        Ok(Duration::from_secs(secs))
    }
}

impl Task {
    /// Create a new task with default values.
    pub fn new(project_id: Uuid, title: impl Into<String>, description: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            project_id,
            milestone_id: None,
            title: title.into(),
            description: description.into(),
            task_type: default_task_type(),
            acceptance_criteria: Vec::new(),
            test_commands: Vec::new(),
            state: TaskState::Pending,
            priority: Priority::Normal,
            effective_priority: 40.0,
            depends_on: Vec::new(),
            blocked_by: Vec::new(),
            blocks: Vec::new(),
            assigned_worker_id: None,
            worker_type_hint: None,
            execution_id: None,
            resume_count: 0,
            retry_at: None,
            defer_count: 0,
            attempt: 1,
            max_attempts: 3,
            timeout: Duration::from_secs(30 * 60),
            started_at: None,
            completed_at: None,
            output: None,
            error: None,
            created_at: now,
            updated_at: now,
            estimated_minutes: None,
            actual_minutes: None,
            tags: Vec::new(),
        }
    }

    /// Set the priority and update effective priority.
    pub fn with_priority(mut self, priority: Priority) -> Self {
        self.priority = priority;
        self.effective_priority = priority.value() as f64;
        self
    }

    /// Add dependencies to the task.
    pub fn with_dependencies(mut self, deps: Vec<Uuid>) -> Self {
        self.depends_on = deps;
        self
    }

    /// Set the worker type hint.
    pub fn with_worker_type(mut self, worker_type: WorkerType) -> Self {
        self.worker_type_hint = Some(worker_type);
        self
    }

    /// Check if this task can be retried.
    pub fn can_retry(&self) -> bool {
        self.attempt < self.max_attempts
    }

    /// Get elapsed time since task was started.
    pub fn elapsed(&self) -> Option<Duration> {
        self.started_at.map(|start| {
            let now = Utc::now();
            let elapsed = now.signed_duration_since(start);
            Duration::from_secs(elapsed.num_seconds().max(0) as u64)
        })
    }

    /// Check if the task has timed out.
    pub fn is_timed_out(&self) -> bool {
        match self.elapsed() {
            Some(elapsed) => elapsed > self.timeout,
            None => false,
        }
    }

    /// Calculate progress percentage (placeholder for now).
    pub fn progress_pct(&self) -> f32 {
        match self.state {
            TaskState::Completed => 100.0,
            TaskState::Failed | TaskState::Cancelled => 0.0,
            TaskState::Running => 50.0, // Placeholder
            TaskState::Testing => 90.0,
            _ => 0.0,
        }
    }

    /// Compute effective priority with boosts.
    pub fn compute_effective_priority(
        &self,
        age_boost: f64,
        dependency_boost: f64,
        retry_penalty: f64,
    ) -> f64 {
        let base = self.priority.value() as f64;
        let age = (Utc::now() - self.created_at).num_minutes() as f64;
        let age_bonus = (age * age_boost).min(20.0);
        let dep_bonus = (self.blocks.len() as f64 * dependency_boost).min(30.0);
        let retry_pen = (self.attempt as f64 - 1.0) * retry_penalty;

        base + age_bonus + dep_bonus - retry_pen
    }
}

/// Task builder for convenient construction.
pub struct TaskBuilder {
    task: Task,
}

impl TaskBuilder {
    /// Create a new task builder.
    pub fn new(project_id: Uuid, title: impl Into<String>) -> Self {
        let title_str = title.into();
        Self {
            task: Task::new(project_id, title_str.clone(), title_str),
        }
    }

    /// Set description.
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.task.description = desc.into();
        self
    }

    /// Set priority.
    pub fn priority(mut self, priority: Priority) -> Self {
        self.task.priority = priority;
        self.task.effective_priority = priority.value() as f64;
        self
    }

    /// Add dependency.
    pub fn depends_on(mut self, task_id: Uuid) -> Self {
        self.task.depends_on.push(task_id);
        self
    }

    /// Set worker type.
    pub fn worker_type(mut self, worker_type: WorkerType) -> Self {
        self.task.worker_type_hint = Some(worker_type);
        self
    }

    /// Set estimated duration.
    pub fn estimated_minutes(mut self, minutes: u32) -> Self {
        self.task.estimated_minutes = Some(minutes);
        self
    }

    /// Set timeout.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.task.timeout = timeout;
        self
    }

    /// Add test command.
    pub fn test_command(mut self, cmd: impl Into<String>) -> Self {
        self.task.test_commands.push(cmd.into());
        self
    }

    /// Add acceptance criterion.
    pub fn acceptance(mut self, criterion: impl Into<String>) -> Self {
        self.task.acceptance_criteria.push(criterion.into());
        self
    }

    /// Set milestone.
    pub fn milestone(mut self, milestone_id: Uuid) -> Self {
        self.task.milestone_id = Some(milestone_id);
        self
    }

    /// Build the task.
    pub fn build(self) -> Task {
        self.task
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_priority_value() {
        assert_eq!(Priority::Critical.value(), 100);
        assert_eq!(Priority::Normal.value(), 40);
        assert_eq!(Priority::Background.value(), 5);
    }

    #[test]
    fn test_priority_from_value() {
        assert!(matches!(Priority::from_value(100), Priority::Critical));
        assert!(matches!(Priority::from_value(50), Priority::High));
        assert!(matches!(Priority::from_value(40), Priority::Normal));
        assert!(matches!(Priority::from_value(5), Priority::Background));
        assert!(matches!(Priority::from_value(0), Priority::Background));
    }

    #[test]
    fn test_priority_ordering() {
        assert!(Priority::Critical > Priority::Urgent);
        assert!(Priority::Urgent > Priority::High);
        assert!(Priority::High > Priority::Normal);
        assert!(Priority::Normal > Priority::Low);
        assert!(Priority::Low > Priority::Background);
    }

    #[test]
    fn test_task_state_terminal() {
        assert!(TaskState::Completed.is_terminal());
        assert!(TaskState::Failed.is_terminal());
        assert!(TaskState::Cancelled.is_terminal());
        assert!(!TaskState::Running.is_terminal());
        assert!(TaskState::TimedOut.is_terminal());
        assert!(!TaskState::Pending.is_terminal());
    }

    #[test]
    fn test_task_state_transitions() {
        // Valid transitions
        assert!(TaskState::Pending.can_transition_to(TaskState::Ready));
        assert!(TaskState::Ready.can_transition_to(TaskState::Queued));
        assert!(TaskState::Queued.can_transition_to(TaskState::Assigned));
        assert!(TaskState::Assigned.can_transition_to(TaskState::Running));
        assert!(TaskState::Running.can_transition_to(TaskState::Testing));
        assert!(TaskState::Testing.can_transition_to(TaskState::Completed));

        // Invalid transitions
        assert!(!TaskState::Completed.can_transition_to(TaskState::Running));
        assert!(!TaskState::Pending.can_transition_to(TaskState::Completed));
        assert!(!TaskState::Failed.can_transition_to(TaskState::Ready));
    }

    #[test]
    fn test_task_creation() {
        let project_id = Uuid::new_v4();
        let task = Task::new(project_id, "Test Task", "A test task");

        assert_eq!(task.title, "Test Task");
        assert_eq!(task.state, TaskState::Pending);
        assert_eq!(task.priority, Priority::Normal);
        assert_eq!(task.attempt, 1);
        assert_eq!(task.max_attempts, 3);
    }

    #[test]
    fn test_task_with_priority() {
        let project_id = Uuid::new_v4();
        let task =
            Task::new(project_id, "Important", "High priority task").with_priority(Priority::High);

        assert_eq!(task.priority, Priority::High);
        assert_eq!(task.effective_priority, 60.0);
    }

    #[test]
    fn test_task_can_retry() {
        let project_id = Uuid::new_v4();
        let mut task = Task::new(project_id, "Test", "Test");

        assert!(task.can_retry()); // 1/3

        task.attempt = 3;
        assert!(!task.can_retry()); // 3/3
    }

    #[test]
    fn test_task_is_timed_out() {
        let project_id = Uuid::new_v4();
        let mut task = Task::new(project_id, "Test", "Test");
        task.timeout = Duration::from_secs(1);

        // Not started
        assert!(!task.is_timed_out());

        // Started 2 seconds ago
        task.started_at = Some(Utc::now() - chrono::Duration::seconds(2));
        assert!(task.is_timed_out());

        // Started recently
        task.started_at = Some(Utc::now());
        assert!(!task.is_timed_out());
    }

    #[test]
    fn test_task_builder() {
        let project_id = Uuid::new_v4();
        let dep_id = Uuid::new_v4();

        let task = TaskBuilder::new(project_id, "Built Task")
            .description("A built task")
            .priority(Priority::High)
            .depends_on(dep_id)
            .estimated_minutes(30)
            .test_command("cargo test")
            .acceptance("Tests pass")
            .build();

        assert_eq!(task.title, "Built Task");
        assert_eq!(task.description, "A built task");
        assert_eq!(task.priority, Priority::High);
        assert_eq!(task.depends_on, vec![dep_id]);
        assert_eq!(task.estimated_minutes, Some(30));
        assert_eq!(task.test_commands, vec!["cargo test"]);
        assert_eq!(task.acceptance_criteria, vec!["Tests pass"]);
    }

    #[test]
    fn test_task_error_creation() {
        let error = TaskError::new(TaskErrorType::TestsFailed, "Unit tests failed", 1);

        assert!(matches!(error.error_type, TaskErrorType::TestsFailed));
        assert_eq!(error.message, "Unit tests failed");
        assert_eq!(error.attempt, 1);
        assert!(error.stack_trace.is_none());
    }

    #[test]
    fn test_compute_effective_priority() {
        let project_id = Uuid::new_v4();
        let mut task = Task::new(project_id, "Test", "Test");
        task.priority = Priority::Normal; // 40
        task.blocks = vec![Uuid::new_v4(), Uuid::new_v4()];

        let priority = task.compute_effective_priority(1.0, 5.0, 5.0);
        assert!(priority > 40.0); // Should be boosted by age and dependencies
    }
}
