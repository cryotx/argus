//! V0.38 Phase 4 end-to-end tests: Quality Gate -> Git Checkpoint -> Task
//! Artifact -> Workspace Completed -> Task Completed, against a real
//! Postgres, a real Git worktree, and a real `git` binary — a stub agent
//! stands in for the external CLI so these cost nothing to run repeatedly.
//!
//! Idempotency across the three crash windows (commit-but-no-artifact,
//! artifact-persistence-failure, and a full finalize replay) is covered at
//! the orchestration-unit level in `argus-core::checkpoint`'s own tests
//! (against scriptable fakes) rather than here — simulating a real crash
//! mid-commit is not something a live subprocess test can do
//! deterministically, whereas the fakes can assert the exact sequence of
//! calls a real crash would produce.
//!
//! ```bash
//! ARGUS_DATABASE_URL=postgres://argus:argus@localhost:5432/argus \
//!   cargo test -p argus-cli --test checkpoint_artifact_e2e -- --ignored --test-threads=1
//! ```

mod e2e_support;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use argus_core::execution_loop::{ExecutionRequest, TaskExecutionResult, TaskExecutor};
use argus_core::task_types::{TaskBuilder, TaskState};

use e2e_support::{Harness, await_terminal, insert_project, write_stub_agent};

// ============================================================
// A stub `TaskExecutor` that runs a real script in the real worktree —
// exactly like `agent_pipeline_e2e.rs`'s `stub_executor`, duplicated here
// (rather than shared) because that helper lives in a different
// integration-test binary and Cargo gives each `tests/*.rs` file its own
// crate.
// ============================================================

struct ScriptExecutor {
    binary: String,
}

#[async_trait]
impl TaskExecutor for ScriptExecutor {
    fn name(&self) -> &str {
        "script-stub"
    }

    async fn execute(&self, request: &ExecutionRequest) -> TaskExecutionResult {
        let started_at = chrono::Utc::now();
        let Some(ws) = &request.workspace else {
            return TaskExecutionResult::failed(request.task.id, started_at, "no workspace");
        };
        let output = tokio::process::Command::new(&self.binary)
            .current_dir(&ws.worktree_path)
            .output()
            .await;
        match output {
            Ok(out) => TaskExecutionResult {
                task_id: request.task.id,
                success: out.status.success(),
                exit_code: out.status.code(),
                stdout_summary: String::from_utf8_lossy(&out.stdout).trim().to_string(),
                stderr_summary: String::from_utf8_lossy(&out.stderr).trim().to_string(),
                started_at,
                finished_at: chrono::Utc::now(),
                failure_reason: (!out.status.success())
                    .then(|| format!("command exited with status {:?}", out.status.code())),
                files_changed: Vec::new(), // recomputed authoritatively by ExecutionLoop
                workspace: Some(ws.worktree_path.clone()),
                metadata: Default::default(),
                workspace_id: Some(ws.workspace_id),
                branch_name: Some(ws.branch_name.clone()),
                base_revision: Some(ws.base_revision.clone()),
                worktree_path: Some(ws.worktree_path.clone()),
            },
            Err(e) => TaskExecutionResult::failed(request.task.id, started_at, e.to_string()),
        }
    }
}

fn stub_executor(binary: impl Into<String>) -> Arc<dyn TaskExecutor> {
    Arc::new(ScriptExecutor {
        binary: binary.into(),
    })
}

/// Real `RepoManager`-level HEAD read, to compare the task branch against
/// its `base_revision` without going through any ARGUS abstraction.
fn git_head(path: &std::path::Path) -> String {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(path)
        .output()
        .expect("git rev-parse HEAD");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

// ============================================================
// Test 1 — the successful path produces a real, verifiable checkpoint
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn successful_checkpoint_produces_verified_artifact() {
    let h = Harness::new("checkpoint-ok").await;
    let project_id = insert_project(&h.pool, "checkpoint-ok").await;

    let stub = write_stub_agent(
        h.scratch_path(),
        "agent.sh",
        "printf 'hello\\n' > feature.txt\necho wrote feature.txt\nexit 0",
    );

    let task = TaskBuilder::new(project_id, "Add feature.txt")
        .description("Create feature.txt containing hello")
        .build();
    let task_id = task.id;
    h.scheduler.add_task_durable(task).await.expect("persist");

    // The canonical repo doesn't exist on disk until the loop's first
    // `ensure_canonical_repo` call, so there is no "before" HEAD to read
    // yet — canonical-untouched is instead verified below via the same
    // `canonical_repo_contaminated` before/after check `ExecutionLoop`
    // itself performs (rule #8).

    let handle = h
        .build_loop(stub_executor(stub.display().to_string()))
        .spawn();
    let state = await_terminal(&h.scheduler, task_id, Duration::from_secs(60)).await;
    handle.shutdown().await;

    let done = h.scheduler.get_task(task_id).await.unwrap();
    assert_eq!(state, TaskState::Completed, "got error: {:?}", done.error);
    assert_eq!(h.persisted_state(task_id).await, "completed");

    let worktree = h.worktree_path_for(project_id, task_id).await;
    let branch_head = git_head(&worktree);

    let output = done.output.clone().expect("output recorded");
    let commit_sha = output
        .commit_sha
        .clone()
        .expect("output should carry the checkpoint commit sha");
    assert_eq!(
        commit_sha, branch_head,
        "artifact commit SHA must equal the task branch HEAD"
    );

    let artifact = h
        .artifact_for_task(task_id)
        .await
        .expect("artifact must be persisted");
    assert_eq!(artifact.commit_sha, branch_head);
    assert_eq!(artifact.project_id, project_id);
    assert_eq!(artifact.task_id, task_id);
    assert_eq!(
        artifact.execution_id,
        done.execution_id.expect("execution_id set")
    );
    assert!(artifact.files_changed.contains(&"feature.txt".to_string()));
    assert_eq!(artifact.quality_status.as_deref(), Some("passed"));
    assert_ne!(
        artifact.base_revision, branch_head,
        "a real commit must have been created"
    );

    // Workspace lifecycle reached Completed only via the artifact path.
    let (ws_status,): (String,) =
        sqlx::query_as("SELECT status FROM task_workspaces WHERE workspace_id = $1")
            .bind(artifact.workspace_id)
            .fetch_one(&h.pool)
            .await
            .expect("read workspace status");
    assert_eq!(ws_status, "completed");

    // Canonical repository was never touched.
    assert_eq!(
        output
            .metadata
            .get("canonical_repo_contaminated")
            .map(String::as_str),
        Some("false"),
        "canonical repository HEAD must never move and must gain no new dirt"
    );
    assert!(
        !h.canonical_root
            .join(project_id.to_string())
            .join("feature.txt")
            .exists(),
        "canonical repository must never receive the agent's edits directly"
    );
}

// ============================================================
// Test 2 — a Quality Gate failure must never produce a commit
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn quality_gate_failure_creates_no_checkpoint() {
    let h = Harness::new("checkpoint-quality-fail").await;
    let project_id = insert_project(&h.pool, "checkpoint-quality-fail").await;

    let stub = write_stub_agent(
        h.scratch_path(),
        "agent.sh",
        "printf 'WRONG\\n' > result.txt\necho wrote result.txt\nexit 0",
    );

    let task = TaskBuilder::new(project_id, "Produce result.txt containing ok")
        .test_command("grep -q ok result.txt")
        .build();
    let task_id = task.id;
    h.scheduler.add_task_durable(task).await.expect("persist");

    let handle = h
        .build_loop(stub_executor(stub.display().to_string()))
        .spawn();
    let state = await_terminal(&h.scheduler, task_id, Duration::from_secs(60)).await;
    handle.shutdown().await;

    assert_eq!(state, TaskState::Failed, "failing tests must fail the task");

    let worktree = h.worktree_path_for(project_id, task_id).await;
    let base_revision = {
        let (base,): (String,) =
            sqlx::query_as("SELECT base_revision FROM task_workspaces WHERE task_id = $1")
                .bind(task_id)
                .fetch_one(&h.pool)
                .await
                .expect("read base_revision");
        base
    };
    assert_eq!(
        git_head(&worktree),
        base_revision,
        "no commit should exist on the task branch after a quality failure"
    );

    assert!(
        h.artifact_for_task(task_id).await.is_none(),
        "no artifact may exist after a quality failure"
    );

    // The dirty (failing) change is still on disk — never rolled back by
    // the checkpoint path (only the Quality Gate's own configured rollback,
    // which is a no-op here, applies).
    assert_eq!(
        std::fs::read_to_string(worktree.join("result.txt"))
            .expect("file present")
            .trim(),
        "WRONG"
    );
}

// ============================================================
// Test 3 — quality passing with zero real changes is a distinct failure,
// never a silently-empty checkpoint
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn quality_pass_with_no_changes_fails_without_allow_empty() {
    let h = Harness::new("checkpoint-no-changes").await;
    let project_id = insert_project(&h.pool, "checkpoint-no-changes").await;

    // A stub that claims success without writing anything. (The real
    // `AgentTaskExecutor` already refuses this at the executor level —
    // this test proves the checkpoint layer itself refuses it too, for any
    // executor that doesn't.)
    let stub = write_stub_agent(
        h.scratch_path(),
        "agent.sh",
        "echo 'nothing to do here'\nexit 0",
    );

    let task = TaskBuilder::new(project_id, "A task with no real work").build();
    let task_id = task.id;
    h.scheduler.add_task_durable(task).await.expect("persist");

    let handle = h
        .build_loop(stub_executor(stub.display().to_string()))
        .spawn();
    let state = await_terminal(&h.scheduler, task_id, Duration::from_secs(60)).await;
    handle.shutdown().await;

    let done = h.scheduler.get_task(task_id).await.unwrap();
    assert_eq!(
        state,
        TaskState::Failed,
        "a quality-passing task with no worktree changes must not complete"
    );
    let err = done.error.expect("failure reason recorded");
    assert!(
        err.message.contains("no changes"),
        "failure reason should name the real problem, got: {}",
        err.message
    );

    assert!(h.artifact_for_task(task_id).await.is_none());

    let worktree = h.worktree_path_for(project_id, task_id).await;
    let (base_revision,): (String,) =
        sqlx::query_as("SELECT base_revision FROM task_workspaces WHERE task_id = $1")
            .bind(task_id)
            .fetch_one(&h.pool)
            .await
            .expect("read base_revision");
    assert_eq!(
        git_head(&worktree),
        base_revision,
        "no commit — not even an empty one — may exist"
    );
}
