//! V0.41 Context Compiler — end-to-end tests against real PostgreSQL. No LLM,
//! no agent, no paid execution.
//!
//! ```bash
//! ARGUS_DATABASE_URL=postgres://argus:argus@localhost:5432/argus \
//!   cargo test -p argus-cli --test context_compiler_e2e -- --ignored --test-threads=1
//! ```

mod e2e_support;

use std::sync::Arc;

use uuid::Uuid;

use argus_core::context::{
    CONTEXT_COMPILER_VERSION, ContextPurpose, ContextRequest, ContextRole, ContextSourceType,
    render_json, render_markdown,
};
use argus_core::project_state::{
    MilestoneStatus, NewCharter, NewDecision, NewImprovement, NewMilestone, NewProject, NewRisk,
    NewWorkItem, NewWorkItemEvidence, PostgresProjectStateRepository, ProjectStateService,
    RiskLikelihood, RiskSeverity, WorkItemStatus,
};
use argus_core::task_types::Priority;

use e2e_support::connect;

fn service(pool: &sqlx::PgPool) -> ProjectStateService {
    ProjectStateService::new(Arc::new(PostgresProjectStateRepository::new(pool.clone())))
}

async fn new_project(svc: &ProjectStateService, prefix: &str) -> Uuid {
    let slug = format!("{prefix}-{}", &Uuid::new_v4().to_string()[..8]);
    svc.create_project_state(NewProject {
        name: format!("{prefix} {}", &slug),
        slug,
        description: format!("{prefix} context test"),
        priority: Priority::Normal,
    })
    .await
    .expect("create project")
    .project_id
}

async fn active_milestone(svc: &ProjectStateService, project_id: Uuid, title: &str) -> Uuid {
    let m = svc
        .create_milestone(NewMilestone {
            project_id,
            title: title.to_string(),
            description: None,
            sequence: None,
            parent_id: None,
            definition_of_done: None,
            external_key: None,
            weight: 1,
        })
        .await
        .expect("create milestone");
    svc.set_milestone_status(m.milestone_id, MilestoneStatus::Active)
        .await
        .expect("activate milestone");
    m.milestone_id
}

async fn work_item(
    svc: &ProjectStateService,
    project_id: Uuid,
    milestone_id: Uuid,
    title: &str,
) -> Uuid {
    svc.create_work_item(NewWorkItem {
        project_id,
        milestone_id: Some(milestone_id),
        phase_id: None,
        title: title.to_string(),
        description: format!("{title} description"),
        priority: Priority::Normal,
        definition_of_done: Some(format!("{title} done")),
        source: Some("e2e".to_string()),
        external_key: None,
        weight: 1,
    })
    .await
    .expect("create work item")
    .work_item_id
}

async fn insert_task(pool: &sqlx::PgPool, project_id: Uuid) -> Uuid {
    let task_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO tasks (id, project_id, title, task_type) VALUES ($1, $2, 'ctx task', 'generic')",
    )
    .bind(task_id)
    .bind(project_id)
    .execute(pool)
    .await
    .expect("insert task");
    task_id
}

async fn insert_artifact(pool: &sqlx::PgPool, project_id: Uuid, commit_sha: &str) -> (Uuid, Uuid) {
    let task_id = insert_task(pool, project_id).await;
    let workspace_id = Uuid::new_v4();
    let execution_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO task_workspaces (workspace_id, project_id, task_id, execution_id, \
         repository_path, worktree_path, branch_name, base_revision, status) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, 'base', 'completed')",
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(task_id)
    .bind(execution_id)
    .bind(format!("/repo/{workspace_id}"))
    .bind(format!("/worktree/{workspace_id}"))
    .bind(format!("argus/task/{workspace_id}"))
    .execute(pool)
    .await
    .expect("insert workspace");

    let artifact_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO task_artifacts (artifact_id, artifact_type, project_id, task_id, execution_id, \
         workspace_id, base_revision, branch_name, commit_sha, files_changed, quality_status) \
         VALUES ($1, 'git_commit', $2, $3, $4, $5, 'base', 'argus/task/x', $6, $7, 'passed')",
    )
    .bind(artifact_id)
    .bind(project_id)
    .bind(task_id)
    .bind(execution_id)
    .bind(workspace_id)
    .bind(commit_sha)
    .bind(serde_json::json!(["src/lib.rs", "checkpoint_smoke.txt"]))
    .execute(pool)
    .await
    .expect("insert artifact");
    (artifact_id, task_id)
}

// ============================================================
// Test 1 — ARGUS's real self-context packet
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn argus_real_context_packet() {
    let pool = connect().await;
    let svc = service(&pool);
    let project = svc.bootstrap_argus().await.unwrap();

    let request = ContextRequest::new(
        project.project_id,
        ContextRole::ProjectManager,
        ContextPurpose::Planning,
    );
    let packet = svc.compile_context(&request).await.unwrap();

    assert_eq!(packet.compiler_version, CONTEXT_COMPILER_VERSION);
    assert!(!packet.context_hash.is_empty());
    assert_eq!(packet.project_identity.name, "ARGUS");
    assert_eq!(
        packet.current_milestone.as_ref().unwrap().title,
        "V0.43 Planner Harness / Protocol"
    );
    assert!(
        packet
            .roadmap_context
            .as_ref()
            .unwrap()
            .milestones
            .iter()
            .any(|m| m.title == "V0.38 Reliable Agent Delivery Runtime" && m.status == "completed")
    );
    assert!(
        packet
            .active_work_items
            .iter()
            .any(|w| w.title == "Planner Harness / Protocol")
    );
    let keys: Vec<&str> = packet
        .deferred_improvements
        .iter()
        .filter_map(|i| i.external_key.as_deref())
        .collect();
    assert!(keys.contains(&"DEF-010"), "{keys:?}");
    assert!(keys.contains(&"DEF-011"), "{keys:?}");
    assert!(keys.contains(&"DEF-012"), "{keys:?}");

    let md = render_markdown(&packet);
    for needle in [
        "Project: ARGUS",
        "Current",
        "V0.43 Planner Harness / Protocol",
        "V0.38",
        "V0.39",
        "V0.40",
        "V0.41",
        "V0.42",
        "Active Work",
        "Planner Harness / Protocol",
        "Deferred Improvements",
        "DEF-010",
        "DEF-011",
        "DEF-012",
        "Relevant Decisions",
        "Recent Artifacts",
        "Risks",
        "Blockers",
        "Provenance",
    ] {
        assert!(md.contains(needle), "markdown missing {needle:?}:\n{md}");
    }

    pool.close().await;
}

// ============================================================
// Test 2 — Developer is scoped to their own WorkItem
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn developer_context_is_isolated_to_their_work_item() {
    let pool = connect().await;
    let svc = service(&pool);
    let project_id = new_project(&svc, "ctx-dev").await;
    svc.save_charter(NewCharter {
        project_id,
        vision: "v".into(),
        goal: "g".into(),
        success_criteria: vec![],
        constraints: vec!["no production writes".into()],
        non_goals: vec![],
    })
    .await
    .unwrap();
    let milestone = active_milestone(&svc, project_id, "Dev Milestone").await;
    let a = work_item(&svc, project_id, milestone, "WorkItem A").await;
    let b = work_item(&svc, project_id, milestone, "WorkItem B").await;
    svc.set_work_item_status(a, WorkItemStatus::Active)
        .await
        .unwrap();
    svc.add_work_item_dependency(a, b).await.unwrap();

    let task = insert_task(&pool, project_id).await;
    svc.link_work_item_task(a, task).await.unwrap();
    svc.add_work_item_evidence(NewWorkItemEvidence {
        work_item_id: a,
        evidence_type: "task".to_string(),
        artifact_id: None,
        task_id: Some(task),
        execution_id: None,
        note: Some("task evidence".into()),
    })
    .await
    .unwrap();

    let decision = svc
        .record_decision(NewDecision {
            project_id,
            title: "Use Postgres".to_string(),
            context: None,
            decision: "store state in postgres".into(),
            reason: None,
            status: argus_core::project_state::DecisionStatus::Accepted,
            source: None,
        })
        .await
        .unwrap();
    // Noise that a Developer must NOT get by default.
    svc.create_improvement(NewImprovement {
        project_id,
        external_key: Some("DEF-DEV".into()),
        title: "deferred backlog item".into(),
        description: None,
        priority: argus_core::project_state::ImprovementPriority::Low,
        status: argus_core::project_state::ImprovementStatus::Deferred,
        source: None,
        why_deferred: None,
        required_before: None,
    })
    .await
    .unwrap();
    for i in 0..12 {
        svc.record_activity(argus_core::project_state::NewActivity {
            project_id,
            activity_type: argus_core::project_state::ActivityType::WorkItemCreated,
            entity_type: "work_item".into(),
            entity_id: None,
            summary: format!("noise {i}"),
            metadata: serde_json::json!({}),
        })
        .await
        .unwrap();
    }

    let request = ContextRequest::new(
        project_id,
        ContextRole::Developer,
        ContextPurpose::Execution,
    )
    .with_work_item(a);
    let packet = svc.compile_context(&request).await.unwrap();

    let work = packet
        .current_work_item
        .as_ref()
        .expect("current work item");
    assert_eq!(work.work_item_id, a);
    assert_eq!(work.definition_of_done.as_deref(), Some("WorkItem A done"));
    assert!(!work.evidence.is_empty(), "A's evidence must be included");
    assert_eq!(work.depends_on, vec![b]);
    assert!(
        packet
            .dependencies
            .iter()
            .any(|d| d.depends_on_id == b && !d.satisfied)
    );
    assert!(
        packet
            .relevant_decisions
            .iter()
            .any(|d| d.decision_id == decision.decision_id),
        "the accepted decision is relevant"
    );
    assert!(
        packet
            .constraints
            .iter()
            .any(|c| c.text == "no production writes"),
        "charter constraints apply"
    );

    // Narrow: no full roadmap, no other work items, no deferred backlog.
    assert!(
        packet.roadmap_context.is_none(),
        "Developer must not get the roadmap"
    );
    assert!(
        packet.active_work_items.is_empty() && packet.blocked_work_items.is_empty(),
        "Developer must not get the whole work-item list"
    );
    assert!(
        packet.deferred_improvements.is_empty(),
        "Developer must not get the deferred backlog"
    );
    assert!(
        packet.recent_activity.len() <= 5,
        "activity must be bounded"
    );

    // B's full description is only reachable via the dependency reference.
    for dep in &packet.dependencies {
        assert!(dep.depends_on_title.as_deref() == Some("WorkItem B"));
    }

    pool.close().await;
}

// ============================================================
// Test 3 — ProjectManager gets the broad state
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn project_manager_context_has_broad_state() {
    let pool = connect().await;
    let svc = service(&pool);
    let project_id = new_project(&svc, "ctx-pm").await;
    svc.save_charter(NewCharter {
        project_id,
        vision: "vision".into(),
        goal: "goal".into(),
        success_criteria: vec!["done".into()],
        constraints: vec!["constraint".into()],
        non_goals: vec!["non goal".into()],
    })
    .await
    .unwrap();
    let milestone = active_milestone(&svc, project_id, "PM Milestone").await;
    let active = work_item(&svc, project_id, milestone, "Active work").await;
    svc.set_work_item_status(active, WorkItemStatus::Active)
        .await
        .unwrap();
    let blocked = work_item(&svc, project_id, milestone, "Blocked work").await;
    svc.set_work_item_status(blocked, WorkItemStatus::Blocked)
        .await
        .unwrap();
    svc.create_risk(NewRisk {
        project_id,
        milestone_id: None,
        title: "High risk".into(),
        description: None,
        severity: RiskSeverity::High,
        likelihood: RiskLikelihood::Medium,
        mitigation: None,
        owner: None,
    })
    .await
    .unwrap();
    svc.record_decision(NewDecision {
        project_id,
        title: "Accepted decision".into(),
        context: None,
        decision: "do it".into(),
        reason: None,
        status: argus_core::project_state::DecisionStatus::Accepted,
        source: None,
    })
    .await
    .unwrap();
    svc.create_improvement(NewImprovement {
        project_id,
        external_key: Some("DEF-PM".into()),
        title: "deferred".into(),
        description: None,
        priority: argus_core::project_state::ImprovementPriority::Low,
        status: argus_core::project_state::ImprovementStatus::Deferred,
        source: None,
        why_deferred: None,
        required_before: None,
    })
    .await
    .unwrap();
    let (artifact_id, _) = insert_artifact(&pool, project_id, "abcdef1234567890").await;
    let _ = artifact_id;
    svc.record_activity(argus_core::project_state::NewActivity {
        project_id,
        activity_type: argus_core::project_state::ActivityType::ProjectCreated,
        entity_type: "project".into(),
        entity_id: None,
        summary: "recent".into(),
        metadata: serde_json::json!({}),
    })
    .await
    .unwrap();

    let request = ContextRequest::new(
        project_id,
        ContextRole::ProjectManager,
        ContextPurpose::Planning,
    );
    let packet = svc.compile_context(&request).await.unwrap();

    assert!(packet.charter.is_some());
    assert!(packet.current_milestone.is_some());
    assert!(packet.roadmap_context.is_some());
    assert!(
        packet
            .active_work_items
            .iter()
            .any(|w| w.title == "Active work")
    );
    assert!(
        packet
            .blocked_work_items
            .iter()
            .any(|w| w.title == "Blocked work")
    );
    assert!(packet.relevant_risks.iter().any(|r| r.title == "High risk"));
    assert!(
        packet
            .relevant_decisions
            .iter()
            .any(|d| d.title == "Accepted decision")
    );
    assert!(
        packet
            .deferred_improvements
            .iter()
            .any(|i| i.external_key.as_deref() == Some("DEF-PM"))
    );
    assert!(!packet.relevant_artifacts.is_empty(), "recent artifacts");
    assert!(!packet.recent_activity.is_empty(), "recent activity");

    pool.close().await;
}

// ============================================================
// Test 4 — Reviewer sees the artifact and evidence
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn reviewer_context_sees_artifact_and_dod() {
    let pool = connect().await;
    let svc = service(&pool);
    let project_id = new_project(&svc, "ctx-review").await;
    let milestone = active_milestone(&svc, project_id, "Review Milestone").await;
    let w = work_item(&svc, project_id, milestone, "Review target").await;
    svc.set_work_item_status(w, WorkItemStatus::Active)
        .await
        .unwrap();
    let (artifact_id, task_id) = insert_artifact(&pool, project_id, "0123456789abcdef").await;
    svc.add_work_item_evidence(NewWorkItemEvidence {
        work_item_id: w,
        evidence_type: "artifact".to_string(),
        artifact_id: Some(artifact_id),
        task_id: Some(task_id),
        execution_id: None,
        note: Some("checkpoint".into()),
    })
    .await
    .unwrap();

    let request = ContextRequest::new(project_id, ContextRole::Reviewer, ContextPurpose::Review)
        .with_work_item(w);
    let packet = svc.compile_context(&request).await.unwrap();

    let work = packet.current_work_item.as_ref().unwrap();
    assert_eq!(work.work_item_id, w);
    assert!(work.definition_of_done.is_some());
    assert_eq!(work.evidence.len(), 1);
    let artifact = packet
        .relevant_artifacts
        .iter()
        .find(|a| a.artifact_id == artifact_id)
        .expect("reviewer must see the artifact");
    assert_eq!(artifact.commit_sha, "0123456789abcdef");
    assert_eq!(artifact.quality_status.as_deref(), Some("passed"));
    assert_eq!(artifact.files_changed.len(), 2);
    // Narrow: no full roadmap for a reviewer.
    assert!(packet.roadmap_context.is_none());

    pool.close().await;
}

// ============================================================
// Test 5 — superseded decisions are not active context
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn superseded_decision_is_excluded() {
    let pool = connect().await;
    let svc = service(&pool);
    let project_id = new_project(&svc, "ctx-super").await;

    let old = svc
        .record_decision(NewDecision {
            project_id,
            title: "Decision A".into(),
            context: None,
            decision: "old approach".into(),
            reason: None,
            status: argus_core::project_state::DecisionStatus::Accepted,
            source: None,
        })
        .await
        .unwrap();
    let new = svc
        .record_decision(NewDecision {
            project_id,
            title: "Decision B".into(),
            context: None,
            decision: "new approach".into(),
            reason: None,
            status: argus_core::project_state::DecisionStatus::Accepted,
            source: None,
        })
        .await
        .unwrap();
    svc.supersede_decision(old.decision_id, new.decision_id)
        .await
        .unwrap();

    let request = ContextRequest::new(
        project_id,
        ContextRole::ProjectManager,
        ContextPurpose::Planning,
    );
    let packet = svc.compile_context(&request).await.unwrap();
    assert!(
        packet
            .relevant_decisions
            .iter()
            .any(|d| d.decision_id == new.decision_id),
        "new decision must be present"
    );
    assert!(
        packet
            .relevant_decisions
            .iter()
            .all(|d| d.decision_id != old.decision_id),
        "superseded decision must not be active context"
    );

    pool.close().await;
}

// ============================================================
// Test 6 — budget truncation is bounded and deterministic
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn budget_truncation_is_bounded_and_deterministic() {
    let pool = connect().await;
    let svc = service(&pool);
    let project_id = new_project(&svc, "ctx-budget").await;
    let milestone = active_milestone(&svc, project_id, "Budget Milestone").await;
    let w = work_item(&svc, project_id, milestone, "Budget WorkItem").await;
    svc.set_work_item_status(w, WorkItemStatus::Active)
        .await
        .unwrap();

    for i in 0..100 {
        svc.record_activity(argus_core::project_state::NewActivity {
            project_id,
            activity_type: argus_core::project_state::ActivityType::WorkItemCreated,
            entity_type: "work_item".into(),
            entity_id: None,
            summary: format!("activity {i}"),
            metadata: serde_json::json!({}),
        })
        .await
        .unwrap();
    }
    for i in 0..30 {
        svc.record_decision(NewDecision {
            project_id,
            title: format!("Decision {i}"),
            context: None,
            decision: format!("decision body {i}"),
            reason: None,
            status: argus_core::project_state::DecisionStatus::Accepted,
            source: None,
        })
        .await
        .unwrap();
    }
    for i in 0..50 {
        insert_artifact(&pool, project_id, &format!("{i:040}")).await;
    }

    let request = ContextRequest::new(
        project_id,
        ContextRole::ProjectManager,
        ContextPurpose::Planning,
    )
    .with_work_item(w)
    .with_budget(20, 6_000);

    let packet = svc.compile_context(&request).await.unwrap();
    assert!(packet.truncation.truncated, "budget must force truncation");
    assert!(
        packet.recent_activity.len() <= 20,
        "activity must be bounded"
    );
    assert!(
        packet.current_work_item.is_some(),
        "current work item is protected"
    );
    assert!(
        packet.estimated_size.estimated_bytes <= 6_000 + 2_000,
        "size should be near budget: {}",
        packet.estimated_size.estimated_bytes
    );
    assert!(
        !packet.truncation.omitted_item_counts.is_empty()
            || !packet.truncation.omitted_sections.is_empty()
    );

    let again = svc.compile_context(&request).await.unwrap();
    assert_eq!(packet.context_hash, again.context_hash);
    assert_eq!(render_markdown(&packet), render_markdown(&again));

    pool.close().await;
}

// ============================================================
// Test 7 — determinism + restart
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn same_state_same_hash_across_restart() {
    let pool = connect().await;
    let svc = service(&pool);
    let project_id = new_project(&svc, "ctx-det").await;
    let milestone = active_milestone(&svc, project_id, "Det Milestone").await;
    let w = work_item(&svc, project_id, milestone, "Det WorkItem").await;
    svc.set_work_item_status(w, WorkItemStatus::Active)
        .await
        .unwrap();

    let request = ContextRequest::new(
        project_id,
        ContextRole::Developer,
        ContextPurpose::Execution,
    )
    .with_work_item(w);
    let first = svc.compile_context(&request).await.unwrap();
    let second = svc.compile_context(&request).await.unwrap();
    assert_eq!(first.context_hash, second.context_hash);
    assert_ne!(
        first.packet_id, second.packet_id,
        "packet id is identity, not hash"
    );
    assert_eq!(render_markdown(&first), render_markdown(&second));
    // JSON differs only by identity/timestamp fields.
    let j1: serde_json::Value = serde_json::from_str(&render_json(&first)).unwrap();
    let j2: serde_json::Value = serde_json::from_str(&render_json(&second)).unwrap();
    assert_eq!(j1["context_hash"], j2["context_hash"]);

    let pool2 = connect().await;
    let svc2 = service(&pool2);
    let after = svc2.compile_context(&request).await.unwrap();
    assert_eq!(
        first.context_hash, after.context_hash,
        "restart must not change the hash"
    );

    pool.close().await;
    pool2.close().await;
}

// ============================================================
// Test 8 — project isolation
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn context_is_isolated_per_project() {
    let pool = connect().await;
    let svc = service(&pool);
    let argus = new_project(&svc, "ctx-iso-argus").await;
    let geo = new_project(&svc, "ctx-iso-geo").await;

    let argus_ms = active_milestone(&svc, argus, "ARGUS-only milestone").await;
    let a = work_item(&svc, argus, argus_ms, "ARGUS-only work").await;
    svc.record_decision(NewDecision {
        project_id: argus,
        title: "ARGUS-only decision".into(),
        context: None,
        decision: "argus".into(),
        reason: None,
        status: argus_core::project_state::DecisionStatus::Accepted,
        source: None,
    })
    .await
    .unwrap();

    let geo_ms = active_milestone(&svc, geo, "GEO-only milestone").await;
    let _g = work_item(&svc, geo, geo_ms, "GEO-only work").await;
    svc.record_decision(NewDecision {
        project_id: geo,
        title: "GEO-only decision".into(),
        context: None,
        decision: "geo".into(),
        reason: None,
        status: argus_core::project_state::DecisionStatus::Accepted,
        source: None,
    })
    .await
    .unwrap();
    let geo_risk = svc
        .create_risk(NewRisk {
            project_id: geo,
            milestone_id: None,
            title: "GEO-only risk".into(),
            description: None,
            severity: RiskSeverity::Critical,
            likelihood: RiskLikelihood::High,
            mitigation: None,
            owner: None,
        })
        .await
        .unwrap();

    let request = ContextRequest::new(argus, ContextRole::ProjectManager, ContextPurpose::Planning)
        .with_work_item(a);
    let packet = svc.compile_context(&request).await.unwrap();

    assert_eq!(packet.project_id, argus);
    assert!(
        packet
            .relevant_decisions
            .iter()
            .all(|d| d.title != "GEO-only decision")
    );
    assert!(
        packet
            .relevant_risks
            .iter()
            .all(|r| r.risk_id != geo_risk.risk_id)
    );
    assert!(
        packet
            .active_work_items
            .iter()
            .all(|w| w.title != "GEO-only work")
    );
    assert!(
        packet
            .roadmap_context
            .as_ref()
            .unwrap()
            .milestones
            .iter()
            .all(|m| m.title != "GEO-only milestone")
    );
    // No source ref may point at another project's entities by id.
    assert!(
        packet.source_refs.iter().all(|r| {
            r.source_type != ContextSourceType::Risk || r.source_id != geo_risk.risk_id
        })
    );
    let _ = geo;

    pool.close().await;
}

// ============================================================
// Test 9 — provenance
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn every_fact_has_a_source_ref() {
    let pool = connect().await;
    let svc = service(&pool);
    let project_id = new_project(&svc, "ctx-prov").await;
    let milestone = active_milestone(&svc, project_id, "Prov Milestone").await;
    let w = work_item(&svc, project_id, milestone, "Prov WorkItem").await;
    svc.set_work_item_status(w, WorkItemStatus::Active)
        .await
        .unwrap();
    let decision = svc
        .record_decision(NewDecision {
            project_id,
            title: "Prov decision".into(),
            context: None,
            decision: "d".into(),
            reason: None,
            status: argus_core::project_state::DecisionStatus::Accepted,
            source: None,
        })
        .await
        .unwrap();
    let risk = svc
        .create_risk(NewRisk {
            project_id,
            milestone_id: None,
            title: "Prov risk".into(),
            description: None,
            severity: RiskSeverity::High,
            likelihood: RiskLikelihood::Low,
            mitigation: None,
            owner: None,
        })
        .await
        .unwrap();
    let (artifact_id, task_id) = insert_artifact(&pool, project_id, "ffffffffffffffff").await;
    svc.add_work_item_evidence(NewWorkItemEvidence {
        work_item_id: w,
        evidence_type: "artifact".into(),
        artifact_id: Some(artifact_id),
        task_id: Some(task_id),
        execution_id: None,
        note: None,
    })
    .await
    .unwrap();

    let request = ContextRequest::new(
        project_id,
        ContextRole::ProjectManager,
        ContextPurpose::Planning,
    )
    .with_work_item(w);
    let packet = svc.compile_context(&request).await.unwrap();

    let has = |ty: ContextSourceType, id: Uuid| {
        packet
            .source_refs
            .iter()
            .any(|r| r.source_type == ty && r.source_id == id)
    };
    assert!(has(ContextSourceType::Project, project_id));
    assert!(has(ContextSourceType::WorkItem, w));
    assert!(has(ContextSourceType::Decision, decision.decision_id));
    assert!(has(ContextSourceType::Risk, risk.risk_id));
    assert!(has(ContextSourceType::Artifact, artifact_id));
    assert!(has(ContextSourceType::Milestone, milestone));

    pool.close().await;
}

// ============================================================
// Test 10 — Markdown is not runtime truth
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn markdown_is_not_runtime_truth() {
    let pool = connect().await;
    let svc = service(&pool);
    let project_id = new_project(&svc, "ctx-md").await;
    let milestone = active_milestone(&svc, project_id, "MD Milestone").await;
    let _ = milestone;

    let request = ContextRequest::new(
        project_id,
        ContextRole::ProjectManager,
        ContextPurpose::StatusReview,
    );
    let before = svc.compile_context(&request).await.unwrap();

    // A conflicting Markdown file must not influence the packet. (No DB write
    // is involved, so the structural hash must be identical.)
    let md_path = std::env::temp_dir().join(format!("argus-ctx-{}.md", Uuid::new_v4()));
    std::fs::write(&md_path, "# Roadmap\n\nCurrent: V9.99\n").unwrap();

    let after = svc.compile_context(&request).await.unwrap();
    assert_eq!(
        before.context_hash, after.context_hash,
        "context must depend only on structured Project State"
    );
    assert_eq!(
        after.current_milestone.as_ref().unwrap().title,
        "MD Milestone"
    );
    assert!(!render_markdown(&after).contains("V9.99"));

    let _ = std::fs::remove_file(&md_path);
    pool.close().await;
}
