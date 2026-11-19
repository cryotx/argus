//! V0.38 Phase 3 — ExecutionLoop workspace-isolation E2E tests.
//!
//! Covers spec items #18-#25: Agent cwd, canonical-repo isolation,
//! `.gitignore`-respecting `files_changed`, missing-worktree /
//! ownership-violation / branch-mismatch fail-closed behavior, budget-block
//! workspace reuse, and failed-Agent workspace retention.
//!
//! No Postgres required: [`InMemoryWorkspaceRepository`] stands in for
//! `PostgresWorkspaceRepository`, and real `git` (via `argus-git`'s
//! `GitWorkspaceGuard`) provides the actual worktree mechanics — these tests
//! exercise real worktrees on disk, not a simulation of them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use uuid::Uuid;

use argus_core::execution_loop::{
    ExecutionLoop, ExecutionLoopConfig, ExecutionRequest, TaskExecutionResult, TaskExecutor,
};
use argus_core::task_scheduler::TaskScheduler;
use argus_core::task_types::{Task, TaskBuilder, TaskState};
use argus_core::types::ArgusResult;
use argus_core::workspace_manager::{WorkspaceConfig, WorkspaceGuard, WorkspaceManager};
use argus_core::workspace_repository::{WorkspaceRecord, WorkspaceRepository};
use argus_git::worktree_guard::GitWorkspaceGuard;

// ============================================================
// Test doubles
// ============================================================

/// In-memory stand-in for `PostgresWorkspaceRepository` — same idempotency
/// contract (unique per `execution_id`), no database required.
#[derive(Default)]
struct InMemoryWorkspaceRepository {
    records: Mutex<Vec<WorkspaceRecord>>,
}

#[async_trait]
impl WorkspaceRepository for InMemoryWorkspaceRepository {
    async fn ensure_workspace(
        &self,
        execution_id: Uuid,
        project_id: Uuid,
        task_id: Uuid,
        repository_path: String,
        worktree_path: String,
        branch_name: String,
        base_revision: String,
    ) -> ArgusResult<WorkspaceRecord> {
        let mut records = self.records.lock().unwrap();
        if let Some(existing) = records.iter().find(|r| r.execution_id == execution_id) {
            return Ok(existing.clone());
        }
        let record = WorkspaceRecord {
            workspace_id: Uuid::new_v4(),
            project_id,
            task_id,
            execution_id,
            repository_path,
            worktree_path,
            branch_name,
            base_revision,
            status: "preparing".to_string(),
            failure_reason: None,
            created_at: Utc::now(),
            activated_at: None,
            completed_at: None,
        };
        records.push(record.clone());
        Ok(record)
    }

    async fn get_workspace(&self, workspace_id: Uuid) -> ArgusResult<Option<WorkspaceRecord>> {
        Ok(self
            .records
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.workspace_id == workspace_id)
            .cloned())
    }

    async fn get_by_execution_id(
        &self,
        execution_id: Uuid,
    ) -> ArgusResult<Option<WorkspaceRecord>> {
        Ok(self
            .records
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.execution_id == execution_id)
            .cloned())
    }

    async fn update_status(
        &self,
        workspace_id: Uuid,
        status: &str,
        failure_reason: Option<&str>,
    ) -> ArgusResult<()> {
        let mut records = self.records.lock().unwrap();
        if let Some(r) = records.iter_mut().find(|r| r.workspace_id == workspace_id) {
            r.status = status.to_string();
            r.failure_reason = failure_reason.map(str::to_string);
        }
        Ok(())
    }

    async fn mark_active(&self, workspace_id: Uuid) -> ArgusResult<()> {
        let mut records = self.records.lock().unwrap();
        if let Some(r) = records.iter_mut().find(|r| r.workspace_id == workspace_id) {
            r.status = "active".to_string();
            r.activated_at = Some(Utc::now());
        }
        Ok(())
    }

    async fn mark_completed(&self, workspace_id: Uuid) -> ArgusResult<()> {
        self.update_status(workspace_id, "completed", None).await
    }

    async fn mark_failed(&self, workspace_id: Uuid, reason: &str) -> ArgusResult<()> {
        self.update_status(workspace_id, "failed", Some(reason))
            .await
    }

    async fn list_recoverable(&self) -> ArgusResult<Vec<WorkspaceRecord>> {
        Ok(self
            .records
            .lock()
            .unwrap()
            .iter()
            .filter(|r| {
                matches!(
                    r.status.as_str(),
                    "preparing" | "active" | "recovery_needed"
                )
            })
            .cloned()
            .collect())
    }

    async fn get_active_for_task(&self, task_id: Uuid) -> ArgusResult<Option<WorkspaceRecord>> {
        Ok(self
            .records
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.task_id == task_id && r.status == "active")
            .cloned())
    }
}

impl InMemoryWorkspaceRepository {
    fn snapshot(&self) -> Vec<WorkspaceRecord> {
        self.records.lock().unwrap().clone()
    }
}

/// Counts invocations and hands back whatever `TaskExecutionResult` a test
/// closure builds — the one executor shape every scenario below needs.
struct FnExecutor<F>
where
    F: Fn(&ExecutionRequest) -> TaskExecutionResult + Send + Sync,
{
    calls: Arc<AtomicUsize>,
    f: F,
}

#[async_trait]
impl<F> TaskExecutor for FnExecutor<F>
where
    F: Fn(&ExecutionRequest) -> TaskExecutionResult + Send + Sync,
{
    fn name(&self) -> &str {
        "fn-stub-executor"
    }

    async fn execute(&self, request: &ExecutionRequest) -> TaskExecutionResult {
        self.calls.fetch_add(1, Ordering::SeqCst);
        (self.f)(request)
    }
}

fn success_result(task: &Task, files_changed: Vec<String>) -> TaskExecutionResult {
    TaskExecutionResult {
        task_id: task.id,
        success: true,
        exit_code: Some(0),
        stdout_summary: "ok".to_string(),
        stderr_summary: String::new(),
        started_at: Utc::now(),
        finished_at: Utc::now(),
        failure_reason: None,
        files_changed,
        workspace: None,
        metadata: HashMap::new(),
        workspace_id: None,
        branch_name: None,
        base_revision: None,
        worktree_path: None,
    }
}

fn failure_result(task: &Task, reason: &str) -> TaskExecutionResult {
    TaskExecutionResult {
        task_id: task.id,
        success: false,
        exit_code: Some(1),
        stdout_summary: String::new(),
        stderr_summary: reason.to_string(),
        started_at: Utc::now(),
        finished_at: Utc::now(),
        failure_reason: Some(reason.to_string()),
        files_changed: Vec::new(),
        workspace: None,
        metadata: HashMap::new(),
        workspace_id: None,
        branch_name: None,
        base_revision: None,
        worktree_path: None,
    }
}

// ============================================================
// Git helpers
// ============================================================

fn git(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("failed to run git {args:?} in {}: {e}", dir.display()))
}

fn init_canonical_repo(path: &Path) {
    std::fs::create_dir_all(path).unwrap();
    assert!(git(path, &["init", "-q"]).status.success());
    git(path, &["config", "user.name", "Test"]);
    git(path, &["config", "user.email", "test@example.com"]);
}

fn commit_all(path: &Path, message: &str) {
    assert!(git(path, &["add", "-A"]).status.success());
    // `--allow-empty`: some callers just need a resolvable HEAD and have
    // nothing staged yet (e.g. a bare "seed" commit before creating a
    // worktree) — that must not fail the way a normal empty commit would.
    let out = git(path, &["commit", "-q", "--allow-empty", "-m", message]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn head_of(path: &Path) -> String {
    let out = git(path, &["rev-parse", "HEAD"]);
    assert!(out.status.success());
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn status_of(path: &Path) -> String {
    let out = git(path, &["status", "--porcelain"]);
    String::from_utf8_lossy(&out.stdout).to_string()
}

async fn wait_for_state(
    scheduler: &TaskScheduler,
    task_id: Uuid,
    want: TaskState,
    timeout: Duration,
) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if scheduler.get_task(task_id).await.map(|t| t.state) == Some(want) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

/// Wait until the task reaches a genuinely terminal state (Completed,
/// Failed, Cancelled, or TimedOut). Deliberately does *not* stop at `Ready`
/// — a freshly-added task starts in `Ready` before the loop ever claims it,
/// so treating `Ready` as "settled" would race the loop and return
/// immediately instead of waiting for the gate under test to actually run.
async fn wait_until_settled(scheduler: &TaskScheduler, task_id: Uuid, timeout: Duration) -> Task {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Some(t) = scheduler.get_task(task_id).await {
            if t.state.is_terminal() {
                return t;
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return scheduler.get_task(task_id).await.expect("task must exist");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Wait until the task is either terminal or deferred (`Ready` with
/// `retry_at` set) — the two rest states a task can end this test's first
/// phase in. Returns `None` if it hits neither within `timeout` (e.g. it's
/// still mid-flight, which the caller should treat as a failure to settle).
async fn wait_until_settled_or_deferred(
    scheduler: &TaskScheduler,
    task_id: Uuid,
    timeout: Duration,
) -> Option<Task> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Some(t) = scheduler.get_task(task_id).await {
            if t.state.is_terminal() || (t.state == TaskState::Ready && t.retry_at.is_some()) {
                return Some(t);
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn fast_config() -> ExecutionLoopConfig {
    ExecutionLoopConfig {
        idle_interval: Duration::from_millis(10),
        maintenance_interval: Duration::from_millis(50),
    }
}

struct Harness {
    _canonical_root: tempfile::TempDir,
    _worktree_root: tempfile::TempDir,
    canonical_root: PathBuf,
    manager: Arc<WorkspaceManager>,
    guard: Arc<dyn WorkspaceGuard>,
    repo: Arc<InMemoryWorkspaceRepository>,
}

impl Harness {
    fn new() -> Self {
        let canonical_root_dir = tempfile::tempdir().unwrap();
        let worktree_root_dir = tempfile::tempdir().unwrap();
        let manager = Arc::new(WorkspaceManager::new(WorkspaceConfig::new(
            worktree_root_dir.path(),
        )));
        Self {
            canonical_root: canonical_root_dir.path().to_path_buf(),
            _canonical_root: canonical_root_dir,
            _worktree_root: worktree_root_dir,
            manager,
            guard: Arc::new(GitWorkspaceGuard::new()),
            repo: Arc::new(InMemoryWorkspaceRepository::default()),
        }
    }

    fn canonical_repo_path(&self, project_id: Uuid) -> PathBuf {
        self.canonical_root.join(project_id.to_string())
    }

    fn build_loop<E: TaskExecutor + 'static>(
        &self,
        scheduler: &Arc<TaskScheduler>,
        executor: Arc<E>,
    ) -> ExecutionLoop {
        ExecutionLoop::new(Arc::clone(scheduler), executor, fast_config()).with_workspace(
            self.repo.clone() as Arc<dyn WorkspaceRepository>,
            self.manager.clone(),
            self.guard.clone(),
            self.canonical_root.clone(),
        )
    }
}

// ============================================================
// #18 — Agent cwd is the worktree, never the canonical repo
// ============================================================

#[tokio::test]
async fn test_e2e_agent_cwd_is_worktree_not_canonical_repo() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("debug")
        .with_test_writer()
        .try_init();
    let h = Harness::new();
    let scheduler = Arc::new(TaskScheduler::new());
    let calls = Arc::new(AtomicUsize::new(0));

    let executor = Arc::new(FnExecutor {
        calls: calls.clone(),
        f: |req: &ExecutionRequest| {
            let ws = req.workspace.as_ref().expect("workspace must be attached");
            // Literally what #18 asks for: `pwd > agent_pwd.txt` inside cwd.
            let out = Command::new("sh")
                .arg("-c")
                .arg("pwd > agent_pwd.txt")
                .current_dir(&ws.worktree_path)
                .output()
                .expect("pwd must run");
            assert!(out.status.success());
            success_result(&req.task, vec!["agent_pwd.txt".to_string()])
        },
    });

    let task = TaskBuilder::new(Uuid::new_v4(), "write pwd").build();
    let task_id = task.id;
    let project_id = task.project_id;
    scheduler.add_task(task).await;

    let handle = h.build_loop(&scheduler, executor).spawn();

    assert!(
        wait_for_state(
            &scheduler,
            task_id,
            TaskState::Completed,
            Duration::from_secs(10)
        )
        .await,
        "task should complete"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let done = scheduler.get_task(task_id).await.unwrap();
    let execution_id = done.execution_id.unwrap();
    let worktree_path = h.manager.worktree_path(project_id, execution_id);
    let canonical_path = h.canonical_repo_path(project_id);

    let recorded = std::fs::read_to_string(worktree_path.join("agent_pwd.txt"))
        .expect("agent_pwd.txt must exist in the worktree");
    let recorded_path = PathBuf::from(recorded.trim());

    assert_eq!(
        recorded_path.canonicalize().unwrap(),
        worktree_path.canonicalize().unwrap(),
        "Agent's real cwd must equal WorkspaceContext.worktree_path"
    );
    assert_ne!(
        recorded_path.canonicalize().unwrap(),
        canonical_path.canonicalize().unwrap_or(canonical_path),
        "Agent's real cwd must never equal the canonical repository path"
    );

    handle.shutdown().await;
}

// ============================================================
// #19 — canonical repository is never touched
// ============================================================

#[tokio::test]
async fn test_e2e_canonical_repository_untouched() {
    let h = Harness::new();
    let scheduler = Arc::new(TaskScheduler::new());

    let task = TaskBuilder::new(Uuid::new_v4(), "create hello.txt").build();
    let task_id = task.id;
    let project_id = task.project_id;

    // Pre-create a real canonical repo with actual history.
    let canonical = h.canonical_repo_path(project_id);
    init_canonical_repo(&canonical);
    std::fs::write(canonical.join("README.md"), "seed\n").unwrap();
    commit_all(&canonical, "seed");
    let head_before = head_of(&canonical);
    let status_before = status_of(&canonical);

    let executor = Arc::new(FnExecutor {
        calls: Arc::new(AtomicUsize::new(0)),
        f: |req: &ExecutionRequest| {
            let ws = req.workspace.as_ref().unwrap();
            std::fs::write(ws.worktree_path.join("hello.txt"), "hello\n").unwrap();
            success_result(&req.task, vec!["hello.txt".to_string()])
        },
    });

    scheduler.add_task(task).await;
    let handle = h.build_loop(&scheduler, executor).spawn();

    assert!(
        wait_for_state(
            &scheduler,
            task_id,
            TaskState::Completed,
            Duration::from_secs(10)
        )
        .await
    );

    let done = scheduler.get_task(task_id).await.unwrap();
    let execution_id = done.execution_id.unwrap();
    let worktree_path = h.manager.worktree_path(project_id, execution_id);

    // Worktree: file exists, git sees the modification.
    assert!(worktree_path.join("hello.txt").is_file());
    assert!(!status_of(&worktree_path).is_empty());

    // Canonical repository: untouched.
    assert!(
        !canonical.join("hello.txt").exists(),
        "hello.txt must not leak into the canonical repository"
    );
    assert_eq!(
        head_of(&canonical),
        head_before,
        "canonical HEAD must not move"
    );
    assert_eq!(
        status_of(&canonical),
        status_before,
        "Agent must not create new dirty state in the canonical repository"
    );

    handle.shutdown().await;
}

// ============================================================
// #20 — files_changed respects .gitignore and comes from the worktree
// ============================================================

#[tokio::test]
async fn test_e2e_files_changed_respects_gitignore() {
    let h = Harness::new();
    let scheduler = Arc::new(TaskScheduler::new());

    let task = TaskBuilder::new(Uuid::new_v4(), "create files with ignored noise").build();
    let task_id = task.id;
    let project_id = task.project_id;

    let canonical = h.canonical_repo_path(project_id);
    init_canonical_repo(&canonical);
    std::fs::write(
        canonical.join(".gitignore"),
        "__pycache__/\n.pytest_cache/\n",
    )
    .unwrap();
    commit_all(&canonical, "add gitignore");

    let executor = Arc::new(FnExecutor {
        calls: Arc::new(AtomicUsize::new(0)),
        f: |req: &ExecutionRequest| {
            let ws = req.workspace.as_ref().unwrap();
            std::fs::create_dir_all(ws.worktree_path.join("src")).unwrap();
            std::fs::write(ws.worktree_path.join("src/a.txt"), "a\n").unwrap();
            std::fs::create_dir_all(ws.worktree_path.join("__pycache__")).unwrap();
            std::fs::write(ws.worktree_path.join("__pycache__/noise.pyc"), "x").unwrap();
            std::fs::create_dir_all(ws.worktree_path.join(".pytest_cache")).unwrap();
            std::fs::write(ws.worktree_path.join(".pytest_cache/noise"), "x").unwrap();
            success_result(&req.task, vec!["should-be-overwritten".to_string()])
        },
    });

    scheduler.add_task(task).await;
    let handle = h.build_loop(&scheduler, executor).spawn();

    assert!(
        wait_for_state(
            &scheduler,
            task_id,
            TaskState::Completed,
            Duration::from_secs(10)
        )
        .await
    );

    let done = scheduler.get_task(task_id).await.unwrap();
    let files_changed = done.output.expect("output recorded").files_changed;
    assert_eq!(
        files_changed,
        vec!["src/a.txt".to_string()],
        "gitignored files must never appear in files_changed"
    );

    handle.shutdown().await;
}

// ============================================================
// #21 — missing worktree: Agent NOT SPAWNED, no canonical-repo fallback
// ============================================================

#[tokio::test]
async fn test_e2e_missing_worktree_blocks_execution() {
    let h = Harness::new();
    let scheduler = Arc::new(TaskScheduler::new());

    let mut task = TaskBuilder::new(Uuid::new_v4(), "should never run").build();
    task.max_attempts = 1; // fail closed immediately, don't retry-loop forever
    let execution_id = Uuid::new_v4();
    task.execution_id = Some(execution_id); // scheduler.claim_task keeps a preset id
    let task_id = task.id;
    let project_id = task.project_id;

    // Simulate a workspace record that already exists (from a prior,
    // successful preparation) whose worktree has since vanished from disk.
    let canonical = h.canonical_repo_path(project_id);
    init_canonical_repo(&canonical);
    commit_all(&canonical, "seed");
    let base_revision = head_of(&canonical);
    let worktree_path = h.manager.worktree_path(project_id, execution_id);
    let branch_name = h.manager.branch_name(task_id, execution_id);
    h.guard
        .ensure_worktree(&canonical, &worktree_path, &branch_name, &base_revision)
        .await
        .unwrap();
    let record = h
        .repo
        .ensure_workspace(
            execution_id,
            project_id,
            task_id,
            canonical.display().to_string(),
            worktree_path.display().to_string(),
            branch_name,
            base_revision,
        )
        .await
        .unwrap();
    h.repo.mark_active(record.workspace_id).await.unwrap();

    // Now delete the worktree out from under the record.
    std::fs::remove_dir_all(&worktree_path).unwrap();

    let calls = Arc::new(AtomicUsize::new(0));
    let executor = Arc::new(FnExecutor {
        calls: calls.clone(),
        f: |req: &ExecutionRequest| success_result(&req.task, vec![]),
    });

    scheduler.add_task(task).await;
    let handle = h.build_loop(&scheduler, executor).spawn();

    let settled = wait_until_settled(&scheduler, task_id, Duration::from_secs(10)).await;
    assert_eq!(calls.load(Ordering::SeqCst), 0, "Agent must not be spawned");
    assert_eq!(
        settled.state,
        TaskState::Failed,
        "task must fail closed rather than silently retry forever"
    );

    let record_after = h
        .repo
        .get_workspace(record.workspace_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        record_after.status, "recovery_needed",
        "workspace must move to recovery_needed, not be auto-repaired"
    );

    handle.shutdown().await;
}

// ============================================================
// #22 — ownership violation: Task A must never run against Workspace B
// ============================================================

#[tokio::test]
async fn test_e2e_ownership_violation_blocks_execution() {
    let h = Harness::new();
    let scheduler = Arc::new(TaskScheduler::new());

    // Workspace B: a real, valid workspace — but for a *different* task.
    let other_task_id = Uuid::new_v4();
    let other_project_id = Uuid::new_v4();
    let shared_execution_id = Uuid::new_v4();
    let canonical_b = h.canonical_repo_path(other_project_id);
    init_canonical_repo(&canonical_b);
    commit_all(&canonical_b, "seed");
    let base_revision = head_of(&canonical_b);
    let worktree_b = h
        .manager
        .worktree_path(other_project_id, shared_execution_id);
    let branch_b = h.manager.branch_name(other_task_id, shared_execution_id);
    h.guard
        .ensure_worktree(&canonical_b, &worktree_b, &branch_b, &base_revision)
        .await
        .unwrap();
    let record_b = h
        .repo
        .ensure_workspace(
            shared_execution_id,
            other_project_id,
            other_task_id, // <- owned by Task B, not Task A
            canonical_b.display().to_string(),
            worktree_b.display().to_string(),
            branch_b,
            base_revision,
        )
        .await
        .unwrap();
    h.repo.mark_active(record_b.workspace_id).await.unwrap();

    // Task A collides with Workspace B's execution_id.
    let mut task_a = TaskBuilder::new(Uuid::new_v4(), "task A").build();
    task_a.max_attempts = 1;
    task_a.execution_id = Some(shared_execution_id);
    let task_a_id = task_a.id;

    let calls = Arc::new(AtomicUsize::new(0));
    let executor = Arc::new(FnExecutor {
        calls: calls.clone(),
        f: |req: &ExecutionRequest| success_result(&req.task, vec![]),
    });

    scheduler.add_task(task_a).await;
    let handle = h.build_loop(&scheduler, executor).spawn();

    let settled = wait_until_settled(&scheduler, task_a_id, Duration::from_secs(10)).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "Executor invocation count must be 0 on ownership mismatch"
    );
    assert_eq!(settled.state, TaskState::Failed);
    let err = settled.error.expect("failure reason recorded");
    assert!(
        err.message.to_lowercase().contains("ownership"),
        "error should name the ownership mismatch, got: {}",
        err.message
    );

    // Workspace B itself must be left completely alone.
    let record_b_after = h
        .repo
        .get_workspace(record_b.workspace_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record_b_after.status, "active");

    handle.shutdown().await;
}

// ============================================================
// #23 — wrong branch: Agent NOT SPAWNED
// ============================================================

#[tokio::test]
async fn test_e2e_wrong_branch_blocks_execution() {
    let h = Harness::new();
    let scheduler = Arc::new(TaskScheduler::new());

    let mut task = TaskBuilder::new(Uuid::new_v4(), "wrong branch").build();
    task.max_attempts = 1;
    let execution_id = Uuid::new_v4();
    task.execution_id = Some(execution_id);
    let task_id = task.id;
    let project_id = task.project_id;

    let canonical = h.canonical_repo_path(project_id);
    init_canonical_repo(&canonical);
    commit_all(&canonical, "seed");
    let base_revision = head_of(&canonical);
    let worktree_path = h.manager.worktree_path(project_id, execution_id);
    let branch_name = h.manager.branch_name(task_id, execution_id);
    h.guard
        .ensure_worktree(&canonical, &worktree_path, &branch_name, &base_revision)
        .await
        .unwrap();
    let record = h
        .repo
        .ensure_workspace(
            execution_id,
            project_id,
            task_id,
            canonical.display().to_string(),
            worktree_path.display().to_string(),
            branch_name,
            base_revision,
        )
        .await
        .unwrap();
    h.repo.mark_active(record.workspace_id).await.unwrap();

    // Human/manual tampering: check out a different branch inside the worktree.
    assert!(
        git(&worktree_path, &["checkout", "-b", "someone-elses-branch"])
            .status
            .success()
    );

    let calls = Arc::new(AtomicUsize::new(0));
    let executor = Arc::new(FnExecutor {
        calls: calls.clone(),
        f: |req: &ExecutionRequest| success_result(&req.task, vec![]),
    });

    scheduler.add_task(task).await;
    let handle = h.build_loop(&scheduler, executor).spawn();

    let settled = wait_until_settled(&scheduler, task_id, Duration::from_secs(10)).await;
    assert_eq!(calls.load(Ordering::SeqCst), 0, "Agent must not be spawned");
    assert_eq!(settled.state, TaskState::Failed);
    let err = settled.error.expect("failure reason recorded");
    assert!(
        err.message.to_lowercase().contains("branch mismatch")
            || err.message.to_lowercase().contains("branch"),
        "error should name the branch mismatch, got: {}",
        err.message
    );

    handle.shutdown().await;
}

// ============================================================
// #24 — budget-block reuse: no second worktree for the same execution
// ============================================================

#[tokio::test]
async fn test_e2e_budget_block_reuses_same_workspace() {
    use argus_core::accounting_state::RuntimeAccountingState;
    use argus_core::budget_service::{BudgetPolicy, BudgetService};
    use argus_core::spend_repository::{SpendRecord, SpendRepository};

    struct AlwaysBlockedThenOkSpendRepo {
        blocked_calls: AtomicUsize,
    }
    #[async_trait]
    impl SpendRepository for AlwaysBlockedThenOkSpendRepo {
        async fn record_spend(&self, _record: SpendRecord) -> ArgusResult<bool> {
            Ok(true)
        }
        async fn spent_today_usd(&self, _project_id: Uuid) -> ArgusResult<Option<f64>> {
            // First preflight check sees the budget exhausted; afterwards healthy.
            let n = self.blocked_calls.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                Ok(Some(1_000.0))
            } else {
                Ok(Some(0.0))
            }
        }
        async fn spent_this_month_usd(&self, _project_id: Uuid) -> ArgusResult<Option<f64>> {
            Ok(Some(0.0))
        }
        async fn task_spend(&self, _task_id: Uuid) -> ArgusResult<Vec<SpendRecord>> {
            Ok(Vec::new())
        }
        async fn execution_spend(&self, _execution_id: Uuid) -> ArgusResult<Vec<SpendRecord>> {
            Ok(Vec::new())
        }
        async fn get_spend(&self, _spend_id: Uuid) -> ArgusResult<Option<SpendRecord>> {
            Ok(None)
        }
    }

    let _ = tracing_subscriber::fmt()
        .with_env_filter("debug")
        .with_test_writer()
        .try_init();
    let h = Harness::new();
    let scheduler = Arc::new(TaskScheduler::new());
    let spend_repo: Arc<dyn SpendRepository> = Arc::new(AlwaysBlockedThenOkSpendRepo {
        blocked_calls: AtomicUsize::new(0),
    });
    let budget_service = Arc::new(
        BudgetService::new(
            spend_repo,
            BudgetPolicy {
                daily_budget_usd: Some(1.0),
                max_per_task_usd: None,
                strict_mode: true,
            },
        )
        .with_accounting_state(RuntimeAccountingState::new()),
    );

    let task = TaskBuilder::new(Uuid::new_v4(), "budget blocked then allowed").build();
    let task_id = task.id;
    let project_id = task.project_id;

    let calls = Arc::new(AtomicUsize::new(0));
    let executor = Arc::new(FnExecutor {
        calls: calls.clone(),
        f: |req: &ExecutionRequest| success_result(&req.task, vec!["ok.txt".to_string()]),
    });

    scheduler.add_task(task).await;
    let handle = ExecutionLoop::new(Arc::clone(&scheduler), executor, fast_config())
        .with_budget_service(budget_service)
        .with_workspace(
            h.repo.clone() as Arc<dyn WorkspaceRepository>,
            h.manager.clone(),
            h.guard.clone(),
            h.canonical_root.clone(),
        )
        .spawn();

    // V0.38.1: a "daily budget exhausted" block is now deferred to the next
    // UTC midnight, not busy-retried — so the real-time wait below would
    // never complete on its own. Confirm the deferral actually happened,
    // then fast-forward it exactly the way a real clock eventually would:
    // through the same public `defer_task` API, not a backdoor.
    let deferred = wait_until_settled_or_deferred(&scheduler, task_id, Duration::from_secs(5))
        .await
        .expect("task should be deferred, not completed, immediately after the block");
    assert_eq!(deferred.state, TaskState::Ready);
    assert!(
        deferred.retry_at.is_some_and(|t| t > Utc::now()),
        "budget-blocked task must carry a future retry_at instead of busy-retrying"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "Agent must not run while blocked"
    );

    scheduler
        .defer_task(task_id, Utc::now(), "test_fast_forward_past_budget_window")
        .await
        .expect("fast-forwarding the deferral should succeed");

    assert!(
        wait_for_state(
            &scheduler,
            task_id,
            TaskState::Completed,
            Duration::from_secs(10)
        )
        .await,
        "task should eventually complete once budget clears"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "Agent runs exactly once, after the block clears"
    );

    let records = h.repo.snapshot();
    assert_eq!(
        records.len(),
        1,
        "exactly one workspace record must exist for this execution — no duplicate on requeue"
    );

    let done = scheduler.get_task(task_id).await.unwrap();
    let execution_id = done.execution_id.unwrap();
    let worktree_path = h.manager.worktree_path(project_id, execution_id);
    let worktree_count = h
        .guard
        .verify_worktree(&h.canonical_repo_path(project_id), &worktree_path)
        .await;
    assert!(
        worktree_count.is_ok(),
        "the single worktree must still verify as valid"
    );

    handle.shutdown().await;
}

// ============================================================
// #25 — failed Agent: workspace retained, partial output retained
// ============================================================

#[tokio::test]
async fn test_e2e_failed_agent_retains_workspace_and_partial_output() {
    let h = Harness::new();
    let scheduler = Arc::new(TaskScheduler::new());

    let mut task = TaskBuilder::new(Uuid::new_v4(), "fails after partial work").build();
    task.max_attempts = 1; // no retry, go straight to Failed
    let task_id = task.id;
    let project_id = task.project_id;

    let executor = Arc::new(FnExecutor {
        calls: Arc::new(AtomicUsize::new(0)),
        f: |req: &ExecutionRequest| {
            let ws = req.workspace.as_ref().unwrap();
            std::fs::write(ws.worktree_path.join("partial.txt"), "half done\n").unwrap();
            failure_result(&req.task, "agent crashed mid-task")
        },
    });

    scheduler.add_task(task).await;
    let handle = h.build_loop(&scheduler, executor).spawn();

    assert!(
        wait_for_state(
            &scheduler,
            task_id,
            TaskState::Failed,
            Duration::from_secs(10)
        )
        .await,
        "task must reach Failed"
    );

    let done = scheduler.get_task(task_id).await.unwrap();
    let execution_id = done.execution_id.unwrap();
    let worktree_path = h.manager.worktree_path(project_id, execution_id);
    let canonical = h.canonical_repo_path(project_id);

    assert!(
        worktree_path.join("partial.txt").is_file(),
        "partial.txt must still exist — Phase 3 never cleans up a failed workspace"
    );
    assert!(
        !canonical.join("partial.txt").exists(),
        "canonical repository must not be polluted even on Agent failure"
    );

    let record = h
        .repo
        .get_by_execution_id(execution_id)
        .await
        .unwrap()
        .expect("workspace record must still exist");
    assert_eq!(record.status, "failed");

    handle.shutdown().await;
}
