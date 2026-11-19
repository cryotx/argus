//! V0.40 Roadmap & Progress Engine — end-to-end tests against real
//! PostgreSQL. No LLM, no agent, no paid execution.
//!
//! ```bash
//! ARGUS_DATABASE_URL=postgres://argus:argus@localhost:5432/argus \
//!   cargo test -p argus-cli --test roadmap_progress_e2e -- --ignored --test-threads=1
//! ```

mod e2e_support;

use std::sync::Arc;

use uuid::Uuid;

use argus_core::project_state::{
    ImprovementPriority, ImprovementStatus, MilestoneStatus, NewImprovement, NewMilestone,
    NewPhase, NewProject, NewRisk, NewWorkItem, NewWorkItemEvidence,
    PostgresProjectStateRepository, ProjectStateService, RiskLikelihood, RiskSeverity, RiskStatus,
    WorkItemStatus,
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
        description: format!("{prefix} roadmap test"),
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
    phase_id: Option<Uuid>,
    title: &str,
    status: WorkItemStatus,
) -> Uuid {
    let w = svc
        .create_work_item(NewWorkItem {
            project_id,
            milestone_id: Some(milestone_id),
            phase_id,
            title: title.to_string(),
            description: format!("{title} description"),
            priority: Priority::Normal,
            definition_of_done: Some(format!("{title} done")),
            source: Some("e2e".to_string()),
            external_key: None,
            weight: 1,
        })
        .await
        .expect("create work item");
    if status != WorkItemStatus::Planned {
        svc.set_work_item_status(w.work_item_id, status)
            .await
            .expect("set status");
    }
    w.work_item_id
}

async fn add_note_evidence(svc: &ProjectStateService, work_item_id: Uuid) {
    svc.add_work_item_evidence(NewWorkItemEvidence {
        work_item_id,
        evidence_type: "note".to_string(),
        artifact_id: None,
        task_id: None,
        execution_id: None,
        note: Some("verified in test".to_string()),
    })
    .await
    .expect("add evidence");
}

async fn verify(svc: &ProjectStateService, work_item_id: Uuid) {
    add_note_evidence(svc, work_item_id).await;
    svc.set_work_item_status(work_item_id, WorkItemStatus::VerifiedComplete)
        .await
        .expect("verify");
}

// ============================================================
// Test 1 — completion is evidence-driven, not self-reported
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn reported_complete_does_not_count_as_progress() {
    let pool = connect().await;
    let svc = service(&pool);
    let project_id = new_project(&svc, "rp-evidence").await;
    let milestone = active_milestone(&svc, project_id, "Evidence Milestone").await;
    let item = work_item(
        &svc,
        project_id,
        milestone,
        None,
        "Needs evidence",
        WorkItemStatus::Active,
    )
    .await;

    svc.set_work_item_status(item, WorkItemStatus::ReportedComplete)
        .await
        .unwrap();
    let before = svc.get_roadmap_progress(project_id).await.unwrap();
    assert_eq!(
        before.overall.verified_items, 0,
        "a self-reported completion is not verified"
    );
    assert_eq!(before.overall.reported_complete_items, 1);
    assert_eq!(before.overall.completion_ratio, 0.0);
    assert!(!before.overall.is_complete());

    // Evidence is required to verify a WorkItem that has a DoD.
    let premature = svc
        .set_work_item_status(item, WorkItemStatus::VerifiedComplete)
        .await;
    assert!(premature.is_err(), "no evidence -> no verification");

    add_note_evidence(&svc, item).await;
    svc.set_work_item_status(item, WorkItemStatus::VerifiedComplete)
        .await
        .unwrap();

    let after = svc.get_roadmap_progress(project_id).await.unwrap();
    assert_eq!(after.overall.verified_items, 1);
    assert_eq!(after.overall.reported_complete_items, 0);
    assert!(after.overall.completion_ratio > before.overall.completion_ratio);
    assert_eq!(after.overall.completion_ratio, 1.0);

    pool.close().await;
}

// ============================================================
// Test 2 — deferred items do not pollute the denominator
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn deferred_items_are_excluded_from_denominator() {
    let pool = connect().await;
    let svc = service(&pool);
    let project_id = new_project(&svc, "rp-deferred").await;
    let milestone = active_milestone(&svc, project_id, "Deferred Milestone").await;

    let a = work_item(
        &svc,
        project_id,
        milestone,
        None,
        "A",
        WorkItemStatus::Planned,
    )
    .await;
    verify(&svc, a).await;
    let _b = work_item(
        &svc,
        project_id,
        milestone,
        None,
        "B",
        WorkItemStatus::Planned,
    )
    .await;
    let c = work_item(
        &svc,
        project_id,
        milestone,
        None,
        "C",
        WorkItemStatus::Planned,
    )
    .await;
    svc.set_work_item_status(c, WorkItemStatus::Deferred)
        .await
        .unwrap();

    let progress = svc.get_roadmap_progress(project_id).await.unwrap();
    let ms = progress
        .milestones
        .iter()
        .find(|m| m.milestone_id == milestone)
        .unwrap();
    assert_eq!(ms.summary.total_items, 2, "A + B are the denominator");
    assert_eq!(ms.summary.verified_items, 1);
    assert_eq!(ms.summary.deferred_items, 1);
    assert_eq!(ms.summary.completion_ratio, 0.5);
    assert_eq!(progress.overall.completion_ratio, 0.5);

    pool.close().await;
}

// ============================================================
// Test 3 — parent aggregation (phase -> milestone -> project)
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn phase_progress_aggregates_into_milestone() {
    let pool = connect().await;
    let svc = service(&pool);
    let project_id = new_project(&svc, "rp-parent").await;
    let milestone = active_milestone(&svc, project_id, "Aggregation Milestone").await;
    let phase = svc
        .create_phase(NewPhase {
            project_id,
            milestone_id: milestone,
            title: "Aggregation Phase".to_string(),
            description: None,
            sequence: None,
            external_key: None,
            definition_of_done: None,
            weight: 1,
        })
        .await
        .unwrap();
    svc.set_phase_status(phase.phase_id, MilestoneStatus::Active)
        .await
        .unwrap();

    for title in ["Work 1", "Work 2"] {
        let id = work_item(
            &svc,
            project_id,
            milestone,
            Some(phase.phase_id),
            title,
            WorkItemStatus::Planned,
        )
        .await;
        verify(&svc, id).await;
    }
    let _work3 = work_item(
        &svc,
        project_id,
        milestone,
        Some(phase.phase_id),
        "Work 3",
        WorkItemStatus::Planned,
    )
    .await;

    let progress = svc.get_roadmap_progress(project_id).await.unwrap();
    let ms = progress
        .milestones
        .iter()
        .find(|m| m.milestone_id == milestone)
        .unwrap();
    assert_eq!(ms.phases.len(), 1);
    assert_eq!(ms.phases[0].summary.total_items, 3);
    assert_eq!(ms.phases[0].summary.verified_items, 2);
    assert!((ms.phases[0].summary.completion_ratio - 2.0 / 3.0).abs() < 1e-9);
    assert!((ms.summary.completion_ratio - 2.0 / 3.0).abs() < 1e-9);
    assert!((progress.overall.completion_ratio - 2.0 / 3.0).abs() < 1e-9);

    pool.close().await;
}

// ============================================================
// Test 4 — weights
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn weights_change_progress() {
    let pool = connect().await;
    let svc = service(&pool);
    let project_id = new_project(&svc, "rp-weight").await;
    let milestone = active_milestone(&svc, project_id, "Weight Milestone").await;
    let phase = svc
        .create_phase(NewPhase {
            project_id,
            milestone_id: milestone,
            title: "Weight Phase".to_string(),
            description: None,
            sequence: None,
            external_key: None,
            definition_of_done: None,
            weight: 1,
        })
        .await
        .unwrap();

    let light = work_item(
        &svc,
        project_id,
        milestone,
        Some(phase.phase_id),
        "Light",
        WorkItemStatus::Planned,
    )
    .await;
    verify(&svc, light).await;
    let heavy = svc
        .create_work_item(NewWorkItem {
            project_id,
            milestone_id: Some(milestone),
            phase_id: Some(phase.phase_id),
            title: "Heavy".to_string(),
            description: "weighted".to_string(),
            priority: Priority::High,
            definition_of_done: Some("heavy done".to_string()),
            source: None,
            external_key: None,
            weight: 3,
        })
        .await
        .unwrap()
        .work_item_id;
    assert_eq!(svc.get_work_item(heavy).await.unwrap().unwrap().weight, 3);

    let progress = svc.get_roadmap_progress(project_id).await.unwrap();
    let ms = progress
        .milestones
        .iter()
        .find(|m| m.milestone_id == milestone)
        .unwrap();
    // 1 completed weight out of 1 + 3 = 0.25.
    assert!((ms.summary.completion_ratio - 0.25).abs() < 1e-9);

    pool.close().await;
}

// ============================================================
// Test 5 — roadmap dependencies gate next candidates
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn dependencies_gate_next_candidates() {
    let pool = connect().await;
    let svc = service(&pool);
    let project_id = new_project(&svc, "rp-deps").await;
    let milestone = active_milestone(&svc, project_id, "Dependency Milestone").await;

    let a = work_item(
        &svc,
        project_id,
        milestone,
        None,
        "Dependency A",
        WorkItemStatus::Planned,
    )
    .await;
    let b = work_item(
        &svc,
        project_id,
        milestone,
        None,
        "Dependent B",
        WorkItemStatus::Planned,
    )
    .await;
    svc.add_work_item_dependency(b, a).await.unwrap();

    let progress = svc.get_roadmap_progress(project_id).await.unwrap();
    let titles: Vec<&str> = progress
        .next_candidates
        .iter()
        .map(|w| w.title.as_str())
        .collect();
    assert!(titles.contains(&"Dependency A"), "A is ready: {titles:?}");
    assert!(
        !titles.contains(&"Dependent B"),
        "B is not ready: {titles:?}"
    );

    verify(&svc, a).await;
    let progress = svc.get_roadmap_progress(project_id).await.unwrap();
    let titles: Vec<&str> = progress
        .next_candidates
        .iter()
        .map(|w| w.title.as_str())
        .collect();
    assert!(
        titles.contains(&"Dependent B"),
        "B becomes ready once A is verified: {titles:?}"
    );

    // The dependency is roadmap metadata, not a runtime task DAG edge.
    let deps = svc.list_work_item_dependencies(project_id).await.unwrap();
    assert_eq!(deps.len(), 1);
    assert_eq!(deps[0].work_item_id, b);
    assert_eq!(deps[0].depends_on_id, a);

    pool.close().await;
}

// ============================================================
// Test 6 — determinism + restart persistence
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn progress_is_deterministic_and_survives_restart() {
    let pool = connect().await;
    let svc = service(&pool);
    let project_id = new_project(&svc, "rp-determinism").await;
    let milestone = active_milestone(&svc, project_id, "Determinism Milestone").await;
    let a = work_item(
        &svc,
        project_id,
        milestone,
        None,
        "Det A",
        WorkItemStatus::Planned,
    )
    .await;
    verify(&svc, a).await;
    let _b = work_item(
        &svc,
        project_id,
        milestone,
        None,
        "Det B",
        WorkItemStatus::Planned,
    )
    .await;

    let first = svc.get_roadmap_progress(project_id).await.unwrap();
    let second = svc.get_roadmap_progress(project_id).await.unwrap();
    assert_eq!(
        serde_json::to_value(&first).unwrap(),
        serde_json::to_value(&second).unwrap(),
        "same DB state must produce identical progress, including updated_at"
    );

    // New pool + repository + service (simulated daemon restart).
    let pool2 = connect().await;
    let svc2 = service(&pool2);
    let after_restart = svc2.get_roadmap_progress(project_id).await.unwrap();
    assert_eq!(
        serde_json::to_value(&first).unwrap(),
        serde_json::to_value(&after_restart).unwrap(),
        "progress must be identical after restart"
    );

    pool.close().await;
    pool2.close().await;
}

// ============================================================
// Test 7 — progress diff
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn progress_diff_reports_completed_started_and_resolved() {
    let pool = connect().await;
    let svc = service(&pool);
    let project_id = new_project(&svc, "rp-diff").await;
    let milestone = active_milestone(&svc, project_id, "Diff Milestone").await;

    let x = work_item(
        &svc,
        project_id,
        milestone,
        None,
        "Diff X",
        WorkItemStatus::Planned,
    )
    .await;
    let risk = svc
        .create_risk(NewRisk {
            project_id,
            milestone_id: None,
            title: "Diff risk".to_string(),
            description: None,
            severity: RiskSeverity::Medium,
            likelihood: RiskLikelihood::Low,
            mitigation: None,
            owner: None,
        })
        .await
        .unwrap();

    // Baseline.
    svc.record_progress_snapshot(project_id).await.unwrap();

    // Mutations after the baseline.
    verify(&svc, x).await;
    let y = work_item(
        &svc,
        project_id,
        milestone,
        None,
        "Diff Y",
        WorkItemStatus::Planned,
    )
    .await;
    svc.set_work_item_status(y, WorkItemStatus::Active)
        .await
        .unwrap();
    svc.set_risk_status(risk.risk_id, RiskStatus::Resolved)
        .await
        .unwrap();
    svc.create_improvement(NewImprovement {
        project_id,
        external_key: Some("DEF-DIFF".to_string()),
        title: "Newly deferred".to_string(),
        description: None,
        priority: ImprovementPriority::Low,
        status: ImprovementStatus::Deferred,
        source: None,
        why_deferred: None,
        required_before: None,
    })
    .await
    .unwrap();

    let diff = svc.get_progress_diff(project_id).await.unwrap();
    assert!(diff.before_overall < diff.after_overall);
    assert!(
        diff.completed_added.iter().any(|s| s.contains("Diff X")),
        "completed_added: {:?}",
        diff.completed_added
    );
    assert!(
        diff.started_added.iter().any(|s| s.contains("Diff Y")),
        "started_added: {:?}",
        diff.started_added
    );
    assert!(
        diff.risks_resolved.iter().any(|s| s == "Diff risk"),
        "risks_resolved: {:?}",
        diff.risks_resolved
    );
    assert!(
        diff.deferred_added.iter().any(|s| s.contains("DEF-DIFF")),
        "deferred_added: {:?}",
        diff.deferred_added
    );
    assert!(
        diff.progress_changes
            .iter()
            .any(|c| c.after_ratio > c.before_ratio),
        "progress_changes: {:?}",
        diff.progress_changes
    );

    pool.close().await;
}

// ============================================================
// Test 8 — multi-project isolation of progress and candidates
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn progress_is_isolated_per_project() {
    let pool = connect().await;
    let svc = service(&pool);
    let argus_project = new_project(&svc, "rp-iso-argus").await;
    let geo_project = new_project(&svc, "rp-iso-geo").await;

    let argus_ms = active_milestone(&svc, argus_project, "ARGUS milestone").await;
    let geo_ms = active_milestone(&svc, geo_project, "GEO milestone").await;
    let argus_item = work_item(
        &svc,
        argus_project,
        argus_ms,
        None,
        "ARGUS work",
        WorkItemStatus::Planned,
    )
    .await;
    verify(&svc, argus_item).await;
    let _geo_item = work_item(
        &svc,
        geo_project,
        geo_ms,
        None,
        "GEO work",
        WorkItemStatus::Planned,
    )
    .await;

    let argus_progress = svc.get_roadmap_progress(argus_project).await.unwrap();
    let geo_progress = svc.get_roadmap_progress(geo_project).await.unwrap();

    assert_eq!(argus_progress.overall.completion_ratio, 1.0);
    assert_eq!(geo_progress.overall.completion_ratio, 0.0);
    assert!(
        argus_progress
            .milestones
            .iter()
            .all(|m| m.title == "ARGUS milestone")
    );
    assert!(
        geo_progress
            .milestones
            .iter()
            .all(|m| m.title == "GEO milestone")
    );
    assert!(
        geo_progress
            .next_candidates
            .iter()
            .all(|w| w.title == "GEO work")
    );
    assert!(
        argus_progress
            .milestones
            .iter()
            .flat_map(|m| m.work_items.iter())
            .all(|w| w.title != "GEO work")
    );

    // Diff is likewise isolated.
    let argus_diff = svc.get_progress_diff(argus_project).await.unwrap();
    assert!(
        argus_diff
            .completed_added
            .iter()
            .any(|s| s.contains("ARGUS work"))
    );
    assert!(
        !argus_diff
            .completed_added
            .iter()
            .any(|s| s.contains("GEO work"))
    );

    pool.close().await;
}
