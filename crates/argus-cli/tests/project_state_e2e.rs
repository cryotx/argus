//! V0.39 Project State foundation — end-to-end tests against a real
//! PostgreSQL. No LLM, no agent, no paid execution.
//!
//! ```bash
//! ARGUS_DATABASE_URL=postgres://argus:argus@localhost:5432/argus \
//!   cargo test -p argus-cli --test project_state_e2e -- --ignored --test-threads=1
//! ```

mod e2e_support;

use std::sync::Arc;

use uuid::Uuid;

use argus_core::project_state::{
    ActivityType, BlockerStatus, ImprovementPriority, ImprovementStatus, MilestoneStatus,
    NewBlocker, NewCharter, NewDecision, NewImprovement, NewMilestone, NewProject, NewRisk,
    NewWorkItem, NewWorkItemEvidence, PostgresProjectStateRepository, ProjectStateService,
    RiskLikelihood, RiskSeverity, RiskStatus, WorkItemStatus,
};
use argus_core::task_types::Priority;

use e2e_support::connect;

fn service(pool: &sqlx::PgPool) -> ProjectStateService {
    ProjectStateService::new(Arc::new(PostgresProjectStateRepository::new(pool.clone())))
}

fn unique_slug(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::new_v4().to_string()[..8])
}

async fn new_project(
    svc: &ProjectStateService,
    prefix: &str,
) -> argus_core::project_state::ProjectRecord {
    let slug = unique_slug(prefix);
    svc.create_project_state(NewProject {
        name: format!("{prefix} {}", &slug),
        slug,
        description: format!("{prefix} e2e project"),
        priority: Priority::Normal,
    })
    .await
    .expect("create project")
}

async fn insert_task(pool: &sqlx::PgPool, project_id: Uuid) -> Uuid {
    let task_id = Uuid::new_v4();
    sqlx::query("INSERT INTO tasks (id, project_id, title, task_type) VALUES ($1, $2, 'e2e task', 'generic')")
        .bind(task_id)
        .bind(project_id)
        .execute(pool)
        .await
        .expect("insert task");
    task_id
}

// ============================================================
// Test 1 — full CRUD lifecycle + snapshot aggregation
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn crud_lifecycle_persists_and_snapshots() {
    let pool = connect().await;
    let svc = service(&pool);
    let project = new_project(&svc, "ps-crud").await;
    let project_id = project.project_id;

    // --- Charter: revision only bumps when content changes ---
    let mk_charter = |vision: &str| NewCharter {
        project_id,
        vision: vision.to_string(),
        goal: "Ship the foundation".to_string(),
        success_criteria: vec!["deterministic snapshot".to_string()],
        constraints: vec!["no LLM".to_string()],
        non_goals: vec!["vector memory".to_string()],
    };
    let c1 = svc.save_charter(mk_charter("v1")).await.unwrap();
    assert_eq!(c1.revision, 1);
    let c1_again = svc.save_charter(mk_charter("v1")).await.unwrap();
    assert_eq!(c1_again.revision, 1, "no-op save must not bump revision");
    let c2 = svc.save_charter(mk_charter("v2")).await.unwrap();
    assert_eq!(c2.revision, 2, "changed content must bump revision");

    // --- Milestone create -> activate -> complete ---
    let milestone = svc
        .create_milestone(NewMilestone {
            project_id,
            title: "M Foundation".to_string(),
            description: Some("foundation".to_string()),
            sequence: None,
            parent_id: None,
            definition_of_done: Some("done".to_string()),

            external_key: None,
            weight: 1,
        })
        .await
        .unwrap();
    assert_eq!(milestone.status, MilestoneStatus::Planned);
    svc.set_milestone_status(milestone.milestone_id, MilestoneStatus::Active)
        .await
        .unwrap();
    let m = svc
        .get_milestone(milestone.milestone_id)
        .await
        .unwrap()
        .unwrap();
    assert!(m.started_at.is_some());
    svc.set_milestone_status(milestone.milestone_id, MilestoneStatus::Completed)
        .await
        .unwrap();
    let m = svc
        .get_milestone(milestone.milestone_id)
        .await
        .unwrap()
        .unwrap();
    assert!(m.completed_at.is_some());

    // A separate Active milestone is what a snapshot reports as "current"
    // (a project with only Completed milestones has no current milestone).
    let current = svc
        .create_milestone(NewMilestone {
            project_id,
            title: "M Current".to_string(),
            description: None,
            sequence: None,
            parent_id: None,
            definition_of_done: None,

            external_key: None,
            weight: 1,
        })
        .await
        .unwrap();
    svc.set_milestone_status(current.milestone_id, MilestoneStatus::Active)
        .await
        .unwrap();

    // --- WorkItem: ReportedComplete is not VerifiedComplete ---
    let work_item = svc
        .create_work_item(NewWorkItem {
            project_id,
            milestone_id: Some(milestone.milestone_id),
            title: "W Foundation".to_string(),
            description: "persist state".to_string(),
            priority: Priority::High,
            definition_of_done: Some("state persists".to_string()),
            source: Some("e2e".to_string()),

            phase_id: None,
            external_key: None,
            weight: 1,
        })
        .await
        .unwrap();
    assert_eq!(work_item.status, WorkItemStatus::Planned);
    svc.set_work_item_status(work_item.work_item_id, WorkItemStatus::Active)
        .await
        .unwrap();
    svc.set_work_item_status(work_item.work_item_id, WorkItemStatus::ReportedComplete)
        .await
        .unwrap();
    let w = svc
        .get_work_item(work_item.work_item_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        w.status,
        WorkItemStatus::ReportedComplete,
        "an Agent claim alone is not verification"
    );
    // Verifying before any evidence must be refused (this WorkItem has a DoD).
    let premature = svc
        .set_work_item_status(work_item.work_item_id, WorkItemStatus::VerifiedComplete)
        .await;
    assert!(
        premature.is_err(),
        "verification without evidence must be rejected"
    );

    // --- WorkItem <-> Task (0..N) and evidence ---
    let task_a = insert_task(&pool, project_id).await;
    let task_b = insert_task(&pool, project_id).await;
    svc.link_work_item_task(work_item.work_item_id, task_a)
        .await
        .unwrap();
    svc.link_work_item_task(work_item.work_item_id, task_b)
        .await
        .unwrap();
    let linked = svc
        .list_work_item_tasks(work_item.work_item_id)
        .await
        .unwrap();
    assert_eq!(linked.len(), 2, "one work item may be backed by many tasks");

    let evidence = svc
        .add_work_item_evidence(NewWorkItemEvidence {
            work_item_id: work_item.work_item_id,
            evidence_type: "task".to_string(),
            artifact_id: None,
            task_id: Some(task_a),
            execution_id: Some(Uuid::new_v4()),
            note: Some("task completed".to_string()),
        })
        .await
        .unwrap();
    assert_eq!(evidence.work_item_id, work_item.work_item_id);
    let evidence_list = svc
        .list_work_item_evidence(work_item.work_item_id)
        .await
        .unwrap();
    assert_eq!(evidence_list.len(), 1);

    // With evidence attached, verification is allowed.
    svc.set_work_item_status(work_item.work_item_id, WorkItemStatus::VerifiedComplete)
        .await
        .unwrap();

    // Artifact evidence with no artifact_id must be rejected.
    let bad = svc
        .add_work_item_evidence(NewWorkItemEvidence {
            work_item_id: work_item.work_item_id,
            evidence_type: "artifact".to_string(),
            artifact_id: None,
            task_id: None,
            execution_id: None,
            note: None,
        })
        .await;
    assert!(bad.is_err());

    // --- Decision: accepted -> superseded (never overwritten) ---
    let d1 = svc
        .record_decision(NewDecision {
            project_id,
            title: "Use Postgres".to_string(),
            context: None,
            decision: "Store state in Postgres".to_string(),
            reason: Some("durability".to_string()),
            status: argus_core::project_state::DecisionStatus::Accepted,
            source: Some("e2e".to_string()),
        })
        .await
        .unwrap();
    let d2 = svc
        .record_decision(NewDecision {
            project_id,
            title: "Add artifact evidence".to_string(),
            context: None,
            decision: "Back work items with artifacts".to_string(),
            reason: None,
            status: argus_core::project_state::DecisionStatus::Accepted,
            source: Some("e2e".to_string()),
        })
        .await
        .unwrap();
    svc.supersede_decision(d1.decision_id, d2.decision_id)
        .await
        .unwrap();
    let decisions = svc.list_decisions(project_id).await.unwrap();
    let old = decisions
        .iter()
        .find(|d| d.decision_id == d1.decision_id)
        .unwrap();
    assert_eq!(
        old.status,
        argus_core::project_state::DecisionStatus::Superseded
    );
    assert_eq!(old.superseded_by, Some(d2.decision_id));

    // --- Risk open -> resolved ---
    let risk = svc
        .create_risk(NewRisk {
            project_id,
            milestone_id: None,
            title: "Schema drift".to_string(),
            description: None,
            severity: RiskSeverity::High,
            likelihood: RiskLikelihood::Medium,
            mitigation: Some("tests".to_string()),
            owner: None,
        })
        .await
        .unwrap();
    assert_eq!(risk.status, RiskStatus::Open);
    svc.set_risk_status(risk.risk_id, RiskStatus::Resolved)
        .await
        .unwrap();
    let risks = svc.list_risks(project_id).await.unwrap();
    let r = risks.iter().find(|r| r.risk_id == risk.risk_id).unwrap();
    assert_eq!(r.status, RiskStatus::Resolved);
    assert!(r.resolved_at.is_some());

    // --- Blocker open -> resolved ---
    let blocker = svc
        .create_blocker(NewBlocker {
            project_id,
            work_item_id: Some(work_item.work_item_id),
            title: "Waiting on migration".to_string(),
            description: None,
        })
        .await
        .unwrap();
    assert_eq!(blocker.status, BlockerStatus::Open);
    svc.resolve_blocker(blocker.blocker_id).await.unwrap();
    let blockers = svc.list_blockers(project_id).await.unwrap();
    let b = blockers
        .iter()
        .find(|b| b.blocker_id == blocker.blocker_id)
        .unwrap();
    assert_eq!(b.status, BlockerStatus::Resolved);
    assert!(b.resolved_at.is_some());

    // --- Deferred improvement create/list/status ---
    let improvement = svc
        .create_improvement(NewImprovement {
            project_id,
            external_key: Some("DEF-TEST".to_string()),
            title: "Test improvement".to_string(),
            description: None,
            priority: ImprovementPriority::Medium,
            status: ImprovementStatus::Deferred,
            source: Some("e2e".to_string()),
            why_deferred: Some("later".to_string()),
            required_before: Some("next phase".to_string()),
        })
        .await
        .unwrap();
    svc.set_improvement_status(improvement.improvement_id, ImprovementStatus::InProgress)
        .await
        .unwrap();
    let improvements = svc.list_improvements(project_id).await.unwrap();
    assert_eq!(improvements.len(), 1);
    assert_eq!(improvements[0].status, ImprovementStatus::InProgress);

    // --- Snapshot aggregation ---
    let snapshot = svc.get_project_snapshot(project_id).await.unwrap();
    assert_eq!(snapshot.project.project_id, project_id);
    assert_eq!(snapshot.charter.as_ref().unwrap().revision, 2);
    assert_eq!(
        snapshot.current_milestone.as_ref().unwrap().milestone_id,
        current.milestone_id
    );
    assert_eq!(snapshot.deferred_count, 0, "improvement is now in_progress");
    assert!(
        snapshot
            .recent_activity
            .iter()
            .any(|a| a.activity_type == ActivityType::DecisionAccepted)
    );
    assert!(
        snapshot
            .recent_activity
            .iter()
            .any(|a| a.activity_type == ActivityType::BlockerOpened)
    );

    // Deterministic status rendering.
    let text = svc.render_status(project_id).await.unwrap();
    assert!(text.contains("M Foundation"), "got:\n{text}");

    // Pause/resume transitions are recorded.
    svc.set_project_status(project_id, argus_core::project_state::ProjectStatus::Paused)
        .await
        .unwrap();
    svc.set_project_status(project_id, argus_core::project_state::ProjectStatus::Active)
        .await
        .unwrap();
    let after = svc.get_project_snapshot(project_id).await.unwrap();
    assert!(
        after
            .recent_activity
            .iter()
            .any(|a| a.activity_type == ActivityType::ProjectPaused)
            && after
                .recent_activity
                .iter()
                .any(|a| a.activity_type == ActivityType::ProjectResumed)
    );

    pool.close().await;
}

// ============================================================
// Test 2 — restart persistence (new service instance, same DB)
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn restart_persistence_survives_new_service() {
    let pool = connect().await;
    let svc = service(&pool);
    let project = new_project(&svc, "ps-restart").await;
    let project_id = project.project_id;

    let milestone = svc
        .create_milestone(NewMilestone {
            project_id,
            title: "Restart Milestone".to_string(),
            description: None,
            sequence: None,
            parent_id: None,
            definition_of_done: None,

            external_key: None,
            weight: 1,
        })
        .await
        .unwrap();
    svc.set_milestone_status(milestone.milestone_id, MilestoneStatus::Active)
        .await
        .unwrap();
    let work = svc
        .create_work_item(NewWorkItem {
            project_id,
            milestone_id: Some(milestone.milestone_id),
            title: "Restart WorkItem".to_string(),
            description: String::new(),
            priority: Priority::High,
            definition_of_done: None,
            source: None,

            phase_id: None,
            external_key: None,
            weight: 1,
        })
        .await
        .unwrap();
    svc.set_work_item_status(work.work_item_id, WorkItemStatus::Active)
        .await
        .unwrap();
    let _decision = svc
        .record_decision(NewDecision {
            project_id,
            title: "Restart Decision".to_string(),
            context: None,
            decision: "survive restart".to_string(),
            reason: None,
            status: argus_core::project_state::DecisionStatus::Accepted,
            source: None,
        })
        .await
        .unwrap();
    let _risk = svc
        .create_risk(NewRisk {
            project_id,
            milestone_id: None,
            title: "Restart Risk".to_string(),
            description: None,
            severity: RiskSeverity::Low,
            likelihood: RiskLikelihood::Low,
            mitigation: None,
            owner: None,
        })
        .await
        .unwrap();
    let before = svc.get_project_snapshot(project_id).await.unwrap();

    // Simulate a daemon restart: brand-new pool + repository + service.
    let pool2 = connect().await;
    let svc2 = service(&pool2);
    let after = svc2.get_project_snapshot(project_id).await.unwrap();

    assert_eq!(after.project.slug, before.project.slug);
    assert_eq!(after.current_milestone.unwrap().title, "Restart Milestone");
    assert_eq!(after.active_work_items.len(), 1);
    assert!(after.open_risks.iter().any(|r| r.title == "Restart Risk"));

    pool.close().await;
    pool2.close().await;
}

// ============================================================
// Test 3 — multi-project isolation
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn multi_project_data_is_isolated() {
    let pool = connect().await;
    let svc = service(&pool);
    let argus = new_project(&svc, "ps-iso-argus").await;
    let geo = new_project(&svc, "ps-iso-geo").await;

    svc.create_milestone(NewMilestone {
        project_id: argus.project_id,
        title: "ARGUS-only milestone".to_string(),
        description: None,
        sequence: None,
        parent_id: None,
        definition_of_done: None,

        external_key: None,
        weight: 1,
    })
    .await
    .unwrap();
    svc.create_work_item(NewWorkItem {
        project_id: geo.project_id,
        milestone_id: None,
        title: "GEO-only work item".to_string(),
        description: String::new(),
        priority: Priority::Normal,
        definition_of_done: None,
        source: None,

        phase_id: None,
        external_key: None,
        weight: 1,
    })
    .await
    .unwrap();
    svc.create_risk(NewRisk {
        project_id: geo.project_id,
        milestone_id: None,
        title: "GEO-only risk".to_string(),
        description: None,
        severity: RiskSeverity::Medium,
        likelihood: RiskLikelihood::Medium,
        mitigation: None,
        owner: None,
    })
    .await
    .unwrap();

    let argus_snapshot = svc.get_project_snapshot(argus.project_id).await.unwrap();
    let geo_snapshot = svc.get_project_snapshot(geo.project_id).await.unwrap();

    assert!(
        argus_snapshot
            .milestones
            .iter()
            .all(|m| m.title != "GEO-only work item")
    );
    assert!(
        argus_snapshot
            .milestones
            .iter()
            .any(|m| m.title == "ARGUS-only milestone")
    );
    assert!(
        argus_snapshot
            .active_work_items
            .iter()
            .all(|w| w.title != "GEO-only work item"),
        "ARGUS snapshot must never contain GEO data"
    );
    assert!(
        argus_snapshot
            .open_risks
            .iter()
            .all(|r| r.title != "GEO-only risk")
    );
    assert!(
        geo_snapshot
            .open_risks
            .iter()
            .any(|r| r.title == "GEO-only risk")
    );
    assert!(
        geo_snapshot
            .milestones
            .iter()
            .all(|m| m.title != "ARGUS-only milestone")
    );

    pool.close().await;
}

// ============================================================
// Test 4 — database is the source of truth; Markdown/metadata is not
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn database_beats_markdown_and_metadata() {
    let pool = connect().await;
    let svc = service(&pool);
    let project = new_project(&svc, "ps-sot").await;
    let project_id = project.project_id;

    // DB says the current milestone is V0.39.
    svc.create_milestone(NewMilestone {
        project_id,
        title: "V0.39 Project State / Memory".to_string(),
        description: None,
        sequence: Some(1),
        parent_id: None,
        definition_of_done: None,

        external_key: None,
        weight: 1,
    })
    .await
    .unwrap();
    let v039 = svc.list_milestones(project_id).await.unwrap()[0].milestone_id;
    svc.set_milestone_status(v039, MilestoneStatus::Active)
        .await
        .unwrap();

    // A conflicting Markdown file and a conflicting metadata blob both claim
    // V0.38 is current. Neither may influence the snapshot.
    let md_path = std::env::temp_dir().join(format!("argus-sot-{}.md", Uuid::new_v4()));
    std::fs::write(&md_path, "# Project State\n\nCurrent: V0.38\n").unwrap();
    sqlx::query("UPDATE projects SET metadata = $2 WHERE id = $1")
        .bind(project_id)
        .bind(serde_json::json!({ "current_milestone": "V0.38" }))
        .execute(&pool)
        .await
        .unwrap();

    let snapshot = svc.get_project_snapshot(project_id).await.unwrap();
    assert_eq!(
        snapshot.current_milestone.unwrap().title,
        "V0.39 Project State / Memory",
        "the snapshot must come from the DB, not Markdown or metadata"
    );
    let text = svc.render_status(project_id).await.unwrap();
    assert!(text.contains("V0.39 Project State / Memory"));
    assert!(!text.contains("Current Milestone:\nV0.38"));

    let _ = std::fs::remove_file(&md_path);
    pool.close().await;
}

// ============================================================
// Test 5 — ARGUS bootstraps its own Project State
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn bootstrap_argus_reports_roadmap_and_progress() {
    let pool = connect().await;
    let svc = service(&pool);

    let project = svc.bootstrap_argus().await.unwrap();
    assert_eq!(project.slug, "argus");
    assert_eq!(project.name, "ARGUS");

    // Idempotent: a second bootstrap must not duplicate state.
    let milestones_before = svc.list_milestones(project.project_id).await.unwrap().len();
    let improvements_before = svc
        .list_improvements(project.project_id)
        .await
        .unwrap()
        .len();
    svc.bootstrap_argus().await.unwrap();
    assert_eq!(
        svc.list_milestones(project.project_id).await.unwrap().len(),
        milestones_before
    );
    assert_eq!(
        svc.list_improvements(project.project_id)
            .await
            .unwrap()
            .len(),
        improvements_before
    );

    let snapshot = svc.get_project_snapshot(project.project_id).await.unwrap();

    let current = snapshot.current_milestone.expect("current milestone");
    assert_eq!(current.title, "V0.43 Planner Harness / Protocol");
    assert_eq!(current.status, MilestoneStatus::Active);

    for (title, status) in [
        (
            "V0.38 Reliable Agent Delivery Runtime",
            MilestoneStatus::Completed,
        ),
        ("V0.39 Project State / Memory", MilestoneStatus::Completed),
        (
            "V0.40 Roadmap / Progress Engine",
            MilestoneStatus::Completed,
        ),
        ("V0.41 Context Compiler", MilestoneStatus::Completed),
        (
            "V0.42 Task Intelligence / Routing",
            MilestoneStatus::Completed,
        ),
        ("V0.43 Planner Harness / Protocol", MilestoneStatus::Active),
    ] {
        assert!(
            snapshot
                .milestones
                .iter()
                .any(|m| m.title == title && m.status == status),
            "expected milestone {title} as {status:?}; got {:?}",
            snapshot
                .milestones
                .iter()
                .map(|m| (m.title.clone(), m.status))
                .collect::<Vec<_>>()
        );
    }

    // V0.39's foundation is now VerifiedComplete (with evidence), and V0.40's
    // work items are the active ones.
    assert!(
        svc.list_work_items(project.project_id)
            .await
            .unwrap()
            .iter()
            .any(|w| w.title == "Project State Foundation"
                && w.status == WorkItemStatus::VerifiedComplete),
        "Project State Foundation must be VerifiedComplete"
    );
    assert!(
        svc.list_work_items(project.project_id)
            .await
            .unwrap()
            .iter()
            .any(|w| w.title == "Deterministic Progress Engine"
                && w.status == WorkItemStatus::VerifiedComplete),
        "Deterministic Progress Engine must be verified"
    );
    assert!(
        svc.list_work_items(project.project_id)
            .await
            .unwrap()
            .iter()
            .any(|w| w.title == "Context Compiler" && w.status == WorkItemStatus::VerifiedComplete),
        "Context Compiler must be verified"
    );
    assert!(
        svc.list_work_items(project.project_id)
            .await
            .unwrap()
            .iter()
            .any(|w| w.title == "Task Intelligence / Model Routing"
                && w.status == WorkItemStatus::VerifiedComplete),
        "Task Intelligence / Model Routing must be verified"
    );
    assert!(
        snapshot
            .active_work_items
            .iter()
            .any(|w| w.title == "Planner Harness / Protocol"),
        "Planner Harness / Protocol must be active"
    );

    let keys: Vec<&str> = snapshot
        .deferred_improvements
        .iter()
        .filter_map(|i| i.external_key.as_deref())
        .collect();
    assert!(keys.contains(&"DEF-010"), "keys: {keys:?}");
    assert!(keys.contains(&"DEF-011"), "keys: {keys:?}");
    assert!(keys.contains(&"DEF-012"), "keys: {keys:?}");

    // Deterministic progress rendering.
    let text = svc.render_status(project.project_id).await.unwrap();
    assert!(text.contains("Overall Verified Progress:"));
    assert!(text.contains("Current:\nV0.43 Planner Harness / Protocol"));
    assert!(text.contains("V0.38 Reliable Agent Delivery Runtime 100%"));
    assert!(text.contains("V0.39 Project State / Memory 100%"));
    assert!(text.contains("V0.40 Roadmap / Progress Engine 100%"));
    assert!(text.contains("V0.41 Context Compiler 100%"));
    assert!(text.contains("V0.42 Task Intelligence / Routing 100%"));
    assert!(text.contains("→ V0.43 Planner Harness / Protocol"));
    assert!(text.contains("DEF-010") && text.contains("DEF-011") && text.contains("DEF-012"));

    // Roadmap API model agrees.
    let roadmap = svc.get_roadmap_progress(project.project_id).await.unwrap();
    assert_eq!(
        roadmap.current_milestone.as_ref().unwrap().title,
        "V0.43 Planner Harness / Protocol"
    );
    assert!(
        roadmap
            .milestones
            .iter()
            .find(|m| m.title == "V0.38 Reliable Agent Delivery Runtime")
            .unwrap()
            .summary
            .completion_ratio
            >= 0.999
    );

    pool.close().await;
}

// ============================================================
// Test 6 — V0.38 artifact -> ProjectActivity hook
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn artifact_produced_activity_hook_records_structured_fact() {
    let pool = connect().await;
    let svc = service(&pool);
    let project = new_project(&svc, "ps-artifact").await;

    let artifact_id = Uuid::new_v4();
    let task_id = Uuid::new_v4();
    svc.record_artifact_produced(
        project.project_id,
        artifact_id,
        task_id,
        "c31292ae8cf0ef5e9e64c87197108bccda64e2e0",
        &["checkpoint_smoke.txt".to_string()],
    )
    .await
    .unwrap();

    let activity = svc.list_activity(project.project_id, 10).await.unwrap();
    let produced = activity
        .iter()
        .find(|a| a.activity_type == ActivityType::ArtifactProduced)
        .expect("ArtifactProduced activity");
    assert_eq!(produced.entity_id, Some(artifact_id));
    assert_eq!(
        produced.metadata.get("commit_sha").and_then(|v| v.as_str()),
        Some("c31292ae8cf0ef5e9e64c87197108bccda64e2e0")
    );

    pool.close().await;
}
