//! Vertical Slice 1 end-to-end tests for the ExecutionLoop.
//!
//! These prove the property the whole slice exists for: a task that is
//! merely *created* reaches a terminal state on its own, with no manual
//! `dequeue_task` call anywhere in the test — now through the current
//! production chain: Scheduler -> Workspace -> Budget -> Executor -> Result.
//!
//! V0.38.1: updated to the current API. `ShellTaskExecutor` (which ran a
//! task's own shell `command`) no longer exists — tasks are natural-language
//! work items for an Agent, not shell commands — so tests 1-4 use a small
//! in-process stub `TaskExecutor` to exercise the scheduler/workspace/loop
//! state machine deterministically and for free. Test 5 drives the real
//! `argus run` daemon over HTTP with a stub agent binary standing in for
//! `claude` (via `ARGUS_AGENT_BINARY_OVERRIDE`), so it still proves the real
//! production wiring, not a simulation of it.
//!
//! All tests here need a real PostgreSQL with the migrations applied, so
//! they follow the existing repository convention of `#[ignore]`. Run them
//! with:
//!
//! ```bash
//! ARGUS_DATABASE_URL=postgres://argus:argus@localhost:5432/argus \
//!   cargo test -p argus-cli --test execution_loop_e2e -- --ignored --test-threads=1
//! ```

mod e2e_support;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use chrono::Utc;
use uuid::Uuid;

use argus_core::execution_loop::{ExecutionRequest, TaskExecutionResult, TaskExecutor};
use argus_core::task_types::{Task, TaskBuilder, TaskState};
use argus_core::workspace_manager::WorkspaceContext;

use e2e_support::{Harness, MASTER_KEY, await_state, free_port, insert_project, write_stub_agent};

/// A small in-process stub: real state machine, real Workspace, real Budget
/// — just no external subprocess, so tests 1-4 stay fast and free.
struct FnExecutor<F>
where
    F: Fn(&ExecutionRequest) -> TaskExecutionResult + Send + Sync,
{
    calls: Arc<AtomicUsize>,
    f: F,
}

#[async_trait::async_trait]
impl<F> TaskExecutor for FnExecutor<F>
where
    F: Fn(&ExecutionRequest) -> TaskExecutionResult + Send + Sync,
{
    fn name(&self) -> &str {
        "stub:vertical-slice-1"
    }
    async fn execute(&self, request: &ExecutionRequest) -> TaskExecutionResult {
        self.calls.fetch_add(1, Ordering::SeqCst);
        (self.f)(request)
    }
}

/// V0.38 Phase 4: a quality-passing task with no actual worktree changes is
/// a distinct no-change failure now (rule: never `--allow-empty`), so a
/// stub "success" outcome needs a real change on disk when a real
/// Workspace is attached — otherwise Checkpoint correctly refuses to
/// fabricate one, and these loop-mechanics tests would spuriously fail on
/// a Phase-4 property they aren't testing. When no workspace is attached
/// (e.g. the manually-built loop in test 4), this is a no-op.
fn success(task: &Task, marker: &str, workspace: Option<&WorkspaceContext>) -> TaskExecutionResult {
    if let Some(ws) = workspace {
        let _ = std::fs::write(ws.worktree_path.join(".argus-e2e-marker"), marker);
    }
    TaskExecutionResult {
        task_id: task.id,
        success: true,
        exit_code: Some(0),
        stdout_summary: marker.to_string(),
        stderr_summary: String::new(),
        started_at: Utc::now(),
        finished_at: Utc::now(),
        failure_reason: None,
        files_changed: Vec::new(),
        workspace: None,
        metadata: HashMap::new(),
        workspace_id: None,
        branch_name: None,
        base_revision: None,
        worktree_path: None,
    }
}

fn failure(task: &Task, reason: &str, stderr: &str) -> TaskExecutionResult {
    TaskExecutionResult {
        task_id: task.id,
        success: false,
        exit_code: Some(17),
        stdout_summary: String::new(),
        stderr_summary: stderr.to_string(),
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
// Test 1 — a created task reaches Completed on its own
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn execution_loop_drives_created_task_to_completed() {
    let h = Harness::new("success").await;
    let project_id = insert_project(&h.pool, "loop-success").await;

    let marker = format!("argus-e2e-{}", Uuid::new_v4());
    let task = TaskBuilder::new(project_id, "e2e success")
        .description("vertical slice 1 success path")
        .build();
    let task_id = task.id;
    h.scheduler
        .add_task_durable(task)
        .await
        .expect("persist task");

    // Persisted and promoted to Ready before the loop ever starts.
    assert_eq!(h.persisted_state(task_id).await, "ready");

    let marker_for_exec = marker.clone();
    let executor: Arc<dyn TaskExecutor> = Arc::new(FnExecutor {
        calls: Arc::new(AtomicUsize::new(0)),
        f: move |req| success(&req.task, &marker_for_exec, req.workspace.as_ref()),
    });
    let handle = h.build_loop(executor).spawn();

    let final_state = await_state(
        &h.scheduler,
        task_id,
        TaskState::Completed,
        Duration::from_secs(30),
    )
    .await;
    handle.shutdown().await;

    assert_eq!(
        final_state,
        TaskState::Completed,
        "task must reach Completed with no manual dequeue"
    );
    assert_eq!(h.persisted_state(task_id).await, "completed");

    // The execution result is observable and reflects the real Workspace it ran in.
    let done = h.scheduler.get_task(task_id).await.expect("task present");
    let output = done.output.expect("execution output persisted");
    assert!(
        output.summary.contains(&marker),
        "stdout summary should carry this task's own output, got: {}",
        output.summary
    );
    assert_eq!(
        output.metadata.get("exit_code").map(String::as_str),
        Some("0")
    );
    assert!(output.metadata.contains_key("started_at"));
    assert!(output.metadata.contains_key("finished_at"));

    // Workspace lifecycle: a real worktree was created and used as cwd.
    let worktree = h.worktree_path_for(project_id, task_id).await;
    assert!(
        worktree.join(".git").exists(),
        "a real Git worktree backed this execution"
    );
}

// ============================================================
// Test 2 — a failing task lands in Failed and the loop keeps going
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn execution_loop_fails_task_and_continues_with_next() {
    let h = Harness::new("failure").await;
    let project_id = insert_project(&h.pool, "loop-failure").await;

    let mut failing = TaskBuilder::new(project_id, "e2e failure").build();
    failing.max_attempts = 1;
    let failing_id = failing.id;
    h.scheduler
        .add_task_durable(failing)
        .await
        .expect("persist failing task");

    // Only the intentionally-failing task fails; anything else (the
    // follow-up submitted below, to prove the loop survives) succeeds.
    let executor: Arc<dyn TaskExecutor> = Arc::new(FnExecutor {
        calls: Arc::new(AtomicUsize::new(0)),
        f: move |req| {
            if req.task.id == failing_id {
                let mut result = failure(&req.task, "command exited with status 17", "boom");
                result.metadata.insert("cost_usd".into(), "2.0".into());
                result
            } else {
                success(&req.task, "ran", req.workspace.as_ref())
            }
        },
    });
    let handle = h.build_loop(executor).spawn();

    let state = await_state(
        &h.scheduler,
        failing_id,
        TaskState::Failed,
        Duration::from_secs(30),
    )
    .await;
    assert_eq!(state, TaskState::Failed, "failing task must reach Failed");
    assert_eq!(h.persisted_state(failing_id).await, "failed");

    let failed = h
        .scheduler
        .get_task(failing_id)
        .await
        .expect("task present");
    let error = failed.error.expect("failure reason recorded");
    assert!(
        error.message.contains("status 17"),
        "failure reason must carry the real exit status, got: {}",
        error.message
    );
    assert!(
        error.message.contains("boom"),
        "failure reason must carry stderr evidence, got: {}",
        error.message
    );

    let spend = h
        .spend_repo
        .task_spend(failing_id)
        .await
        .expect("read failed execution spend");
    assert_eq!(spend.len(), 1);
    assert!(!spend[0].success);
    assert_eq!(spend[0].cost_usd, Some(2.0));
    assert_eq!(spend[0].execution_id, failed.execution_id);
    assert_eq!(
        spend[0].error.as_deref(),
        Some("command exited with status 17")
    );
    assert_eq!(
        h.spend_repo.spent_today_usd(project_id).await.unwrap(),
        Some(2.0)
    );
    assert_eq!(
        h.spend_repo.spent_this_month_usd(project_id).await.unwrap(),
        Some(2.0)
    );
    assert!(!h.spend_repo.record_spend(spend[0].clone()).await.unwrap());
    assert_eq!(h.spend_repo.task_spend(failing_id).await.unwrap().len(), 1);
    for (daily, per_task, expected) in [
        (Some(1.0), None, "daily budget exhausted"),
        (None, Some(1.0), "task budget exhausted"),
    ] {
        let budget = argus_core::budget_service::BudgetService::new(
            h.spend_repo.clone(),
            argus_core::budget_service::BudgetPolicy {
                daily_budget_usd: daily,
                max_per_task_usd: per_task,
                strict_mode: true,
            },
        );
        let decision = budget.preflight_check(project_id, failing_id, None).await;
        assert!(
            matches!(decision, argus_core::budget_service::BudgetDecision::Blocked { ref reason, .. } if reason.contains(expected)),
            "{decision:?}"
        );
    }

    // The loop is still alive: a task submitted afterwards still completes.
    let follow_up = TaskBuilder::new(project_id, "e2e after failure").build();
    let follow_up_id = follow_up.id;
    h.scheduler
        .add_task_durable(follow_up)
        .await
        .expect("persist follow-up task");

    let state = await_state(
        &h.scheduler,
        follow_up_id,
        TaskState::Completed,
        Duration::from_secs(30),
    )
    .await;
    handle.shutdown().await;

    assert_eq!(
        state,
        TaskState::Completed,
        "loop must keep processing after a task failure"
    );
}

// ============================================================
// Test 3 — several tasks in a row, each executed exactly once
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn execution_loop_processes_consecutive_tasks_exactly_once() {
    let h = Harness::new("batch").await;
    let project_id = insert_project(&h.pool, "loop-batch").await;

    let mut ids = Vec::new();
    for i in 0..4 {
        let task = TaskBuilder::new(project_id, format!("e2e batch {i}")).build();
        ids.push(task.id);
        h.scheduler
            .add_task_durable(task)
            .await
            .expect("persist batch task");
    }

    let calls = Arc::new(AtomicUsize::new(0));
    let executor: Arc<dyn TaskExecutor> = Arc::new(FnExecutor {
        calls: calls.clone(),
        f: |req| success(&req.task, "ran", req.workspace.as_ref()),
    });
    let handle = h.build_loop(executor).spawn();

    for task_id in &ids {
        let state = await_state(
            &h.scheduler,
            *task_id,
            TaskState::Completed,
            Duration::from_secs(45),
        )
        .await;
        assert_eq!(
            state,
            TaskState::Completed,
            "task {task_id} should complete"
        );
    }

    // Give the loop a moment in which a duplicate execution could happen.
    tokio::time::sleep(Duration::from_millis(300)).await;
    handle.shutdown().await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        ids.len(),
        "each task must execute exactly once — no duplicates, nothing dropped"
    );
    for task_id in &ids {
        assert_eq!(h.persisted_state(*task_id).await, "completed");
    }
    assert_eq!(h.scheduler.queue_len().await, 0, "queue must be drained");
}

// ============================================================
// Test 4 — restart: a restored task is picked up again
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn execution_loop_resumes_task_after_scheduler_restart() {
    let h = Harness::new("restart").await;
    let project_id = insert_project(&h.pool, "loop-restart").await;

    // --- "First process": create a task and strand it mid-flight in Running.
    let repository = Arc::new(argus_core::task_repository::PostgresTaskRepository::new(
        h.pool.clone(),
    ));
    let first = argus_core::task_scheduler::TaskScheduler::new_with_repository(
        Arc::clone(&repository) as Arc<_>,
        None,
    );
    let mut task = TaskBuilder::new(project_id, "e2e restart").build();
    task.max_attempts = 1;
    let task_id = task.id;
    first.add_task_durable(task).await.expect("persist task");
    assert!(first.claim_task(task_id).await.expect("claim"));
    assert!(first.transition(task_id, TaskState::Assigned).await);
    assert!(first.transition(task_id, TaskState::Running).await);
    assert_eq!(h.persisted_state(task_id).await, "running");
    drop(first);

    // --- "Second process": restore, then start the loop against the SAME
    // Workspace/Budget stack (a restart doesn't get a new database).
    let second = argus_core::task_scheduler::TaskScheduler::new_with_repository(
        Arc::clone(&repository) as Arc<_>,
        None,
    );
    second.restore().await.expect("restore scheduler state");

    let restored = second.get_task(task_id).await.expect("task restored");
    assert_eq!(
        restored.state,
        TaskState::Ready,
        "an interrupted Running task must come back as Ready"
    );
    assert_eq!(
        restored.resume_count, 1,
        "restore must record the resume attempt"
    );

    let second = Arc::new(second);
    let executor: Arc<dyn TaskExecutor> = Arc::new(FnExecutor {
        calls: Arc::new(AtomicUsize::new(0)),
        f: |req| success(&req.task, "resumed-after-restart", req.workspace.as_ref()),
    });
    let handle = argus_core::execution_loop::ExecutionLoop::new(
        Arc::clone(&second),
        executor,
        e2e_support::fast_loop(),
    )
    .with_budget_service(h.budget_service.clone())
    .with_workspace(
        h.workspace_repo.clone(),
        h.workspace_manager.clone(),
        h.workspace_guard.clone(),
        h.canonical_root.clone(),
    )
    .with_checkpoint(h.checkpointer.clone(), h.artifact_repo.clone())
    .spawn();

    let state = await_state(
        &second,
        task_id,
        TaskState::Completed,
        Duration::from_secs(30),
    )
    .await;
    handle.shutdown().await;

    assert_eq!(
        state,
        TaskState::Completed,
        "a restored task must not be stuck; the loop should finish it"
    );
    assert_eq!(h.persisted_state(task_id).await, "completed");
}

// ============================================================
// Test 5 — the real daemon wires the loop (and Workspace) up over HTTP
// ============================================================

/// This is the test that catches "the loop works but production forgot to
/// start it": it drives the real `argus run` binary over HTTP, through real
/// Workspace isolation, with a stub agent binary standing in for `claude`
/// (`ARGUS_AGENT_BINARY_OVERRIDE`) so it costs nothing to run.
#[tokio::test]
#[ignore = "requires postgres and spawns the argus daemon"]
async fn daemon_run_starts_execution_loop_and_completes_task_over_http() {
    let pool = e2e_support::connect().await;
    let project_id = insert_project(&pool, "loop-daemon").await;
    let scratch = e2e_support::Scratch::new("daemon");
    let workspace_root = scratch.path().join("workspace");
    let worktree_root = scratch.path().join("worktrees");
    let port = free_port();

    let marker = format!("daemon-e2e-{}", Uuid::new_v4());
    let stub = write_stub_agent(
        scratch.path(),
        "agent.sh",
        &format!(
            "printf '{marker}\\n' > daemon_marker.txt\n\
             printf '{json}\\n'\n\
             exit 0",
            json = r#"{"is_error":false,"subtype":"success","result":"wrote daemon_marker.txt","total_cost_usd":0.0,"num_turns":1,"session_id":"stub","usage":{"input_tokens":1,"output_tokens":1},"type":"result"}"#
        ),
    );

    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("resolve repo root");
    let mut daemon = std::process::Command::new(env!("CARGO_BIN_EXE_argus"))
        .arg("run")
        .current_dir(&repo_root)
        .env("ARGUS_DATABASE_URL", e2e_support::database_url())
        .env("ARGUS_MASTER_KEY", MASTER_KEY)
        .env("ARGUS_ACTOR_ROLE", "owner")
        .env("ARGUS_HTTP_ADMIN_TOKEN", MASTER_KEY)
        .env("ARGUS_SERVER_PORT", port.to_string())
        .env("ARGUS_WORKSPACE_ROOT", workspace_root.display().to_string())
        .env("ARGUS_WORKTREE_ROOT", worktree_root.display().to_string())
        .env("ARGUS_AGENT_BINARY_OVERRIDE", stub.display().to_string())
        .spawn()
        .expect("spawn argus daemon");

    // Always reap the daemon, even if an assertion below fails.
    let result = run_daemon_assertions(port, project_id, &worktree_root, &marker).await;
    let _ = daemon.kill();
    let _ = daemon.wait();
    result.expect("daemon e2e assertions");
}

async fn run_daemon_assertions(
    port: u16,
    project_id: Uuid,
    worktree_root: &Path,
    marker: &str,
) -> anyhow::Result<()> {
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;

    // Wait for the HTTP server to come up.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        if tokio::time::Instant::now() > deadline {
            anyhow::bail!("daemon did not start listening on {port}");
        }
        if client
            .get(format!("{base}/api/status"))
            .send()
            .await
            .is_ok_and(|r| r.status().is_success())
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    let created: serde_json::Value = client
        .post(format!("{base}/api/tasks"))
        .bearer_auth(MASTER_KEY)
        .json(&serde_json::json!({
            "project_id": project_id.to_string(),
            "title": "daemon e2e task",
            "description": "created over HTTP against the real daemon",
            "priority": "high",
        }))
        .send()
        .await?
        .json()
        .await?;

    let task_id = created["id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("no task id in response: {created}"))?
        .to_string();
    anyhow::ensure!(!task_id.is_empty(), "empty task id: {created}");

    // Poll the public API only — no scheduler access, no manual dequeue.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let mut last_status = String::new();
    loop {
        if tokio::time::Instant::now() > deadline {
            anyhow::bail!(
                "task {task_id} never reached a terminal state (last status: {last_status})"
            );
        }
        let task: serde_json::Value = client
            .get(format!("{base}/api/tasks/{task_id}"))
            .send()
            .await?
            .json()
            .await?;
        last_status = task["status"].as_str().unwrap_or_default().to_string();
        if last_status == "completed" {
            break;
        }
        anyhow::ensure!(
            last_status != "failed",
            "task failed in the daemon path: {task}"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    // The marker landed in a real worktree under this project, not just
    // "somewhere" — and never in the canonical repository.
    let project_worktrees = worktree_root.join(project_id.to_string());
    let mut found = None;
    if project_worktrees.is_dir() {
        for entry in std::fs::read_dir(&project_worktrees)? {
            let candidate = entry?.path().join("daemon_marker.txt");
            if candidate.is_file() {
                found = Some(candidate);
                break;
            }
        }
    }
    let file = found.ok_or_else(|| {
        anyhow::anyhow!(
            "daemon_marker.txt not found under {}",
            project_worktrees.display()
        )
    })?;
    let contents = std::fs::read_to_string(&file)?;
    anyhow::ensure!(
        contents.trim() == marker,
        "unexpected marker contents: {contents:?}"
    );

    let canonical_marker = worktree_root
        .parent()
        .expect("scratch dir")
        .join("workspace")
        .join(project_id.to_string())
        .join("daemon_marker.txt");
    anyhow::ensure!(
        !canonical_marker.exists(),
        "canonical repository must never receive the agent's edits directly"
    );

    Ok(())
}
