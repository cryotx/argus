//! Vertical Slice 2 end-to-end tests: ExecutionLoop -> Workspace -> Budget ->
//! AgentDriver -> real Git worktree edits -> quality gate -> result back in
//! the task record.
//!
//! V0.38.1: updated to the current production API — Workspace isolation
//! (real `PostgresWorkspaceRepository` + `WorkspaceManager` +
//! `GitWorkspaceGuard`) and `BudgetService` are wired in exactly as
//! `argus run` wires them, and `TaskExecutor::execute` takes an
//! `ExecutionRequest`. Most tests drive a **stub agent**: a real executable
//! that really edits the repository (inside the real, verified worktree —
//! Workspace is never bypassed) and really returns exit codes, but costs
//! nothing. That keeps the whole pipeline — prompt building, process
//! spawning, workspace verification, git diffing, the quality gate, state
//! transitions — under automated test without spending API credits on every
//! `cargo test`.
//!
//! One test uses a **real** coding agent and is `#[ignore]`d because it
//! costs money; a separate test drives the real `argus run` daemon over
//! HTTP with the stub standing in for `claude` via `ARGUS_AGENT_BINARY_OVERRIDE`.
//! Run the free ones:
//!
//! ```bash
//! ARGUS_DATABASE_URL=postgres://argus:argus@localhost:5432/argus \
//!   cargo test -p argus-cli --test agent_pipeline_e2e -- --ignored --test-threads=1
//! ```

mod e2e_support;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use uuid::Uuid;

use argus_core::execution_loop::{ExecutionRequest, TaskExecutionResult, TaskExecutor};
use argus_core::task_types::{TaskBuilder, TaskState};
use argus_worker::AgentTaskExecutor;
use argus_worker::agent::{AgentDriver, ClaudeCodeDriver, StubAgentDriver};

use e2e_support::{
    Harness, MASTER_KEY, await_terminal, free_port, insert_project, write_stub_agent,
};

/// Adapts an `AgentTaskExecutor` (which still only knows about the task, not
/// the wrapping `ExecutionRequest`) to the current `TaskExecutor` trait — a
/// thin pass-through, not a workaround: it forwards the real
/// `WorkspaceContext` untouched.
struct AgentExecutorAdapter(AgentTaskExecutor);

#[async_trait::async_trait]
impl TaskExecutor for AgentExecutorAdapter {
    fn name(&self) -> &str {
        self.0.name()
    }
    async fn execute(&self, request: &ExecutionRequest) -> TaskExecutionResult {
        self.0.execute(request).await
    }
}

fn stub_executor(binary: impl Into<String>) -> Arc<dyn TaskExecutor> {
    Arc::new(AgentExecutorAdapter(AgentTaskExecutor::new(
        Arc::new(StubAgentDriver::new(binary)),
        // AgentTaskExecutor's own `workspace_root` is only its no-Workspace
        // fallback; every test here attaches real Workspace integration via
        // `Harness::build_loop`, so this path is never actually taken.
        std::env::temp_dir(),
    )))
}

// ============================================================
// Test 1 — agent edits the repo, task completes, diff is recorded
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn agent_edits_repository_and_task_completes() {
    let h = Harness::new("edit").await;
    let project_id = insert_project(&h.pool, "edit").await;

    let stub = write_stub_agent(
        h.scratch_path(),
        "agent.sh",
        r#"
mkdir -p src
printf 'pub fn greet(n: &str) -> String { format!("Hello, {n}!") }\n' > src/lib.rs
printf 'ARGUS_STUB_WROTE_THIS\n' > NOTES.md
echo "created src/lib.rs and NOTES.md"
exit 0
"#,
    );

    let task = TaskBuilder::new(project_id, "Add a greet function")
        .description("Add greet(name) to src/lib.rs")
        .acceptance("greet returns a greeting")
        .build();
    let task_id = task.id;
    h.scheduler.add_task_durable(task).await.expect("persist");

    let handle = h
        .build_loop(stub_executor(stub.display().to_string()))
        .spawn();
    let state = await_terminal(&h.scheduler, task_id, Duration::from_secs(60)).await;
    handle.shutdown().await;

    assert_eq!(state, TaskState::Completed, "agent task should complete");

    // The real, verified worktree was modified on disk...
    let worktree = h.worktree_path_for(project_id, task_id).await;
    assert!(
        worktree.join(".git").exists(),
        "worktree should be a real git worktree"
    );
    let lib = std::fs::read_to_string(worktree.join("src/lib.rs")).expect("src/lib.rs written");
    assert!(lib.contains("pub fn greet"), "agent's edit is on disk");

    // ...and the canonical repository was not.
    let canonical = h.canonical_root.join(project_id.to_string());
    assert!(
        !canonical.join("src").exists(),
        "canonical repository must never receive the agent's edits directly"
    );

    // And the change is recorded back on the task.
    let done = h.scheduler.get_task(task_id).await.unwrap();
    let output = done.output.expect("output recorded");
    assert!(
        output.files_changed.contains(&"src/lib.rs".to_string()),
        "git diff should list src/lib.rs, got {:?}",
        output.files_changed
    );
    assert!(
        output.files_changed.contains(&"NOTES.md".to_string()),
        "git diff should list NOTES.md, got {:?}",
        output.files_changed
    );
    assert!(
        output.summary.contains("created src/lib.rs"),
        "agent's final message should be the summary, got: {}",
        output.summary
    );
    assert_eq!(h.persisted_state(task_id).await, "completed");
}

// ============================================================
// Test 2 — an agent that changes nothing is a failure, not a pass
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn agent_that_changes_nothing_fails() {
    let h = Harness::new("noop").await;
    let project_id = insert_project(&h.pool, "noop").await;

    let stub = write_stub_agent(
        h.scratch_path(),
        "agent.sh",
        "echo 'I have completed the task successfully.'\nexit 0",
    );

    let task = TaskBuilder::new(project_id, "Do something real").build();
    let task_id = task.id;
    h.scheduler.add_task_durable(task).await.expect("persist");

    let handle = h
        .build_loop(stub_executor(stub.display().to_string()))
        .spawn();
    let state = await_terminal(&h.scheduler, task_id, Duration::from_secs(60)).await;
    handle.shutdown().await;

    assert_eq!(
        state,
        TaskState::Failed,
        "an agent that edits nothing must not be reported as success"
    );
    let err = h
        .scheduler
        .get_task(task_id)
        .await
        .unwrap()
        .error
        .expect("failure reason");
    assert!(
        err.message.contains("without modifying any file"),
        "reason should name the real problem, got: {}",
        err.message
    );
}

// ============================================================
// Test 3 — the quality gate really runs tests against the agent's work
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn quality_gate_runs_tests_against_agent_output() {
    let h = Harness::new("tests").await;

    // --- Case A: agent writes code that satisfies the test command.
    let good_project = insert_project(&h.pool, "tests-pass").await;
    let good_stub = write_stub_agent(
        h.scratch_path(),
        "good.sh",
        "printf 'ok\\n' > result.txt\necho wrote result.txt\nexit 0",
    );
    let good_task = TaskBuilder::new(good_project, "Produce result.txt containing ok")
        .test_command("grep -q ok result.txt")
        .build();
    let good_id = good_task.id;
    h.scheduler
        .add_task_durable(good_task)
        .await
        .expect("persist");

    let handle = h
        .build_loop(stub_executor(good_stub.display().to_string()))
        .spawn();
    let state = await_terminal(&h.scheduler, good_id, Duration::from_secs(60)).await;
    handle.shutdown().await;
    assert_eq!(
        state,
        TaskState::Completed,
        "passing tests should let the task complete"
    );

    // --- Case B: agent writes a file, but the test command fails on it.
    let bad_project = insert_project(&h.pool, "tests-fail").await;
    let bad_stub = write_stub_agent(
        h.scratch_path(),
        "bad.sh",
        "printf 'WRONG\\n' > result.txt\necho wrote result.txt\nexit 0",
    );
    let bad_task = TaskBuilder::new(bad_project, "Produce result.txt containing ok")
        .test_command("grep -q ok result.txt")
        .build();
    let bad_id = bad_task.id;
    h.scheduler
        .add_task_durable(bad_task)
        .await
        .expect("persist");

    let handle = h
        .build_loop(stub_executor(bad_stub.display().to_string()))
        .spawn();
    let state = await_terminal(&h.scheduler, bad_id, Duration::from_secs(60)).await;
    handle.shutdown().await;

    assert_eq!(
        state,
        TaskState::Failed,
        "the agent edited a file but the tests fail, so the task must fail"
    );
    // The file really was written — proving the gate, not the agent, rejected it.
    let worktree = h.worktree_path_for(bad_project, bad_id).await;
    assert_eq!(
        std::fs::read_to_string(worktree.join("result.txt"))
            .expect("file written")
            .trim(),
        "WRONG"
    );
}

// ============================================================
// Test 4 — a crashing agent fails cleanly and the loop survives
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn crashing_agent_fails_task_without_killing_loop() {
    let h = Harness::new("crash").await;
    let project_id = insert_project(&h.pool, "crash").await;

    let crash_stub = write_stub_agent(
        h.scratch_path(),
        "crash.sh",
        "echo 'agent exploded' >&2\nexit 42",
    );
    let good_stub = write_stub_agent(
        h.scratch_path(),
        "good.sh",
        "printf 'recovered\\n' > after.txt\necho done\nexit 0",
    );

    let mut crashing = TaskBuilder::new(project_id, "will crash").build();
    crashing.max_attempts = 1;
    let crashing_id = crashing.id;
    h.scheduler
        .add_task_durable(crashing)
        .await
        .expect("persist");

    let handle = h
        .build_loop(stub_executor(crash_stub.display().to_string()))
        .spawn();
    let state = await_terminal(&h.scheduler, crashing_id, Duration::from_secs(60)).await;
    handle.shutdown().await;

    assert_eq!(state, TaskState::Failed);
    let err = h
        .scheduler
        .get_task(crashing_id)
        .await
        .unwrap()
        .error
        .expect("failure reason");
    assert!(
        err.message.contains("42"),
        "exit status should be reported, got: {}",
        err.message
    );
    assert!(
        err.message.contains("exploded"),
        "agent stderr should be reported, got: {}",
        err.message
    );

    // The same scheduler, a fresh loop with a working agent, still processes
    // the next task — the crash must not have killed anything.
    let follow_up = TaskBuilder::new(project_id, "after crash").build();
    let follow_up_id = follow_up.id;
    h.scheduler
        .add_task_durable(follow_up)
        .await
        .expect("persist");

    let handle = h
        .build_loop(stub_executor(good_stub.display().to_string()))
        .spawn();
    let state = await_terminal(&h.scheduler, follow_up_id, Duration::from_secs(60)).await;
    handle.shutdown().await;
    assert_eq!(state, TaskState::Completed, "loop recovers after a crash");
}

// ============================================================
// Test 5 — the missing-binary path is explicit
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn missing_agent_binary_fails_with_clear_reason() {
    let h = Harness::new("missing").await;
    let project_id = insert_project(&h.pool, "missing").await;

    let task = TaskBuilder::new(project_id, "needs a missing agent").build();
    let task_id = task.id;
    h.scheduler.add_task_durable(task).await.expect("persist");

    let handle = h
        .build_loop(stub_executor("argus-no-such-agent-binary"))
        .spawn();
    let state = await_terminal(&h.scheduler, task_id, Duration::from_secs(60)).await;
    handle.shutdown().await;

    assert_eq!(state, TaskState::Failed);
    let err = h
        .scheduler
        .get_task(task_id)
        .await
        .unwrap()
        .error
        .expect("failure reason");
    assert!(
        err.message.contains("not found on PATH"),
        "got: {}",
        err.message
    );
}

// ============================================================
// Test 6 — REAL coding agent. Costs money. Opt-in only.
// ============================================================

/// The one test that proves the whole V0.38 Phase 4 chain against a real
/// model: `HTTP-equivalent task -> Scheduler -> Workspace -> Budget ->
/// Claude Code -> Quality -> Git Checkpoint -> Artifact -> Completed`.
///
/// Rule #25/#26: this must never spend money just because someone ran
/// `cargo test -- --ignored`. `#[ignore]` alone is not that guard — it only
/// stops the *default* `cargo test`. The real guard is
/// `ARGUS_RUN_REAL_AGENT_TESTS=1`, checked *before* anything else (before
/// even `ClaudeCodeDriver::is_available()`), so an unset/false value spawns
/// zero real Agent processes, full stop:
///
/// ```bash
/// ARGUS_DATABASE_URL=postgres://argus:argus@localhost:5432/argus \
/// ARGUS_RUN_REAL_AGENT_TESTS=1 \
///   cargo test -p argus-cli --test agent_pipeline_e2e -- --ignored --test-threads=1 \
///   real_coding_agent_creates_a_verified_git_checkpoint
/// ```
#[tokio::test]
#[ignore = "costs API credits: runs a real coding agent"]
async fn real_coding_agent_creates_a_verified_git_checkpoint() {
    if std::env::var("ARGUS_RUN_REAL_AGENT_TESTS").as_deref() != Ok("1") {
        eprintln!(
            "skipping: set ARGUS_RUN_REAL_AGENT_TESTS=1 to run this test (it costs API credits). \
             REAL AGENT SPAWNS: 0"
        );
        return;
    }

    let driver = ClaudeCodeDriver::default();
    if !driver.is_available().await {
        eprintln!("skipping: `claude` not on PATH");
        return;
    }

    let h = Harness::new("real").await;
    let project_id = insert_project(&h.pool, "real").await;

    let executor: Arc<dyn TaskExecutor> = Arc::new(AgentExecutorAdapter(
        AgentTaskExecutor::new(Arc::new(driver), std::env::temp_dir())
            // Cheap model: this task is trivial and the point is the pipeline.
            .with_model(Some("haiku".to_string())),
    ));

    let mut task = TaskBuilder::new(project_id, "Create a checkpoint smoke-test file")
        .description(
            "Create a file named checkpoint_smoke.txt in the repository root \
             containing exactly this one line: ARGUS GIT CHECKPOINT",
        )
        .acceptance("checkpoint_smoke.txt exists and contains exactly \"ARGUS GIT CHECKPOINT\"")
        // Verified by a command the agent does not control.
        .test_command("test \"$(cat checkpoint_smoke.txt)\" = \"ARGUS GIT CHECKPOINT\" && echo ok")
        .timeout(Duration::from_secs(300))
        .build();
    task.max_attempts = 1;
    let task_id = task.id;
    h.scheduler.add_task_durable(task).await.expect("persist");

    let handle = h.build_loop(executor).spawn();
    let state = await_terminal(&h.scheduler, task_id, Duration::from_secs(360)).await;
    handle.shutdown().await;

    let done = h.scheduler.get_task(task_id).await.unwrap();
    assert_eq!(
        state,
        TaskState::Completed,
        "real agent task failed: {:?}",
        done.error
    );

    let worktree = h.worktree_path_for(project_id, task_id).await;
    let smoke = std::fs::read_to_string(worktree.join("checkpoint_smoke.txt"))
        .expect("checkpoint_smoke.txt written by agent");
    assert_eq!(smoke.trim(), "ARGUS GIT CHECKPOINT");

    let output = done.output.expect("output recorded");
    assert!(
        output
            .files_changed
            .iter()
            .any(|f| f == "checkpoint_smoke.txt"),
        "git diff should list checkpoint_smoke.txt, got {:?}",
        output.files_changed
    );
    assert!(
        output.metadata.contains_key("cost_usd"),
        "claude driver should record cost, metadata: {:?}",
        output.metadata
    );

    // --- V0.38 Phase 4 assertions ---
    let commit_sha = output
        .commit_sha
        .clone()
        .expect("task output should carry the checkpoint commit SHA");
    let head = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&worktree)
        .output()
        .expect("git rev-parse");
    assert_eq!(
        String::from_utf8_lossy(&head.stdout).trim(),
        commit_sha,
        "the task branch HEAD must be the checkpointed commit"
    );

    let artifact = h
        .artifact_for_task(task_id)
        .await
        .expect("a Git-checkpoint artifact must be persisted");
    assert_eq!(artifact.commit_sha, commit_sha);
    assert_eq!(artifact.project_id, project_id);
    assert_eq!(artifact.task_id, task_id);
    assert_eq!(artifact.agent_runtime.as_deref(), Some("agent:claude_code"));

    // Canonical repository must never have received the agent's edits
    // directly, or grown a checkpoint commit of its own. The canonical repo
    // is created lazily on the loop's first `ensure_canonical_repo` call, so
    // a before/after directory-entry count is the wrong probe (it would only
    // measure "did the directory get created"). Assert on the ExecutionLoop's
    // own authoritative before/after canonical HEAD and contamination flag
    // instead — the same check `ExecutionLoop` performs on every execution.
    let canonical = h.canonical_root.join(project_id.to_string());
    assert!(
        !canonical.join("checkpoint_smoke.txt").exists(),
        "canonical repository must never receive the agent's edits directly"
    );
    assert_eq!(
        output
            .metadata
            .get("canonical_repo_contaminated")
            .map(String::as_str),
        Some("false"),
        "canonical repository must never be used as agent cwd or modified"
    );
    assert_eq!(
        output.metadata.get("canonical_head_before"),
        output.metadata.get("canonical_head_after"),
        "canonical repository HEAD must not move"
    );
    assert_ne!(
        output
            .metadata
            .get("canonical_head_after")
            .map(String::as_str),
        Some(commit_sha.as_str()),
        "the checkpoint commit must never land on the canonical branch"
    );

    println!(
        "real agent checkpoint: task_id={task_id} execution_id={:?} workspace_id={} \
         model={:?} session={:?} duration_ms={} cost_usd={:?} base_revision={} branch={} \
         commit_sha={commit_sha} files_changed={:?} artifact_id={}",
        done.execution_id,
        artifact.workspace_id,
        output.metadata.get("model"),
        output.metadata.get("agent_session_id"),
        output
            .metadata
            .get("duration_ms")
            .cloned()
            .unwrap_or_default(),
        output.metadata.get("cost_usd"),
        artifact.base_revision,
        artifact.branch_name,
        artifact.files_changed,
        artifact.artifact_id,
    );
}

// ============================================================
// Test 7 — the real daemon wires the AGENT executor (and Workspace) up
// ============================================================

/// Slice 1 proved `cmd_run` starts the loop. This proves `cmd_run` also
/// selects and drives the *agent* executor through real Workspace isolation
/// — using a stub agent binary (via `ARGUS_AGENT_BINARY_OVERRIDE`), so it
/// verifies the wiring without spending credits.
#[tokio::test]
#[ignore = "requires postgres and spawns the argus daemon"]
async fn daemon_run_uses_agent_executor() {
    let pool = e2e_support::connect().await;
    let project_id = insert_project(&pool, "daemon-agent").await;
    let scratch = e2e_support::Scratch::new("daemon-agent");
    let workspace_root = scratch.path().join("workspace");
    let worktree_root = scratch.path().join("worktrees");
    let port = free_port();

    let marker = format!("daemon-agent-{}", Uuid::new_v4());
    // The stub impersonates the CLI it stands in for: it edits the worktree
    // *and* emits a Claude-Code-shaped result object, so this exercises the
    // real `ClaudeCodeDriver` parser on the daemon's own code path.
    let stub = write_stub_agent(
        scratch.path(),
        "agent.sh",
        &format!(
            "printf '{marker}\\n' > agent_was_here.txt\n\
             printf '{json}\\n'\n\
             exit 0",
            json = r#"{"is_error":false,"subtype":"success","result":"wrote agent_was_here.txt","total_cost_usd":0.0,"num_turns":1,"session_id":"stub","usage":{"input_tokens":1,"output_tokens":1},"type":"result"}"#
        ),
    );

    let repo_root = repo_root_dir();
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

    let result = daemon_agent_assertions(port, project_id, &worktree_root, &marker).await;
    let _ = daemon.kill();
    let _ = daemon.wait();
    result.expect("daemon agent assertions");
}

/// `crates/argus-cli` -> repo root, so the spawned daemon resolves
/// `config/argus.toml` exactly the way a real deployment does.
fn repo_root_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("resolve repo root")
}

async fn daemon_agent_assertions(
    port: u16,
    project_id: Uuid,
    worktree_root: &Path,
    marker: &str,
) -> anyhow::Result<()> {
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        if tokio::time::Instant::now() > deadline {
            anyhow::bail!("daemon never started listening");
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

    // No `command` field: the agent executor derives the work from the task.
    let created: serde_json::Value = client
        .post(format!("{base}/api/tasks"))
        .bearer_auth(MASTER_KEY)
        .json(&serde_json::json!({
            "project_id": project_id.to_string(),
            "title": "have the agent create a file",
            "description": "the stub agent writes agent_was_here.txt",
            "priority": "high",
        }))
        .send()
        .await?
        .json()
        .await?;
    let task_id = created["id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("no id: {created}"))?
        .to_string();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    loop {
        if tokio::time::Instant::now() > deadline {
            anyhow::bail!("task {task_id} never reached a terminal state");
        }
        let task: serde_json::Value = client
            .get(format!("{base}/api/tasks/{task_id}"))
            .send()
            .await?
            .json()
            .await?;
        match task["status"].as_str().unwrap_or_default() {
            "completed" => break,
            "failed" => anyhow::bail!("task failed in the daemon agent path: {task}"),
            _ => tokio::time::sleep(Duration::from_millis(250)).await,
        }
    }

    // The daemon's agent really wrote into the task's real Git worktree —
    // find it by scanning for the file rather than assuming an execution_id,
    // since the daemon (not this test) generated it.
    let mut found = None;
    if worktree_root.join(project_id.to_string()).is_dir() {
        for entry in std::fs::read_dir(worktree_root.join(project_id.to_string()))? {
            let candidate = entry?.path().join("agent_was_here.txt");
            if candidate.is_file() {
                found = Some(candidate);
                break;
            }
        }
    }
    let file = found.ok_or_else(|| {
        anyhow::anyhow!(
            "agent_was_here.txt not found under any worktree in {}",
            worktree_root.join(project_id.to_string()).display()
        )
    })?;
    let contents = std::fs::read_to_string(&file)?;
    anyhow::ensure!(
        contents.trim() == marker,
        "unexpected agent output: {contents:?}"
    );

    // And the canonical repository (not a worktree) must NOT have it.
    let canonical_file = workspace_root_sibling(worktree_root, project_id);
    anyhow::ensure!(
        !canonical_file.exists(),
        "canonical repository must never receive the agent's edits directly"
    );

    Ok(())
}

fn workspace_root_sibling(worktree_root: &Path, project_id: Uuid) -> std::path::PathBuf {
    worktree_root
        .parent()
        .expect("scratch dir")
        .join("workspace")
        .join(project_id.to_string())
        .join("agent_was_here.txt")
}
