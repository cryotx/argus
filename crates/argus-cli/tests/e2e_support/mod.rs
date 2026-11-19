//! Shared harness for the Postgres-backed E2E test binaries
//! (`agent_pipeline_e2e.rs`, `execution_loop_e2e.rs`).
//!
//! V0.38.1: these tests exercise the *current* production API — real
//! `TaskScheduler` + real `PostgresWorkspaceRepository`/`WorkspaceManager`/
//! `GitWorkspaceGuard` + real `BudgetService` + `ExecutionLoop` — the same
//! chain `argus run` wires up, just with a stub agent binary standing in for
//! the real `claude` CLI so most of these cost nothing to run repeatedly.
//! Nothing here bypasses Workspace isolation: the executor always runs in a
//! real, verified Git worktree.
//!
//! This module is compiled independently into each of the two E2E test
//! binaries above via `mod e2e_support;` (there is no shared library between
//! integration-test binaries in Cargo). Each binary only uses a subset of
//! these helpers, so per-binary dead-code analysis would otherwise flag
//! whatever the *other* binary alone uses — hence the blanket allow below,
//! not a sign anything here is actually unused.
#![allow(dead_code)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

use argus_core::accounting_state::RuntimeAccountingState;
use argus_core::artifact_repository::{ArtifactRepository, PostgresArtifactRepository};
use argus_core::budget_service::{BudgetPolicy, BudgetService};
use argus_core::checkpoint::GitCheckpointer;
use argus_core::execution_loop::{ExecutionLoop, ExecutionLoopConfig, TaskExecutor};
use argus_core::spend_repository::{PostgresSpendRepository, SpendRepository};
use argus_core::task_repository::PostgresTaskRepository;
use argus_core::task_scheduler::TaskScheduler;
use argus_core::task_types::TaskState;
use argus_core::workspace_manager::{WorkspaceConfig, WorkspaceManager};
use argus_core::workspace_repository::{PostgresWorkspaceRepository, WorkspaceRepository};
use argus_git::checkpoint::GitCommitCheckpointer;
use argus_git::worktree_guard::GitWorkspaceGuard;

pub const MASTER_KEY: &str = "0123456789abcdef0123456789abcdef";

pub fn database_url() -> String {
    std::env::var("ARGUS_DATABASE_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .expect("set ARGUS_DATABASE_URL or DATABASE_URL for this e2e test")
}

pub async fn connect() -> PgPool {
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&database_url())
        .await
        .expect("connect postgres");
    sqlx::migrate!("../../migrations")
        .run(&pool)
        .await
        .expect("run migrations");
    pool
}

pub async fn insert_project(pool: &PgPool, label: &str) -> Uuid {
    let project_id = Uuid::new_v4();
    let name = format!("{label}-{}", &project_id.to_string()[..8]);
    sqlx::query(
        r#"
        INSERT INTO projects (id, name, description, git_local_path, active, priority)
        VALUES ($1, $2, 'e2e', $3, true, 'medium')
        "#,
    )
    .bind(project_id)
    .bind(&name)
    .bind(format!("/tmp/argus/projects/{name}"))
    .execute(pool)
    .await
    .expect("insert project");
    project_id
}

/// A scratch directory that cleans itself up.
pub struct Scratch(PathBuf);

impl Scratch {
    pub fn new(label: &str) -> Self {
        let p = std::env::temp_dir().join(format!("argus-e2e-{label}-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&p).expect("create scratch");
        Self(p)
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Write an executable stub agent script. Runs with the task's worktree as
/// its cwd and the built prompt as `$1`, exactly like a real agent CLI.
pub fn write_stub_agent(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\nif [ \"${{1:-}}\" = --version ]; then\n  printf 'argus-stub 0.0.0\\n'\n  exit 0\nfi\n{body}\n")).expect("write stub agent");
    let mut perms = std::fs::metadata(&path).expect("stat stub").permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).expect("chmod stub");
    path
}

pub fn fast_loop() -> ExecutionLoopConfig {
    ExecutionLoopConfig {
        idle_interval: Duration::from_millis(25),
        maintenance_interval: Duration::from_millis(200),
    }
}

pub async fn await_terminal(
    scheduler: &TaskScheduler,
    task_id: Uuid,
    timeout: Duration,
) -> TaskState {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut last = TaskState::Pending;
    while tokio::time::Instant::now() < deadline {
        if let Some(t) = scheduler.get_task(task_id).await {
            last = t.state;
            if t.state.is_terminal() {
                return t.state;
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    last
}

pub async fn await_state(
    scheduler: &TaskScheduler,
    task_id: Uuid,
    want: TaskState,
    timeout: Duration,
) -> TaskState {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut last = TaskState::Pending;
    while tokio::time::Instant::now() < deadline {
        if let Some(t) = scheduler.get_task(task_id).await {
            last = t.state;
            if t.state == want {
                return t.state;
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    last
}

pub fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let p = l.local_addr().expect("addr").port();
    drop(l);
    p
}

/// Real production stack (Postgres-backed Workspace + Budget + Scheduler),
/// minus only the external agent binary — a real vertical vs. the daemon,
/// not a simulation of it.
pub struct Harness {
    pub pool: PgPool,
    pub scratch: Scratch,
    pub canonical_root: PathBuf,
    pub scheduler: Arc<TaskScheduler>,
    pub workspace_repo: Arc<dyn WorkspaceRepository>,
    pub workspace_manager: Arc<WorkspaceManager>,
    pub workspace_guard: Arc<GitWorkspaceGuard>,
    pub budget_service: Arc<BudgetService>,
    pub spend_repo: Arc<dyn SpendRepository>,
    pub accounting_state: RuntimeAccountingState,
    pub checkpointer: Arc<dyn GitCheckpointer>,
    pub artifact_repo: Arc<dyn ArtifactRepository>,
}

impl Harness {
    pub async fn new(label: &str) -> Self {
        let pool = connect().await;
        let scratch = Scratch::new(label);
        let canonical_root = scratch.path().join("canonical");
        let worktree_root = scratch.path().join("worktrees");
        std::fs::create_dir_all(&canonical_root).unwrap();
        std::fs::create_dir_all(&worktree_root).unwrap();

        let scheduler = Arc::new(TaskScheduler::new_with_repository(
            Arc::new(PostgresTaskRepository::new(pool.clone())),
            None,
        ));
        let workspace_repo: Arc<dyn WorkspaceRepository> =
            Arc::new(PostgresWorkspaceRepository::new(pool.clone()));
        let workspace_manager =
            Arc::new(WorkspaceManager::new(WorkspaceConfig::new(&worktree_root)));
        let workspace_guard = Arc::new(GitWorkspaceGuard::new());

        // Generous budget: these tests verify pipeline plumbing, not budget
        // enforcement (that's covered separately), so a huge daily limit
        // keeps every preflight check an unconditional Allow.
        let spend_repo: Arc<dyn SpendRepository> =
            Arc::new(PostgresSpendRepository::new(pool.clone()));
        let accounting_state = RuntimeAccountingState::new();
        let budget_service = Arc::new(
            BudgetService::new(
                spend_repo.clone(),
                BudgetPolicy {
                    daily_budget_usd: Some(10_000.0),
                    max_per_task_usd: Some(1_000.0),
                    strict_mode: true,
                },
            )
            .with_accounting_state(accounting_state.clone()),
        );

        let checkpointer: Arc<dyn GitCheckpointer> = Arc::new(GitCommitCheckpointer::new());
        let artifact_repo: Arc<dyn ArtifactRepository> =
            Arc::new(PostgresArtifactRepository::new(pool.clone()));

        Self {
            pool,
            scratch,
            canonical_root,
            scheduler,
            workspace_repo,
            workspace_manager,
            workspace_guard,
            budget_service,
            spend_repo,
            accounting_state,
            checkpointer,
            artifact_repo,
        }
    }

    pub fn scratch_path(&self) -> &Path {
        self.scratch.path()
    }

    /// Build an `ExecutionLoop` over this harness's real Workspace, Budget,
    /// Spend/Accounting, and Checkpoint stack with the given executor — the
    /// same shape `argus run` builds, minus only the daemon's HTTP layer.
    /// V0.38 Phase 4: a quality-passing task only completes once its
    /// worktree changes are a real Git commit with a persisted Artifact, and
    /// its spend is recorded through the same loop the daemon uses, exactly
    /// like production.
    pub fn build_loop(&self, executor: Arc<dyn TaskExecutor>) -> ExecutionLoop {
        ExecutionLoop::new(Arc::clone(&self.scheduler), executor, fast_loop())
            .with_budget_service(self.budget_service.clone())
            .with_spend_repo(self.spend_repo.clone())
            .with_accounting_state(self.accounting_state.clone())
            .with_workspace(
                self.workspace_repo.clone(),
                self.workspace_manager.clone(),
                self.workspace_guard.clone(),
                self.canonical_root.clone(),
            )
            .with_checkpoint(self.checkpointer.clone(), self.artifact_repo.clone())
    }

    /// The real Git worktree a completed/running task actually executed in.
    pub async fn worktree_path_for(&self, project_id: Uuid, task_id: Uuid) -> PathBuf {
        let execution_id = self
            .scheduler
            .get_task(task_id)
            .await
            .and_then(|t| t.execution_id)
            .expect("task must have an execution_id by now");
        self.workspace_manager
            .worktree_path(project_id, execution_id)
    }

    /// The (at most one) Git-checkpoint artifact a task has produced, if any.
    pub async fn artifact_for_task(
        &self,
        task_id: Uuid,
    ) -> Option<argus_core::artifact_repository::ArtifactRecord> {
        self.artifact_repo
            .get_by_task_id(task_id)
            .await
            .expect("query artifacts")
            .into_iter()
            .next()
    }

    pub async fn persisted_state(&self, task_id: Uuid) -> String {
        let (state,): (String,) = sqlx::query_as("SELECT state::text FROM tasks WHERE id = $1")
            .bind(task_id)
            .fetch_one(&self.pool)
            .await
            .expect("read persisted task state");
        state
    }
}
