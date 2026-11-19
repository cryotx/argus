//! V0.43 Planner Harness / Protocol — end-to-end tests against real
//! PostgreSQL, using the deterministic FakePlannerAdapter. No LLM, no provider.

mod e2e_support;

use std::sync::Arc;

use uuid::Uuid;

use argus_core::context::{ContextPurpose, ContextRole};
use argus_core::intelligence::{PostgresTaskIntelligenceRepository, TaskIntelligenceService};
use argus_core::planner::{
    CreatePlanningRequestInput, FakePlannerAdapter, PlanProposal, PlannerHarness, PlanningKind,
    PlanningRepository, PlanningResult, PlanningStatus, PostgresPlanningRepository,
    ProposalOperation, ProposalStatus, ProposedPhase, ProposedWorkItem, RequestedOutput,
};
use argus_core::project_state::{
    MilestoneStatus, NewMilestone, NewProject, PostgresProjectStateRepository, ProjectStateService,
};
use argus_core::task_types::Priority;

use e2e_support::connect;

type Services = (
    ProjectStateService,
    TaskIntelligenceService,
    PlannerHarness,
    Arc<dyn PlanningRepository>,
);

fn services(pool: &sqlx::PgPool) -> Services {
    let state: Arc<dyn argus_core::project_state::ProjectStateRepository> =
        Arc::new(PostgresProjectStateRepository::new(pool.clone()));
    let intel_repo: Arc<dyn argus_core::intelligence::TaskIntelligenceRepository> =
        Arc::new(PostgresTaskIntelligenceRepository::new(pool.clone()));
    let planning_repo: Arc<dyn PlanningRepository> =
        Arc::new(PostgresPlanningRepository::new(pool.clone()));
    let intel = TaskIntelligenceService::new(state.clone(), intel_repo);
    let planner = PlannerHarness::new(
        state.clone(),
        planning_repo.clone(),
        Some(Arc::new(TaskIntelligenceService::new(
            state.clone(),
            Arc::new(PostgresTaskIntelligenceRepository::new(pool.clone())),
        ))),
    );
    (
        ProjectStateService::new(state),
        intel,
        planner,
        planning_repo,
    )
}

async fn new_project(state: &ProjectStateService, prefix: &str) -> Uuid {
    let slug = format!("{prefix}-{}", &Uuid::new_v4().to_string()[..8]);
    state
        .create_project_state(NewProject {
            name: format!("{prefix} {slug}"),
            slug,
            description: "planner test".to_string(),
            priority: Priority::Normal,
        })
        .await
        .unwrap()
        .project_id
}

async fn active_milestone(state: &ProjectStateService, project_id: Uuid, title: &str) -> Uuid {
    let m = state
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
        .unwrap();
    state
        .set_milestone_status(m.milestone_id, MilestoneStatus::Active)
        .await
        .unwrap();
    m.milestone_id
}

async fn create_request(
    planner: &PlannerHarness,
    project_id: Uuid,
    objective: &str,
) -> argus_core::planner::PlanningRequest {
    planner
        .create_planning_request(CreatePlanningRequestInput {
            project_id,
            planning_kind: PlanningKind::PhasePlanning,
            role: ContextRole::ProjectManager,
            purpose: ContextPurpose::Planning,
            objective: objective.to_string(),
            constraints: vec!["additive".to_string()],
            requested_outputs: vec![RequestedOutput::RoadmapChanges, RequestedOutput::WorkItems],
            work_item_id: None,
        })
        .await
        .unwrap()
}

fn create_work_item(
    title: &str,
    milestone_id: Uuid,
    external_key: Option<&str>,
) -> ProposedWorkItem {
    ProposedWorkItem {
        operation: ProposalOperation::Create,
        target_id: None,
        title: title.to_string(),
        description: Some(format!("{title} description")),
        definition_of_done: Some(format!("{title} done")),
        milestone_id: Some(milestone_id),
        phase_id: None,
        priority: Some(Priority::High),
        status: None,
        dependencies: vec![],
        external_key: external_key.map(str::to_string),
        weight: None,
        rationale: Some("planned by test planner".to_string()),
    }
}

// ============================================================
// Test 1 — full fake-planner chain: request -> validate -> diff -> apply
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn fake_planner_full_chain() {
    let pool = connect().await;
    let (state, _intel, planner, _repo) = services(&pool);
    let project_id = new_project(&state, "plan-full").await;
    let milestone = active_milestone(&state, project_id, "V0.43 Planner Harness / Protocol").await;

    let request = create_request(&planner, project_id, "Plan the remaining V0.43 work").await;
    assert!(request.request_hash.starts_with("sha256:"));
    assert!(request.request_hash.len() > 10);

    let proposal = PlanProposal {
        phase_changes: vec![ProposedPhase {
            operation: ProposalOperation::Create,
            target_id: None,
            milestone_id: Some(milestone),
            title: "Planner Protocol Foundation".to_string(),
            description: Some("protocol + harness".to_string()),
            sequence: None,
            external_key: None,
            definition_of_done: Some("protocol documented and tested".to_string()),
            status: None,
            rationale: Some("foundation for V0.43".to_string()),
        }],
        work_item_changes: vec![
            create_work_item(
                "PlanningRequest / Result protocol",
                milestone,
                Some("P43-001"),
            ),
            create_work_item("Proposal Validator", milestone, Some("P43-002")),
            create_work_item(
                "Manual Web Planner export/import",
                milestone,
                Some("P43-003"),
            ),
        ],
        ..Default::default()
    };
    let adapter = FakePlannerAdapter::new(PlanningStatus::Proposed, proposal)
        .with_summary("V0.43 foundation plan");

    let outcome = planner
        .run_adapter(&adapter, request.planning_request_id)
        .await
        .unwrap();
    assert!(
        outcome.validation_errors.is_empty(),
        "{:?}",
        outcome.validation_errors
    );
    assert_eq!(outcome.proposal.status, ProposalStatus::Validated);
    assert_eq!(outcome.diff.phase_creates, 1);
    assert_eq!(outcome.diff.work_item_creates, 3);
    assert!(
        outcome
            .diff
            .entries
            .iter()
            .any(|e| e.label.contains("Planner Protocol Foundation"))
    );

    let approved = planner
        .approve_proposal(outcome.proposal.proposal_id)
        .await
        .unwrap();
    assert_eq!(approved.status, ProposalStatus::Approved);

    let apply = planner
        .apply_proposal(outcome.proposal.proposal_id)
        .await
        .unwrap();
    assert!(!apply.already_applied);
    assert_eq!(apply.phases_created, 1);
    assert_eq!(apply.work_items_created, 3);

    // Real Project State now contains the phase and work items.
    let phases = state.list_phases(project_id).await.unwrap();
    assert!(
        phases
            .iter()
            .any(|p| p.title == "Planner Protocol Foundation")
    );
    let work_items = state.list_work_items(project_id).await.unwrap();
    for title in [
        "PlanningRequest / Result protocol",
        "Proposal Validator",
        "Manual Web Planner export/import",
    ] {
        assert!(
            work_items.iter().any(|w| w.title == title),
            "missing {title}"
        );
    }

    pool.close().await;
}

// ============================================================
// Test 1b — ARGUS plans its own remaining V0.43 work (no apply)
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn argus_self_planning_request_validates_and_diffs() {
    let pool = connect().await;
    let (state, _intel, planner, _repo) = services(&pool);
    let project = state.bootstrap_argus().await.unwrap();

    let v043 = state
        .list_milestones(project.project_id)
        .await
        .unwrap()
        .into_iter()
        .find(|m| m.external_key.as_deref() == Some("V0.43"))
        .expect("V0.43 milestone");

    let request = create_request(
        &planner,
        project.project_id,
        "Plan the remaining implementation work required to close V0.43 Planner Harness.",
    )
    .await;
    assert!(request.context_hash.starts_with("sha256:"));

    let proposal = PlanProposal {
        work_item_changes: vec![create_work_item(
            "Clarify planner apply semantics",
            v043.milestone_id,
            Some("P43-SELF-1"),
        )],
        ..Default::default()
    };
    let adapter = FakePlannerAdapter::new(PlanningStatus::Proposed, proposal)
        .with_summary("self-planning dry run");
    let outcome = planner
        .run_adapter(&adapter, request.planning_request_id)
        .await
        .unwrap();

    assert!(
        outcome.validation_errors.is_empty(),
        "{:?}",
        outcome.validation_errors
    );
    assert_eq!(outcome.proposal.status, ProposalStatus::Validated);
    assert_eq!(outcome.diff.work_item_creates, 1);
    assert!(!outcome.diff.entries.is_empty());
    // Deliberately NOT applied: the real roadmap is untouched.
    assert!(outcome.proposal.applied_at.is_none());

    pool.close().await;
}

// ============================================================
// Test 2 — planner cannot fabricate VerifiedComplete
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn fabricated_verified_complete_is_rejected() {
    let pool = connect().await;
    let (state, _intel, planner, _repo) = services(&pool);
    let project_id = new_project(&state, "plan-verify").await;
    let milestone = active_milestone(&state, project_id, "M").await;

    let request = create_request(&planner, project_id, "illegal verification").await;
    let mut item = create_work_item("Fake done", milestone, None);
    item.status = Some(argus_core::project_state::WorkItemStatus::VerifiedComplete);
    let proposal = PlanProposal {
        work_item_changes: vec![item],
        ..Default::default()
    };
    let adapter = FakePlannerAdapter::new(PlanningStatus::Proposed, proposal);

    let outcome = planner
        .run_adapter(&adapter, request.planning_request_id)
        .await
        .unwrap();
    assert_eq!(outcome.proposal.status, ProposalStatus::Rejected);
    assert!(
        outcome
            .validation_errors
            .iter()
            .any(|e| e.contains("VerifiedComplete"))
    );

    // Applying a rejected proposal is impossible.
    let apply = planner.apply_proposal(outcome.proposal.proposal_id).await;
    assert!(apply.is_err());

    pool.close().await;
}

// ============================================================
// Test 3 — cross-project injection
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn cross_project_injection_is_rejected() {
    let pool = connect().await;
    let (state, _intel, planner, _repo) = services(&pool);
    let argus = new_project(&state, "plan-argus").await;
    let geo = new_project(&state, "plan-geo").await;
    let geo_milestone = active_milestone(&state, geo, "GEO milestone").await;

    let request = create_request(&planner, argus, "steal GEO ids").await;
    let proposal = PlanProposal {
        // References another project's milestone.
        work_item_changes: vec![create_work_item("Injected", geo_milestone, None)],
        ..Default::default()
    };
    let adapter = FakePlannerAdapter::new(PlanningStatus::Proposed, proposal);
    let outcome = planner
        .run_adapter(&adapter, request.planning_request_id)
        .await
        .unwrap();

    assert_eq!(outcome.proposal.status, ProposalStatus::Rejected);
    assert!(
        outcome
            .validation_errors
            .iter()
            .any(|e| e.contains("cross-project")),
        "{:?}",
        outcome.validation_errors
    );

    pool.close().await;
}

// ============================================================
// Test 4 — stale proposal does not mutate state
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn stale_proposal_does_not_mutate_state() {
    let pool = connect().await;
    let (state, _intel, planner, _repo) = services(&pool);
    let project_id = new_project(&state, "plan-stale").await;
    let milestone = active_milestone(&state, project_id, "M").await;

    // Plan A, validated + approved at revision 0.
    let request_a = create_request(&planner, project_id, "first plan").await;
    let proposal_a = PlanProposal {
        work_item_changes: vec![create_work_item("Plan A item", milestone, None)],
        ..Default::default()
    };
    let out_a = planner
        .run_adapter(
            &FakePlannerAdapter::new(PlanningStatus::Proposed, proposal_a),
            request_a.planning_request_id,
        )
        .await
        .unwrap();
    planner
        .approve_proposal(out_a.proposal.proposal_id)
        .await
        .unwrap();

    // Plan B applies and bumps the revisions.
    let request_b = create_request(&planner, project_id, "second plan").await;
    let proposal_b = PlanProposal {
        work_item_changes: vec![create_work_item("Plan B item", milestone, None)],
        ..Default::default()
    };
    let out_b = planner
        .run_adapter(
            &FakePlannerAdapter::new(PlanningStatus::Proposed, proposal_b),
            request_b.planning_request_id,
        )
        .await
        .unwrap();
    planner
        .approve_proposal(out_b.proposal.proposal_id)
        .await
        .unwrap();
    planner
        .apply_proposal(out_b.proposal.proposal_id)
        .await
        .unwrap();

    // Plan A is now stale: apply must fail with no mutation.
    let stale = planner.apply_proposal(out_a.proposal.proposal_id).await;
    assert!(stale.is_err());
    let record = planner
        .get_proposal(out_a.proposal.proposal_id)
        .await
        .unwrap();
    assert_eq!(record.status, ProposalStatus::Stale);
    let items = state.list_work_items(project_id).await.unwrap();
    assert!(!items.iter().any(|w| w.title == "Plan A item"));

    pool.close().await;
}

// ============================================================
// Test 5 — double apply is idempotent
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn double_apply_is_idempotent() {
    let pool = connect().await;
    let (state, _intel, planner, _repo) = services(&pool);
    let project_id = new_project(&state, "plan-idem").await;
    let milestone = active_milestone(&state, project_id, "M").await;

    let request = create_request(&planner, project_id, "idempotent apply").await;
    let proposal = PlanProposal {
        work_item_changes: vec![create_work_item("Once only", milestone, None)],
        ..Default::default()
    };
    let outcome = planner
        .run_adapter(
            &FakePlannerAdapter::new(PlanningStatus::Proposed, proposal),
            request.planning_request_id,
        )
        .await
        .unwrap();
    planner
        .approve_proposal(outcome.proposal.proposal_id)
        .await
        .unwrap();
    let first = planner
        .apply_proposal(outcome.proposal.proposal_id)
        .await
        .unwrap();
    assert!(!first.already_applied);

    let second = planner
        .apply_proposal(outcome.proposal.proposal_id)
        .await
        .unwrap();
    assert!(second.already_applied);
    assert_eq!(second.work_items_created, 0);

    let count = state
        .list_work_items(project_id)
        .await
        .unwrap()
        .iter()
        .filter(|w| w.title == "Once only")
        .count();
    assert_eq!(count, 1, "SECOND APPLY CREATES DUPLICATES: NO");

    pool.close().await;
}

// ============================================================
// Test 6 — duplicate import + tamper detection
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn duplicate_import_and_tamper_detection() {
    let pool = connect().await;
    let (state, _intel, planner, _repo) = services(&pool);
    let project_id = new_project(&state, "plan-import").await;
    let milestone = active_milestone(&state, project_id, "M").await;

    let request = create_request(&planner, project_id, "import").await;
    let proposal = PlanProposal {
        work_item_changes: vec![create_work_item("Imported", milestone, None)],
        ..Default::default()
    };
    let outcome = planner
        .run_adapter(
            &FakePlannerAdapter::new(PlanningStatus::Proposed, proposal),
            request.planning_request_id,
        )
        .await
        .unwrap();

    // Importing the same result again reuses the proposal.
    let again = planner.import_result(outcome.result.clone()).await.unwrap();
    assert_eq!(again.proposal.proposal_id, outcome.proposal.proposal_id);

    // Tampered context hash is rejected.
    let mut tampered: PlanningResult = outcome.result.clone();
    tampered.context_hash = "sha256:tampered".to_string();
    assert!(planner.import_result(tampered).await.is_err());

    // Unknown protocol version is rejected.
    let mut bad_version = outcome.result.clone();
    bad_version.protocol_version = "planner-protocol-v9".to_string();
    assert!(planner.import_result(bad_version).await.is_err());

    pool.close().await;
}

// ============================================================
// Test 7 — manual packet export
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn manual_export_packet_is_usable() {
    let pool = connect().await;
    let (state, _intel, planner, _repo) = services(&pool);
    let project_id = new_project(&state, "plan-export").await;
    let _ = active_milestone(&state, project_id, "V0.43 Planner Harness / Protocol").await;

    let request = create_request(&planner, project_id, "Plan V0.43 remaining work").await;
    let packet = planner
        .export_packet(request.planning_request_id)
        .await
        .unwrap();

    assert!(packet.contains("planner-protocol-v1"));
    assert!(packet.contains(&request.context_hash));
    assert!(packet.contains(&request.request_hash));
    assert!(packet.contains("OUTPUT REQUIREMENT"));
    assert!(packet.contains("verified_complete"));
    assert!(packet.contains("Plan V0.43 remaining work"));
    let lower = packet.to_lowercase();
    assert!(!lower.contains("claude"));
    assert!(!lower.contains("gpt"));
    assert!(!lower.contains("anthropic"));

    pool.close().await;
}

// ============================================================
// Test 8 — restart persistence of an approved (unapplied) proposal
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn approved_proposal_survives_restart() {
    let pool = connect().await;
    let (state, _intel, planner, _repo) = services(&pool);
    let project_id = new_project(&state, "plan-restart").await;
    let milestone = active_milestone(&state, project_id, "M").await;

    let request = create_request(&planner, project_id, "restart").await;
    let proposal = PlanProposal {
        work_item_changes: vec![create_work_item("Persisted", milestone, None)],
        ..Default::default()
    };
    let outcome = planner
        .run_adapter(
            &FakePlannerAdapter::new(PlanningStatus::Proposed, proposal),
            request.planning_request_id,
        )
        .await
        .unwrap();
    planner
        .approve_proposal(outcome.proposal.proposal_id)
        .await
        .unwrap();

    // New pool + services (simulated restart).
    let pool2 = connect().await;
    let (_state2, _intel2, planner2, _repo2) = services(&pool2);
    let record = planner2
        .get_proposal(outcome.proposal.proposal_id)
        .await
        .unwrap();
    assert_eq!(record.status, ProposalStatus::Approved);
    assert!(record.applied_at.is_none());

    // The full lifecycle is auditable.
    let events = planner2
        .list_lifecycle(outcome.proposal.proposal_id)
        .await
        .unwrap();
    assert!(events.iter().any(|e| e.event_type == "validated"));
    assert!(events.iter().any(|e| e.event_type == "approved"));

    pool.close().await;
    pool2.close().await;
}

// ============================================================
// Test 9 — apply is transactional (partial failure rolls back)
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn partial_apply_rolls_back() {
    let pool = connect().await;
    let (state, _intel, _planner, repo) = services(&pool);
    let project_id = new_project(&state, "plan-tx").await;
    let milestone = active_milestone(&state, project_id, "M").await;

    // Build an Approved proposal directly (bypassing the validator) whose
    // second create duplicates the first's external_key, which the DB unique
    // index rejects at apply time.
    let request = argus_core::planner::PlanningRequest {
        planning_request_id: Uuid::new_v4(),
        project_id,
        planning_kind: PlanningKind::PhasePlanning,
        role: "project_manager".to_string(),
        purpose: "planning".to_string(),
        context_packet_ref: None,
        context_hash: "sha256:tx".to_string(),
        compiler_version: "context-compiler-v1".to_string(),
        work_item_id: None,
        task_profile_ref: None,
        routing_decision_ref: None,
        state_revision: 0,
        roadmap_revision: 0,
        objective: "tx".to_string(),
        constraints: vec![],
        requested_outputs: vec![],
        protocol_version: argus_core::planner::PROTOCOL_VERSION.to_string(),
        request_hash: "sha256:txreq".to_string(),
        created_at: chrono::Utc::now(),
    };
    let request = repo.save_request(request).await.unwrap();
    let result = PlanningResult {
        planning_result_id: Uuid::new_v4(),
        planning_request_id: request.planning_request_id,
        status: PlanningStatus::Proposed,
        summary: None,
        proposal: PlanProposal {
            work_item_changes: vec![
                create_work_item("TX A", milestone, Some("DUP-KEY")),
                create_work_item("TX B", milestone, Some("DUP-KEY")),
            ],
            ..Default::default()
        },
        assumptions: vec![],
        clarifications: vec![],
        warnings: vec![],
        planner_metadata: Default::default(),
        protocol_version: argus_core::planner::PROTOCOL_VERSION.to_string(),
        context_hash: request.context_hash.clone(),
        request_hash: request.request_hash.clone(),
        result_hash: String::new(),
        created_at: chrono::Utc::now(),
    };
    let stored = repo.save_result(result, project_id).await.unwrap();
    let proposal = argus_core::planner::ProposalRecord {
        proposal_id: Uuid::new_v4(),
        planning_result_id: stored.result.planning_result_id,
        planning_request_id: request.planning_request_id,
        project_id,
        status: ProposalStatus::Approved,
        proposal: stored.result.proposal.clone(),
        validation_errors: vec![],
        context_hash: request.context_hash.clone(),
        request_hash: request.request_hash.clone(),
        result_hash: stored.result.result_hash.clone(),
        state_revision: 0,
        roadmap_revision: 0,
        applied_at: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    let proposal = repo.save_proposal(proposal).await.unwrap();

    let outcome = repo.apply_proposal(proposal.proposal_id).await;
    assert!(
        outcome.is_err(),
        "duplicate external_key must fail the apply"
    );

    // Transaction rolled back: neither work item exists.
    let items = state.list_work_items(project_id).await.unwrap();
    assert!(!items.iter().any(|w| w.title == "TX A"));
    assert!(!items.iter().any(|w| w.title == "TX B"));

    pool.close().await;
}

// ============================================================
// Test 10 — untrusted JSON is parsed strictly
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn untrusted_json_rejects_unknown_fields() {
    let pool = connect().await;
    let (state, _intel, planner, _repo) = services(&pool);
    let project_id = new_project(&state, "plan-sec").await;
    let milestone = active_milestone(&state, project_id, "M").await;

    let request = create_request(&planner, project_id, "security").await;
    let outcome = planner
        .run_adapter(
            &FakePlannerAdapter::new(
                PlanningStatus::Proposed,
                PlanProposal {
                    work_item_changes: vec![create_work_item("Sec", milestone, None)],
                    ..Default::default()
                },
            ),
            request.planning_request_id,
        )
        .await
        .unwrap();
    let json = serde_json::to_string(&outcome.result).unwrap();

    // Unknown top-level field is rejected.
    let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
    value["command"] = serde_json::json!("rm -rf /");
    assert!(serde_json::from_value::<PlanningResult>(value).is_err());

    // Unknown field inside the proposal is rejected.
    let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
    value["proposal"]["shell_command"] = serde_json::json!("curl http://evil");
    assert!(serde_json::from_value::<PlanningResult>(value).is_err());

    // A path-like extra field is likewise rejected.
    let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
    value["proposal"]["file_path"] = serde_json::json!("../../etc/passwd");
    assert!(serde_json::from_value::<PlanningResult>(value).is_err());

    pool.close().await;
}

// ============================================================
// Test 11 — determinism of request hash and diff
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn request_hash_and_diff_are_deterministic() {
    let pool = connect().await;
    let (state, _intel, planner, _repo) = services(&pool);
    let project_id = new_project(&state, "plan-det").await;
    let _ = active_milestone(&state, project_id, "M").await;

    let a = create_request(&planner, project_id, "same objective").await;
    let b = planner
        .create_planning_request(CreatePlanningRequestInput {
            project_id,
            planning_kind: PlanningKind::PhasePlanning,
            role: ContextRole::ProjectManager,
            purpose: ContextPurpose::Planning,
            objective: "same objective".to_string(),
            constraints: vec!["additive".to_string()],
            requested_outputs: vec![RequestedOutput::RoadmapChanges, RequestedOutput::WorkItems],
            work_item_id: None,
        })
        .await
        .unwrap();
    assert_eq!(a.request_hash, b.request_hash);
    assert_eq!(
        a.planning_request_id, b.planning_request_id,
        "reuses request"
    );

    pool.close().await;
}

// ============================================================
// Test 12 — project isolation of proposals
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn proposals_are_project_scoped() {
    let pool = connect().await;
    let (state, _intel, planner, _repo) = services(&pool);
    let argus = new_project(&state, "plan-iso-argus").await;
    let geo = new_project(&state, "plan-iso-geo").await;
    let argus_ms = active_milestone(&state, argus, "ARGUS M").await;
    let geo_ms = active_milestone(&state, geo, "GEO M").await;

    let ra = create_request(&planner, argus, "argus plan").await;
    planner
        .run_adapter(
            &FakePlannerAdapter::new(
                PlanningStatus::Proposed,
                PlanProposal {
                    work_item_changes: vec![create_work_item("ARGUS work", argus_ms, None)],
                    ..Default::default()
                },
            ),
            ra.planning_request_id,
        )
        .await
        .unwrap();
    let rg = create_request(&planner, geo, "geo plan").await;
    planner
        .run_adapter(
            &FakePlannerAdapter::new(
                PlanningStatus::Proposed,
                PlanProposal {
                    work_item_changes: vec![create_work_item("GEO work", geo_ms, None)],
                    ..Default::default()
                },
            ),
            rg.planning_request_id,
        )
        .await
        .unwrap();

    let argus_proposals = planner.list_proposals(argus, 50).await.unwrap();
    assert!(argus_proposals.iter().all(|p| p.project_id == argus));
    assert!(argus_proposals.iter().all(|p| {
        p.proposal
            .work_item_changes
            .iter()
            .all(|w| w.title != "GEO work")
    }));
    let geo_proposals = planner.list_proposals(geo, 50).await.unwrap();
    assert!(geo_proposals.iter().all(|p| p.project_id == geo));

    pool.close().await;
}
