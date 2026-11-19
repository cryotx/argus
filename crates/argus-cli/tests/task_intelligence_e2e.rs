//! V0.42 Task Intelligence / Model Routing — end-to-end tests against real
//! PostgreSQL. No LLM, no provider, no paid execution.

mod e2e_support;

use std::sync::Arc;

use uuid::Uuid;

use argus_core::context::{ContextPurpose, ContextRole};
use argus_core::intelligence::{
    EscalationAction, FailureClass, FailureInput, OutcomeInput, PostgresTaskIntelligenceRepository,
    TaskIntelligenceService, VerificationRequirement,
};
use argus_core::project_state::{
    MilestoneStatus, NewMilestone, NewProject, NewWorkItem, PostgresProjectStateRepository,
    ProjectStateService,
};
use argus_core::task_types::Priority;

use e2e_support::connect;

fn services(pool: &sqlx::PgPool) -> (ProjectStateService, TaskIntelligenceService) {
    let state: Arc<dyn argus_core::project_state::ProjectStateRepository> =
        Arc::new(PostgresProjectStateRepository::new(pool.clone()));
    let intel: Arc<dyn argus_core::intelligence::TaskIntelligenceRepository> =
        Arc::new(PostgresTaskIntelligenceRepository::new(pool.clone()));
    (
        ProjectStateService::new(state.clone()),
        TaskIntelligenceService::new(state, intel),
    )
}

async fn new_project(state: &ProjectStateService, prefix: &str) -> Uuid {
    let slug = format!("{prefix}-{}", &Uuid::new_v4().to_string()[..8]);
    state
        .create_project_state(NewProject {
            name: format!("{prefix} {slug}"),
            slug,
            description: "task intelligence test".to_string(),
            priority: Priority::Normal,
        })
        .await
        .unwrap()
        .project_id
}

async fn milestone(state: &ProjectStateService, project_id: Uuid) -> Uuid {
    let m = state
        .create_milestone(NewMilestone {
            project_id,
            title: "TI Milestone".to_string(),
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

async fn work_item(
    state: &ProjectStateService,
    project_id: Uuid,
    milestone_id: Uuid,
    title: &str,
    description: &str,
    with_dod: bool,
) -> Uuid {
    state
        .create_work_item(NewWorkItem {
            project_id,
            milestone_id: Some(milestone_id),
            phase_id: None,
            title: title.to_string(),
            description: description.to_string(),
            priority: Priority::Normal,
            definition_of_done: with_dod.then(|| format!("{title} done")),
            source: Some("e2e".to_string()),
            external_key: None,
            weight: 1,
        })
        .await
        .unwrap()
        .work_item_id
}

// ============================================================
// Test 1 — ARGUS classifies its own V0.42 WorkItem
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn argus_real_classification_is_explainable() {
    let pool = connect().await;
    let (state, intel) = services(&pool);
    let project = state.bootstrap_argus().await.unwrap();

    let work_item = state
        .list_work_items(project.project_id)
        .await
        .unwrap()
        .into_iter()
        .find(|w| w.title == "Task Intelligence / Model Routing")
        .expect("V0.42 work item");

    let (profile, decision) = intel
        .classify_work_item(
            project.project_id,
            work_item.work_item_id,
            ContextRole::Developer,
            ContextPurpose::Execution,
        )
        .await
        .unwrap();

    assert!(
        profile.recommended_tier > argus_core::intelligence::ModelTier::T0,
        "a routing/intelligence work item must not be T0, got {}",
        profile.recommended_tier
    );
    assert!(profile.confidence > 0.0 && profile.confidence <= 1.0);
    for score in [
        profile.complexity_score,
        profile.risk_score,
        profile.ambiguity_score,
        profile.reasoning_score,
        profile.scope_score,
        profile.verification_score,
        profile.context_score,
        profile.reversibility_score,
    ] {
        assert!((0.0..=1.0).contains(&score), "score out of range: {score}");
    }
    assert!(
        !profile.reasons.is_empty(),
        "a non-trivial tier must be explainable"
    );
    assert!(!decision.reasons.is_empty());
    assert!(!profile.required_capabilities.is_empty());
    assert!(
        decision.required_tier >= profile.recommended_tier,
        "routing must respect the profile floor"
    );
    assert!(decision.verification_requirement >= profile.verification_requirement);
    assert_eq!(decision.role, "developer");
    assert_eq!(decision.purpose, "execution");

    // Persisted and reusable.
    let profiles = intel.list_profiles(project.project_id, 50).await.unwrap();
    assert!(profiles.iter().any(|p| p.input_hash == profile.input_hash));

    pool.close().await;
}

// ============================================================
// Test 2 — simple / standard / complex / critical
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn tier_expectations_for_four_task_shapes() {
    let pool = connect().await;
    let (state, intel) = services(&pool);
    let project_id = new_project(&state, "ti-tiers").await;
    let ms = milestone(&state, project_id).await;

    let simple = work_item(
        &state,
        project_id,
        ms,
        "Change README typo",
        "Fix a spelling mistake in the documentation.",
        true,
    )
    .await;
    let standard = work_item(
        &state,
        project_id,
        ms,
        "Add a bounded REST endpoint using the existing repository pattern",
        "Implement a new read-only API endpoint.",
        true,
    )
    .await;
    let complex = work_item(
        &state,
        project_id,
        ms,
        "Refactor scheduler recovery across multiple crates while preserving restart semantics",
        "Move recovery logic and keep restart behavior identical.",
        true,
    )
    .await;
    let critical = work_item(
        &state,
        project_id,
        ms,
        "Modify authentication and secret permission boundaries",
        "Tighten auth checks and rotate secret handling.",
        true,
    )
    .await;

    let classify = |id| {
        let intel = &intel;
        async move {
            intel
                .classify_work_item(
                    project_id,
                    id,
                    ContextRole::Developer,
                    ContextPurpose::Execution,
                )
                .await
                .unwrap()
        }
    };

    let (p_simple, d_simple) = classify(simple).await;
    assert_eq!(
        p_simple.recommended_tier,
        argus_core::intelligence::ModelTier::T0
    );
    assert_eq!(
        d_simple.verification_requirement,
        VerificationRequirement::Basic
    );

    let (p_standard, _) = classify(standard).await;
    assert_eq!(
        p_standard.recommended_tier,
        argus_core::intelligence::ModelTier::T1
    );

    let (p_complex, d_complex) = classify(complex).await;
    assert!(
        p_complex.recommended_tier >= argus_core::intelligence::ModelTier::T2,
        "got {}",
        p_complex.recommended_tier
    );
    assert!(d_complex.verification_requirement >= VerificationRequirement::Strong);

    let (p_critical, d_critical) = classify(critical).await;
    assert!(
        p_critical.recommended_tier >= argus_core::intelligence::ModelTier::T2,
        "risk floor must apply, got {}",
        p_critical.recommended_tier
    );
    assert!(d_critical.verification_requirement >= VerificationRequirement::Strong);

    pool.close().await;
}

// ============================================================
// Test 3 — short high-risk vs long low-risk
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn prompt_length_does_not_drive_difficulty() {
    let pool = connect().await;
    let (state, intel) = services(&pool);
    let project_id = new_project(&state, "ti-length").await;
    let ms = milestone(&state, project_id).await;

    let short_risky = work_item(&state, project_id, ms, "Fix duplicate billing.", "", false).await;
    let long_docs = work_item(
        &state,
        project_id,
        ms,
        "Reformat the documentation guide",
        &"Rewrite the documentation prose, improve wording, adjust headings, \
          fix comma splices, rewrite the introduction, add examples, clarify \
          terminology, and reorganize the table of contents. "
            .repeat(20),
        false,
    )
    .await;

    let (risky, _) = intel
        .classify_work_item(
            project_id,
            short_risky,
            ContextRole::Developer,
            ContextPurpose::Execution,
        )
        .await
        .unwrap();
    let (docs, _) = intel
        .classify_work_item(
            project_id,
            long_docs,
            ContextRole::Developer,
            ContextPurpose::Execution,
        )
        .await
        .unwrap();

    assert!(
        risky.recommended_tier >= argus_core::intelligence::ModelTier::T2,
        "short financial prompt must not be T0/T1, got {}",
        risky.recommended_tier
    );
    assert!(
        docs.recommended_tier <= argus_core::intelligence::ModelTier::T1,
        "long documentation prompt must stay low, got {}",
        docs.recommended_tier
    );

    pool.close().await;
}

// ============================================================
// Test 4 — failure escalation matrix
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn failure_escalation_matrix() {
    let pool = connect().await;
    let (state, intel) = services(&pool);
    let project_id = new_project(&state, "ti-escalation").await;
    let ms = milestone(&state, project_id).await;

    let auth = work_item(
        &state,
        project_id,
        ms,
        "Modify authentication and secret permission boundaries",
        "Tighten auth checks.",
        true,
    )
    .await;
    let (_, decision) = intel
        .classify_work_item(
            project_id,
            auth,
            ContextRole::Developer,
            ContextPurpose::Execution,
        )
        .await
        .unwrap();
    let routing_id = decision.routing_id;

    let decide = |failure, attempts, truncated| {
        intel.record_failure(
            routing_id,
            FailureInput {
                failure_class: Some(failure),
                error_type: None,
                attempts: Some(attempts),
                context_truncated: truncated,
            },
        )
    };

    // Environment failure: repair, never escalate.
    let env = decide(FailureClass::EnvironmentFailure, 5, false)
        .await
        .unwrap();
    assert_eq!(env.action, EscalationAction::RepairEnvironment);
    assert_eq!(env.to_tier, None);

    // Budget / quota: never escalate.
    for failure in [FailureClass::BudgetFailure, FailureClass::QuotaFailure] {
        let d = decide(failure, 5, false).await.unwrap();
        assert!(!d.action.escalates_tier(), "{failure} must not escalate");
    }

    // Context insufficient with truncated context: recompile first.
    let ctx = decide(FailureClass::ContextInsufficient, 1, true)
        .await
        .unwrap();
    assert_eq!(ctx.action, EscalationAction::RecompileContext);

    // Reasoning failure at T3: human, not T4.
    assert_eq!(
        decision.required_tier,
        argus_core::intelligence::ModelTier::T3,
        "auth task should be T3"
    );
    let reasoning = decide(FailureClass::ReasoningFailure, 3, false)
        .await
        .unwrap();
    assert_eq!(reasoning.action, EscalationAction::EscalateHuman);
    assert!(reasoning.requires_human);

    // Escalations persisted.
    let escalations = intel.list_escalations(project_id, 50).await.unwrap();
    assert!(escalations.len() >= 4);

    pool.close().await;
}

// ============================================================
// Test 5 — outcomes + routing statistics
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn routing_outcomes_and_statistics() {
    let pool = connect().await;
    let (state, intel) = services(&pool);
    let project_id = new_project(&state, "ti-stats").await;
    let ms = milestone(&state, project_id).await;
    let item = work_item(
        &state,
        project_id,
        ms,
        "Add a bounded REST endpoint",
        "Implement an endpoint.",
        true,
    )
    .await;

    let (_, decision) = intel
        .classify_work_item(
            project_id,
            item,
            ContextRole::Developer,
            ContextPurpose::Execution,
        )
        .await
        .unwrap();

    intel
        .record_outcome(
            decision.routing_id,
            OutcomeInput {
                success: true,
                failure_class: None,
                attempts: 1,
                duration_ms: Some(1000),
                artifact_id: None,
            },
        )
        .await
        .unwrap();
    intel
        .record_outcome(
            decision.routing_id,
            OutcomeInput {
                success: false,
                failure_class: Some(FailureClass::VerificationFailure),
                attempts: 3,
                duration_ms: Some(2000),
                artifact_id: None,
            },
        )
        .await
        .unwrap();

    let stats = intel.routing_stats(project_id).await.unwrap();
    assert_eq!(stats.len(), 1, "one (kind, tier) bucket");
    let row = &stats[0];
    assert_eq!(row.attempts, 2);
    assert_eq!(row.successes, 1);
    assert!((row.failure_rate - 0.5).abs() < 1e-6);
    assert!((row.average_attempts - 2.0).abs() < 1e-6);

    pool.close().await;
}

// ============================================================
// Test 6 — determinism + restart
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn classification_and_routing_are_deterministic() {
    let pool = connect().await;
    let (state, intel) = services(&pool);
    let project_id = new_project(&state, "ti-det").await;
    let ms = milestone(&state, project_id).await;
    let item = work_item(
        &state,
        project_id,
        ms,
        "Refactor scheduler recovery across multiple crates",
        "Preserve restart semantics.",
        true,
    )
    .await;

    let (p1, d1) = intel
        .classify_work_item(
            project_id,
            item,
            ContextRole::Developer,
            ContextPurpose::Execution,
        )
        .await
        .unwrap();
    let (p2, d2) = intel
        .classify_work_item(
            project_id,
            item,
            ContextRole::Developer,
            ContextPurpose::Execution,
        )
        .await
        .unwrap();
    assert_eq!(p1.input_hash, p2.input_hash);
    assert_eq!(d1.input_hash, d2.input_hash);
    assert_eq!(p1.profile_id, p2.profile_id, "idempotent profile reuse");
    assert_eq!(d1.routing_id, d2.routing_id, "idempotent routing reuse");
    assert_eq!(p1.recommended_tier, p2.recommended_tier);

    // Restart: new repositories/service.
    let pool2 = connect().await;
    let (state2, intel2) = services(&pool2);
    let (p3, d3) = intel2
        .classify_work_item(
            project_id,
            item,
            ContextRole::Developer,
            ContextPurpose::Execution,
        )
        .await
        .unwrap();
    assert_eq!(p1.input_hash, p3.input_hash);
    assert_eq!(d1.input_hash, d3.input_hash);
    let _ = state2;

    pool.close().await;
    pool2.close().await;
}

// ============================================================
// Test 7 — project isolation
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn intelligence_is_project_scoped() {
    let pool = connect().await;
    let (state, intel) = services(&pool);
    let argus = new_project(&state, "ti-iso-argus").await;
    let geo = new_project(&state, "ti-iso-geo").await;
    let argus_ms = milestone(&state, argus).await;
    let geo_ms = milestone(&state, geo).await;

    let a = work_item(&state, argus, argus_ms, "Fix duplicate billing.", "", false).await;
    let g = work_item(&state, geo, geo_ms, "Reformat documentation", "docs", false).await;

    let (_, a_decision) = intel
        .classify_work_item(argus, a, ContextRole::Developer, ContextPurpose::Execution)
        .await
        .unwrap();
    intel
        .record_outcome(
            a_decision.routing_id,
            OutcomeInput {
                success: false,
                failure_class: Some(FailureClass::ReasoningFailure),
                attempts: 5,
                duration_ms: None,
                artifact_id: None,
            },
        )
        .await
        .unwrap();
    let (_, _g_decision) = intel
        .classify_work_item(geo, g, ContextRole::Developer, ContextPurpose::Execution)
        .await
        .unwrap();

    // ARGUS stats contain its outcome; GEO stats are empty/independent.
    let argus_stats = intel.routing_stats(argus).await.unwrap();
    assert_eq!(argus_stats.len(), 1);
    let geo_stats = intel.routing_stats(geo).await.unwrap();
    assert!(
        geo_stats.is_empty(),
        "GEO must not see ARGUS outcomes: {geo_stats:?}"
    );

    let argus_profiles = intel.list_profiles(argus, 50).await.unwrap();
    assert!(argus_profiles.iter().all(|p| p.project_id == argus));
    let geo_profiles = intel.list_profiles(geo, 50).await.unwrap();
    assert!(geo_profiles.iter().all(|p| p.project_id == geo));

    // GEO work item cannot be classified under ARGUS.
    let cross = intel
        .classify_work_item(argus, g, ContextRole::Developer, ContextPurpose::Execution)
        .await;
    assert!(cross.is_err(), "cross-project classification must fail");

    pool.close().await;
}
