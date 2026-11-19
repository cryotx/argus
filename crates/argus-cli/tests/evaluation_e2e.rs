//! V0.45: real Postgres, processes, worktrees, test commands and Git commits.
//! No model calls. Run with --ignored --test-threads=1 on a disposable DB.
mod e2e_support;

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use uuid::Uuid;

use argus_core::{
    evaluation::{
        EvaluationCheck, EvaluationContext, EvaluationHarness, EvaluationOutcome, Evaluator,
    },
    execution_loop::{ExecutionRequest, TaskExecutionResult, TaskExecutor},
    project_state::{
        NewMilestone, NewWorkItem, NewWorkItemEvidence, PostgresProjectStateRepository,
        ProjectStateService, WorkItemStatus,
    },
    task_types::{Priority, TaskBuilder, TaskState},
};
use argus_worker::{AgentTaskExecutor, agent::StubAgentDriver};
use e2e_support::{Harness, await_terminal, insert_project, write_stub_agent};

struct Executor(AgentTaskExecutor);
#[async_trait]
impl TaskExecutor for Executor {
    fn name(&self) -> &str {
        "evaluation-test-agent"
    }
    async fn execute(&self, request: &ExecutionRequest) -> TaskExecutionResult {
        self.0.execute(request).await
    }
}

struct Fixture {
    h: Harness,
    project: Uuid,
    item: Uuid,
    task: Uuid,
    state: ProjectStateService,
    evaluation: Arc<EvaluationHarness>,
}

async fn fixture(tests: bool) -> Fixture {
    let h = Harness::new("evaluation").await;
    let project = insert_project(&h.pool, "evaluation").await;
    let state = ProjectStateService::new(Arc::new(PostgresProjectStateRepository::new(
        h.pool.clone(),
    )));
    let milestone = state
        .create_milestone(NewMilestone {
            project_id: project,
            title: "Verification".into(),
            description: None,
            sequence: Some(1),
            parent_id: None,
            definition_of_done: None,
            external_key: None,
            weight: 1,
        })
        .await
        .unwrap()
        .milestone_id;
    let item = state
        .create_work_item(NewWorkItem {
            project_id: project,
            milestone_id: Some(milestone),
            phase_id: None,
            title: "Verified output".into(),
            description: "Write a verified file".into(),
            priority: Priority::Normal,
            definition_of_done: Some("verified.txt contains VERIFIED".into()),
            source: Some("e2e".into()),
            external_key: None,
            weight: 1,
        })
        .await
        .unwrap()
        .work_item_id;
    let mut builder = TaskBuilder::new(project, "Write verified file")
        .acceptance("verified.txt contains VERIFIED");
    if tests {
        builder = builder.test_command("test \"$(cat verified.txt)\" = VERIFIED");
    }
    let mut task = builder.build();
    task.max_attempts = 1;
    let task_id = task.id;
    h.scheduler.add_task_durable(task).await.unwrap();
    state
        .repository()
        .link_work_item_task(item, task_id)
        .await
        .unwrap();
    let binary = write_stub_agent(
        h.scratch_path(),
        "agent.sh",
        "printf 'VERIFIED\\n' > verified.txt",
    );
    let executor = Arc::new(Executor(AgentTaskExecutor::new(
        Arc::new(StubAgentDriver::new(binary.to_string_lossy())),
        h.canonical_root.clone(),
    )));
    let evaluation = Arc::new(EvaluationHarness::new(h.pool.clone()));
    let handle = h
        .build_loop(executor)
        .with_evaluation(evaluation.clone())
        .spawn();
    assert_eq!(
        await_terminal(&h.scheduler, task_id, Duration::from_secs(20)).await,
        TaskState::Completed
    );
    handle.shutdown().await;
    Fixture {
        h,
        project,
        item,
        task: task_id,
        state,
        evaluation,
    }
}

#[tokio::test]
#[ignore = "requires postgres"]
async fn execution_verifies_progress_and_acceptance_is_durable_and_idempotent() {
    let f = fixture(true).await;
    let item = f.state.get_work_item(f.item).await.unwrap().unwrap();
    assert_eq!(item.status, WorkItemStatus::VerifiedComplete);
    let reports = f.evaluation.list(f.project, f.item).await.unwrap();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].outcome, EvaluationOutcome::Passed);
    assert!(
        reports[0]
            .checks
            .iter()
            .all(|c| c.outcome == EvaluationOutcome::Passed)
    );
    assert_eq!(
        f.state
            .get_progress_report(f.project)
            .await
            .unwrap()
            .completion_percent,
        100.0
    );
    let id = reports[0].evaluation_id;
    f.evaluation
        .accept(f.project, f.item, id, "operator")
        .await
        .unwrap();
    f.evaluation
        .accept(f.project, f.item, id, "operator")
        .await
        .unwrap();
    let restarted = EvaluationHarness::new(f.h.pool.clone());
    assert_eq!(
        restarted
            .get(f.project, f.item, id)
            .await
            .unwrap()
            .input_hash,
        reports[0].input_hash
    );
    assert_eq!(
        f.state.get_work_item(f.item).await.unwrap().unwrap().status,
        WorkItemStatus::Accepted
    );
    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM work_item_acceptances WHERE work_item_id=$1")
            .bind(f.item)
            .fetch_one(&f.h.pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
#[ignore = "requires postgres"]
async fn a_note_and_reported_completion_cannot_substitute_for_execution() {
    let f = fixture(true).await;
    sqlx::query("DELETE FROM work_item_tasks WHERE work_item_id=$1")
        .bind(f.item)
        .execute(&f.h.pool)
        .await
        .unwrap();
    f.state
        .set_work_item_status(f.item, WorkItemStatus::ReportedComplete)
        .await
        .unwrap();
    f.state
        .add_work_item_evidence(NewWorkItemEvidence {
            work_item_id: f.item,
            evidence_type: "note".into(),
            artifact_id: None,
            task_id: None,
            execution_id: None,
            note: Some("Agent says done".into()),
        })
        .await
        .unwrap();
    let report = f.evaluation.evaluate(f.project, f.item).await.unwrap();
    assert_eq!(report.outcome, EvaluationOutcome::Indeterminate);
    assert!(
        f.evaluation
            .accept(f.project, f.item, report.evaluation_id, "operator")
            .await
            .is_err()
    );
    assert_eq!(
        f.state
            .get_progress_report(f.project)
            .await
            .unwrap()
            .completion_percent,
        0.0
    );
}

#[tokio::test]
#[ignore = "requires postgres"]
async fn checkpoint_without_independent_test_evidence_is_indeterminate() {
    let f = fixture(false).await;
    let report = f
        .evaluation
        .list(f.project, f.item)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(report.outcome, EvaluationOutcome::Indeterminate);
    assert!(
        !f.state
            .get_work_item(f.item)
            .await
            .unwrap()
            .unwrap()
            .status
            .is_complete()
    );
}

#[tokio::test]
#[ignore = "requires postgres"]
async fn missing_git_object_cannot_be_verified() {
    let f = fixture(true).await;
    let original = f
        .evaluation
        .list(f.project, f.item)
        .await
        .unwrap()
        .remove(0);
    let path = f.h.worktree_path_for(f.project, f.task).await;
    std::fs::rename(path.join(".git"), path.join(".git.hidden")).unwrap();
    assert!(
        f.evaluation
            .accept(f.project, f.item, original.evaluation_id, "operator")
            .await
            .is_err()
    );
    let report = f.evaluation.evaluate(f.project, f.item).await.unwrap();
    assert_eq!(report.outcome, EvaluationOutcome::Indeterminate);
    assert_eq!(
        f.state.get_work_item(f.item).await.unwrap().unwrap().status,
        WorkItemStatus::ReportedComplete
    );
}

#[tokio::test]
#[ignore = "requires postgres"]
async fn tampered_artifact_file_set_is_rejected() {
    let f = fixture(true).await;
    sqlx::query("UPDATE task_artifacts SET files_changed='[]' WHERE task_id=$1")
        .bind(f.task)
        .execute(&f.h.pool)
        .await
        .unwrap();
    assert_eq!(
        f.evaluation
            .evaluate(f.project, f.item)
            .await
            .unwrap()
            .outcome,
        EvaluationOutcome::Failed
    );
}

#[tokio::test]
#[ignore = "requires postgres"]
async fn stale_definition_of_done_and_new_tasks_invalidate_acceptance() {
    let f = fixture(true).await;
    let original = f
        .evaluation
        .list(f.project, f.item)
        .await
        .unwrap()
        .remove(0);
    sqlx::query(
        "UPDATE work_items SET definition_of_done='changed acceptance' WHERE work_item_id=$1",
    )
    .bind(f.item)
    .execute(&f.h.pool)
    .await
    .unwrap();
    assert!(
        f.evaluation
            .accept(f.project, f.item, original.evaluation_id, "operator")
            .await
            .is_err()
    );
    let fresh = f.evaluation.evaluate(f.project, f.item).await.unwrap();
    let next = TaskBuilder::new(f.project, "Additional work").build();
    f.h.scheduler.add_task_durable(next.clone()).await.unwrap();
    f.state
        .repository()
        .link_work_item_task(f.item, next.id)
        .await
        .unwrap();
    assert_eq!(
        f.state.get_work_item(f.item).await.unwrap().unwrap().status,
        WorkItemStatus::Active
    );
    assert!(
        f.evaluation
            .accept(f.project, f.item, fresh.evaluation_id, "operator")
            .await
            .is_err()
    );
    sqlx::query("DELETE FROM tasks WHERE id=$1")
        .bind(next.id)
        .execute(&f.h.pool)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires postgres"]
async fn project_scope_is_enforced_for_links_reports_and_artifacts() {
    let f = fixture(true).await;
    let other = insert_project(&f.h.pool, "foreign").await;
    let foreign = TaskBuilder::new(other, "foreign task").build();
    f.h.scheduler
        .add_task_durable(foreign.clone())
        .await
        .unwrap();
    assert!(
        f.state
            .repository()
            .link_work_item_task(f.item, foreign.id)
            .await
            .is_err()
    );
    assert!(f.evaluation.list(other, f.item).await.is_err());
    assert!(f.evaluation.evaluate(other, f.item).await.is_err());
    sqlx::query("DELETE FROM tasks WHERE id=$1")
        .bind(foreign.id)
        .execute(&f.h.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE task_artifacts SET project_id=$2 WHERE task_id=$1")
        .bind(f.task)
        .bind(other)
        .execute(&f.h.pool)
        .await
        .unwrap();
    assert_eq!(
        f.evaluation
            .evaluate(f.project, f.item)
            .await
            .unwrap()
            .outcome,
        EvaluationOutcome::Failed
    );
}

#[tokio::test]
#[ignore = "requires postgres"]
async fn additional_required_evaluator_failure_is_persisted_and_blocks_progress() {
    struct Reject;
    #[async_trait]
    impl Evaluator for Reject {
        fn name(&self) -> &'static str {
            "review"
        }
        async fn evaluate(&self, _: &EvaluationContext) -> EvaluationCheck {
            EvaluationCheck {
                evaluator: self.name().into(),
                outcome: EvaluationOutcome::Failed,
                summary: "Review rejected".into(),
                artifact_ids: vec![],
            }
        }
    }
    let f = fixture(true).await;
    let harness = EvaluationHarness::new(f.h.pool.clone()).with_evaluator(Arc::new(Reject));
    let report = harness.evaluate(f.project, f.item).await.unwrap();
    assert_eq!(report.outcome, EvaluationOutcome::Failed);
    assert_eq!(
        harness
            .get(f.project, f.item, report.evaluation_id)
            .await
            .unwrap()
            .checks
            .len(),
        3
    );
    assert_eq!(
        f.state
            .get_progress_report(f.project)
            .await
            .unwrap()
            .completion_percent,
        0.0
    );
}

#[tokio::test]
#[ignore = "requires postgres"]
async fn late_persistence_failure_rolls_back_report_and_work_item_promotion() {
    let f = fixture(true).await;
    f.state
        .set_work_item_status(f.item, WorkItemStatus::ReportedComplete)
        .await
        .unwrap();
    let before = f.evaluation.list(f.project, f.item).await.unwrap().len();
    // A disposable-DB fault after report insertion and status promotion, at
    // activity persistence. The constraint does not validate historical rows.
    let constraint = format!("evaluation_fault_{}", f.item.simple());
    let statement = format!(
        "ALTER TABLE project_activities ADD CONSTRAINT {constraint} CHECK (entity_id IS DISTINCT FROM '{}'::uuid OR activity_type <> 'work_item_verified') NOT VALID",
        f.item
    );
    sqlx::query(&statement).execute(&f.h.pool).await.unwrap();
    let result = f.evaluation.evaluate(f.project, f.item).await;
    sqlx::query(&format!(
        "ALTER TABLE project_activities DROP CONSTRAINT {constraint}"
    ))
    .execute(&f.h.pool)
    .await
    .unwrap();
    assert!(result.is_err());
    assert_eq!(
        f.evaluation.list(f.project, f.item).await.unwrap().len(),
        before
    );
    assert_eq!(
        f.state.get_work_item(f.item).await.unwrap().unwrap().status,
        WorkItemStatus::ReportedComplete
    );
    assert_eq!(
        f.state
            .get_progress_report(f.project)
            .await
            .unwrap()
            .completion_percent,
        0.0
    );
}

#[tokio::test]
#[ignore = "requires postgres and spawns the argus daemon"]
async fn daemon_http_verification_enforces_roles_and_reports_real_operational_data() {
    use std::process::{Child, Command, Stdio};
    struct Daemon(Child);
    impl Drop for Daemon {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let f = fixture(true).await;
    let port = e2e_support::free_port();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let _daemon = Daemon(
        Command::new(env!("CARGO_BIN_EXE_argus"))
            .arg("run")
            .current_dir(root)
            .env("ARGUS_DATABASE_URL", e2e_support::database_url())
            .env("ARGUS_MASTER_KEY", e2e_support::MASTER_KEY)
            .env("ARGUS_ACTOR_ROLE", "admin")
            .env("ARGUS_HTTP_ADMIN_TOKEN", e2e_support::MASTER_KEY)
            .env(
                "ARGUS_HTTP_DEVELOPER_TOKEN",
                "developer-token-for-testing-00000000",
            )
            .env(
                "ARGUS_HTTP_VIEWER_TOKEN",
                "viewer-token-for-testing-00000000000",
            )
            .env("ARGUS_SERVER_HOST", "127.0.0.1")
            .env("ARGUS_SERVER_PORT", port.to_string())
            .env("ARGUS_WORKSPACE_ROOT", &f.h.canonical_root)
            .env("ARGUS_WORKTREE_ROOT", f.h.scratch_path().join("worktrees"))
            // Any recovered unrelated test task also uses a zero-cost local script.
            .env(
                "ARGUS_AGENT_BINARY_OVERRIDE",
                f.h.scratch_path().join("agent.sh"),
            )
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let base = format!("http://127.0.0.1:{port}");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        if client
            .get(format!("{base}/api/status"))
            .send()
            .await
            .is_ok_and(|r| r.status().is_success())
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "daemon did not start"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let path = format!("{base}/api/projects/{}/work-items/{}", f.project, f.item);
    assert_eq!(
        client
            .post(format!("{path}/evaluate"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        client
            .post(format!("{path}/evaluate"))
            .bearer_auth("viewer-token-for-testing-00000000000")
            .header("x-argus-role", "admin")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let response = client
        .post(format!("{path}/evaluate"))
        .bearer_auth("developer-token-for-testing-00000000")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let report: argus_core::evaluation::EvaluationReport = response.json().await.unwrap();
    assert_eq!(report.outcome, EvaluationOutcome::Passed);
    let body = serde_json::json!({"evaluation_id":report.evaluation_id});
    assert_eq!(
        client
            .post(format!("{path}/accept"))
            .bearer_auth("developer-token-for-testing-00000000")
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        client
            .post(format!("{path}/accept"))
            .bearer_auth(e2e_support::MASTER_KEY)
            .json(&serde_json::json!({"evaluation_id":report.evaluation_id,"actor":"forged"}))
            .send()
            .await
            .unwrap()
            .status(),
        422
    );
    assert_eq!(
        client
            .post(format!("{path}/accept"))
            .bearer_auth(e2e_support::MASTER_KEY)
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    let (actor,): (String,) = sqlx::query_as(
        "SELECT actor FROM work_item_acceptances WHERE work_item_id=$1 AND evaluation_id=$2",
    )
    .bind(f.item)
    .bind(report.evaluation_id)
    .fetch_one(&f.h.pool)
    .await
    .unwrap();
    assert_eq!(actor, "http:admin");
    let config: serde_json::Value = client
        .get(format!("{base}/api/config"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(config["server"]["port"], port);
    assert_eq!(config["database"]["url"], "[redacted]");
    assert_eq!(
        client
            .put(format!("{base}/api/config"))
            .bearer_auth(e2e_support::MASTER_KEY)
            .json(&serde_json::json!({}))
            .send()
            .await
            .unwrap()
            .status(),
        501
    );
    let logs: serde_json::Value = client
        .get(format!("{base}/api/logs?source=audit&level=error"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(logs.as_array().unwrap().iter().any(|entry| {
        entry["message"]
            .as_str()
            .is_some_and(|message| message.contains("/api/config"))
    }));
    assert!(!logs.to_string().contains("Sample log entry"));
}
