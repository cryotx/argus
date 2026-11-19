//! V0.44 Provider / Auth / Subscription / Quota — end-to-end tests against
//! real PostgreSQL using deterministic fake adapters/quota/budget. No model
//! call, no paid API.

mod e2e_support;

use std::sync::Arc;

use uuid::Uuid;

use argus_core::intelligence::{CapabilityRequirement, ModelTier, ReasoningEffort};
use argus_core::project_state::{NewProject, PostgresProjectStateRepository, ProjectStateService};
use argus_core::provider::{
    AuthKind, AuthStatus, BillingMode, FakeBudgetGate, FakeProviderAdapter, FallbackPolicy,
    NewAuthProfile, PostgresProviderRepository, ProviderAdapter, ProviderRepository,
    ProviderRoutingInput, ProviderRoutingResult, ProviderService, QuotaAvailability,
    QuotaConfidence, QuotaKind, QuotaSnapshot, QuotaSource, RuntimeProvider, SecretReference,
    SelectionPolicy, StaticQuotaProvider,
};
use argus_core::task_types::Priority;

use e2e_support::connect;

fn fake_adapters(installed: &[(RuntimeProvider, bool)]) -> Vec<Arc<dyn ProviderAdapter>> {
    installed
        .iter()
        .map(|(p, ok)| Arc::new(FakeProviderAdapter::new(*p, *ok)) as Arc<dyn ProviderAdapter>)
        .collect()
}

fn repo(pool: &sqlx::PgPool) -> Arc<dyn ProviderRepository> {
    Arc::new(PostgresProviderRepository::new(pool.clone()))
}

fn service(
    pool: &sqlx::PgPool,
    installed: &[(RuntimeProvider, bool)],
    allow_budget: bool,
) -> Arc<ProviderService> {
    Arc::new(ProviderService::new(
        repo(pool),
        Arc::new(StaticQuotaProvider::unknown()),
        Some(Arc::new(FakeBudgetGate {
            allow: allow_budget,
            reason: "budget blocked by test".to_string(),
        })),
        fake_adapters(installed),
    ))
}

async fn new_project(pool: &sqlx::PgPool, prefix: &str) -> Uuid {
    let state =
        ProjectStateService::new(Arc::new(PostgresProjectStateRepository::new(pool.clone())));
    let slug = format!("{prefix}-{}", &Uuid::new_v4().to_string()[..8]);
    state
        .create_project_state(NewProject {
            name: format!("{prefix} {slug}"),
            slug,
            description: "provider routing test".to_string(),
            priority: Priority::Normal,
        })
        .await
        .unwrap()
        .project_id
}

/// `auth_profiles` is global (not project-scoped) config, and `reset_auth`
/// truncates it for the whole table. Cargo runs the tests in this binary on
/// many threads against one shared database, so each test must hold this lock
/// for its whole register → route → assert sequence or they delete each other's
/// rows. Serializing the critical section — not the tests — is what makes the
/// suite parallel-safe and deterministic.
static AUTH_GUARD: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Auth profiles are global config, so tests reset them to stay deterministic.
/// Callers must hold `AUTH_GUARD`.
async fn reset_auth(pool: &sqlx::PgPool) {
    sqlx::query("DELETE FROM auth_profiles")
        .execute(pool)
        .await
        .unwrap();
}

async fn register(
    service: &ProviderService,
    provider: RuntimeProvider,
    kind: AuthKind,
    locator: &str,
) -> Uuid {
    service
        .ensure_auth_profile(NewAuthProfile {
            provider_id: provider,
            auth_kind: kind,
            account_label: Some(format!("{}-{}", provider.as_str(), kind.as_str())),
            workspace_label: None,
            status: AuthStatus::Available,
            secret_reference: Some(SecretReference::new(locator)),
            credential_source: Some("test".to_string()),
        })
        .await
        .unwrap()
        .auth_profile_id
}

fn input(
    project_id: Uuid,
    tier: ModelTier,
    caps: Vec<CapabilityRequirement>,
) -> ProviderRoutingInput {
    ProviderRoutingInput {
        project_id,
        routing_decision_id: None,
        task_id: None,
        role: "developer".to_string(),
        purpose: "execution".to_string(),
        required_tier: tier,
        required_capabilities: caps,
        reasoning_effort: ReasoningEffort::Medium,
        estimated_cost_usd: None,
        policy: SelectionPolicy::default(),
    }
}

fn exhausted(provider: RuntimeProvider, auth: Uuid) -> QuotaSnapshot {
    QuotaSnapshot {
        quota_snapshot_id: Uuid::new_v4(),
        provider_id: provider,
        auth_profile_id: Some(auth),
        quota_kind: QuotaKind::SubscriptionAllowance,
        availability: QuotaAvailability::Unavailable,
        remaining: Some(0.0),
        quota_limit: None,
        unit: "messages".to_string(),
        reset_at: None,
        observed_at: chrono::Utc::now(),
        source: QuotaSource::CliStatus,
        confidence: QuotaConfidence::Medium,
        raw_metadata: serde_json::json!({}),
    }
}

const CLI: &[(RuntimeProvider, bool)] = &[
    (RuntimeProvider::ClaudeCodeCli, true),
    (RuntimeProvider::CodexCli, true),
    (RuntimeProvider::OpenCodeCli, true),
];

// ============================================================
// Test 1 — subscription preferred and selection persisted
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn subscription_route_persists_selection() {
    let _guard = AUTH_GUARD.lock().await;
    let pool = connect().await;
    reset_auth(&pool).await;
    let project_id = new_project(&pool, "prov-sub").await;
    let svc = service(&pool, CLI, true);
    register(
        &svc,
        RuntimeProvider::ClaudeCodeCli,
        AuthKind::SubscriptionLogin,
        "cli:claude",
    )
    .await;
    register(
        &svc,
        RuntimeProvider::OpenAiApi,
        AuthKind::ApiKey,
        "env:OPENAI_API_KEY",
    )
    .await;

    let mut i = input(project_id, ModelTier::T2, vec![]);
    i.policy.allowed_providers = Some(vec![
        RuntimeProvider::ClaudeCodeCli,
        RuntimeProvider::OpenAiApi,
    ]);
    let outcome = svc.route(i).await.unwrap();
    let target = outcome.result.selected_target().expect("selected");
    assert_eq!(target.runtime_provider, RuntimeProvider::ClaudeCodeCli);
    assert_eq!(target.billing_mode, BillingMode::SubscriptionIncluded);
    assert!(!target.selection_reasons.is_empty());

    let selections = svc.list_selections(project_id, 10).await.unwrap();
    assert_eq!(selections.len(), 1);
    assert_eq!(selections[0].provider_id, RuntimeProvider::ClaudeCodeCli);
    let attempts = svc.list_attempts(project_id, 50).await.unwrap();
    assert!(!attempts.is_empty(), "attempts must be recorded for audit");

    pool.close().await;
}

// ============================================================
// Test 2 — API fallback honors the budget gate
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn api_fallback_respects_budget() {
    let _guard = AUTH_GUARD.lock().await;
    let pool = connect().await;
    reset_auth(&pool).await;
    let project_id = new_project(&pool, "prov-budget").await;
    let svc = service(&pool, CLI, true);
    let sub = register(
        &svc,
        RuntimeProvider::ClaudeCodeCli,
        AuthKind::SubscriptionLogin,
        "cli:claude",
    )
    .await;
    register(
        &svc,
        RuntimeProvider::OpenAiApi,
        AuthKind::ApiKey,
        "env:OPENAI_API_KEY",
    )
    .await;
    let r = repo(&pool);
    r.save_quota(exhausted(RuntimeProvider::ClaudeCodeCli, sub))
        .await
        .unwrap();

    // Budget denies the API fallback.
    let blocked = service(&pool, CLI, false);
    let mut bi = input(project_id, ModelTier::T2, vec![]);
    bi.policy.allowed_providers = Some(vec![
        RuntimeProvider::ClaudeCodeCli,
        RuntimeProvider::OpenAiApi,
    ]);
    let out = blocked.route(bi).await.unwrap();
    assert!(matches!(
        out.result,
        ProviderRoutingResult::BudgetBlocked { .. }
    ));
    assert!(
        out.selection.is_none(),
        "no selection is persisted when blocked"
    );

    // Budget allows -> API selected.
    let allowed = service(&pool, CLI, true);
    let mut ai = input(project_id, ModelTier::T2, vec![]);
    ai.policy.allowed_providers = Some(vec![
        RuntimeProvider::ClaudeCodeCli,
        RuntimeProvider::OpenAiApi,
    ]);
    let out = allowed.route(ai).await.unwrap();
    let target = out.result.selected_target().expect("selected");
    assert_eq!(target.billing_mode, BillingMode::ApiMetered);

    pool.close().await;
}

// ============================================================
// Test 3 — Ask fallback returns NeedsApproval, no spend
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn ask_before_api_fallback() {
    let _guard = AUTH_GUARD.lock().await;
    let pool = connect().await;
    reset_auth(&pool).await;
    let project_id = new_project(&pool, "prov-ask").await;
    let svc = service(&pool, CLI, true);
    let sub = register(
        &svc,
        RuntimeProvider::ClaudeCodeCli,
        AuthKind::SubscriptionLogin,
        "cli:claude",
    )
    .await;
    register(
        &svc,
        RuntimeProvider::OpenAiApi,
        AuthKind::ApiKey,
        "env:OPENAI_API_KEY",
    )
    .await;
    repo(&pool)
        .save_quota(exhausted(RuntimeProvider::ClaudeCodeCli, sub))
        .await
        .unwrap();

    let mut i = input(project_id, ModelTier::T2, vec![]);
    i.policy.allowed_providers = Some(vec![
        RuntimeProvider::ClaudeCodeCli,
        RuntimeProvider::OpenAiApi,
    ]);
    i.policy.fallback_policy = FallbackPolicy::Ask;
    let out = svc.route(i).await.unwrap();
    assert!(matches!(
        out.result,
        ProviderRoutingResult::NeedsApproval { .. }
    ));
    assert!(out.selection.is_none());

    pool.close().await;
}

// ============================================================
// Test 4 — unknown quota is not treated as exhausted
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn unknown_quota_is_not_exhausted() {
    let _guard = AUTH_GUARD.lock().await;
    let pool = connect().await;
    reset_auth(&pool).await;
    let project_id = new_project(&pool, "prov-unknown").await;
    let svc = service(&pool, CLI, true);
    let sub = register(
        &svc,
        RuntimeProvider::ClaudeCodeCli,
        AuthKind::SubscriptionLogin,
        "cli:claude",
    )
    .await;
    repo(&pool)
        .save_quota(argus_core::provider::quota::unknown_snapshot(
            RuntimeProvider::ClaudeCodeCli,
            Some(sub),
        ))
        .await
        .unwrap();

    let mut i = input(project_id, ModelTier::T2, vec![]);
    i.policy.allowed_providers = Some(vec![RuntimeProvider::ClaudeCodeCli]);
    let out = svc.route(i).await.unwrap();
    assert_eq!(
        out.result.selected_target().unwrap().runtime_provider,
        RuntimeProvider::ClaudeCodeCli,
        "unknown quota must not be treated as exhausted"
    );

    pool.close().await;
}

// ============================================================
// Test 5 — missing auth returns NeedsLogin (never fabricated)
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn missing_auth_returns_needs_login() {
    let _guard = AUTH_GUARD.lock().await;
    let pool = connect().await;
    reset_auth(&pool).await;
    let project_id = new_project(&pool, "prov-login").await;
    let svc = service(&pool, CLI, true);
    svc.ensure_auth_profile(NewAuthProfile {
        provider_id: RuntimeProvider::ClaudeCodeCli,
        auth_kind: AuthKind::SubscriptionLogin,
        account_label: Some("needs-login".to_string()),
        workspace_label: None,
        status: AuthStatus::NeedsLogin,
        secret_reference: Some(SecretReference::cli_managed("claude")),
        credential_source: Some("cli-managed".to_string()),
    })
    .await
    .unwrap();

    let mut i = input(project_id, ModelTier::T2, vec![]);
    i.policy.allowed_providers = Some(vec![RuntimeProvider::ClaudeCodeCli]);
    let out = svc.route(i).await.unwrap();
    assert!(matches!(
        out.result,
        ProviderRoutingResult::NeedsLogin { .. }
    ));

    pool.close().await;
}

// ============================================================
// Test 6 — project policy isolation
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn project_policy_isolation() {
    let _guard = AUTH_GUARD.lock().await;
    let pool = connect().await;
    reset_auth(&pool).await;
    let argus = new_project(&pool, "prov-iso-argus").await;
    let geo = new_project(&pool, "prov-iso-geo").await;
    let svc = service(&pool, CLI, true);
    register(
        &svc,
        RuntimeProvider::ClaudeCodeCli,
        AuthKind::SubscriptionLogin,
        "cli:claude",
    )
    .await;
    register(
        &svc,
        RuntimeProvider::OpenAiApi,
        AuthKind::ApiKey,
        "env:OPENAI_API_KEY",
    )
    .await;

    // ARGUS denies Claude; GEO allows it. Note the caller-supplied policies
    // below do NOT repeat the denial — `route` must apply the persisted
    // project policy itself, or this test would pass without the feature.
    let mut argus_policy = argus_core::provider::ProjectProviderPolicy::default_for(argus);
    argus_policy.denied_providers = vec![RuntimeProvider::ClaudeCodeCli];
    svc.upsert_policy(argus_policy).await.unwrap();

    let mut argus_i = input(argus, ModelTier::T2, vec![]);
    argus_i.policy.allowed_providers = Some(vec![
        RuntimeProvider::ClaudeCodeCli,
        RuntimeProvider::OpenAiApi,
    ]);
    let argus_out = svc.route(argus_i).await.unwrap();
    assert_ne!(
        argus_out.result.selected_target().unwrap().runtime_provider,
        RuntimeProvider::ClaudeCodeCli,
        "a project denial must survive a caller that never mentions it"
    );

    let mut geo_i = input(geo, ModelTier::T2, vec![]);
    geo_i.policy.allowed_providers = Some(vec![RuntimeProvider::ClaudeCodeCli]);
    let geo_out = svc.route(geo_i).await.unwrap();
    assert_eq!(
        geo_out.result.selected_target().unwrap().runtime_provider,
        RuntimeProvider::ClaudeCodeCli
    );

    pool.close().await;
}

// ============================================================
// Test 7 — secret redaction (locators only, never values)
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn secrets_are_redacted() {
    let _guard = AUTH_GUARD.lock().await;
    let pool = connect().await;
    reset_auth(&pool).await;
    let _ = new_project(&pool, "prov-secret").await;
    let svc = service(&pool, CLI, true);
    register(
        &svc,
        RuntimeProvider::OpenAiApi,
        AuthKind::ApiKey,
        "env:OPENAI_API_KEY",
    )
    .await;

    let profiles = svc.list_auth_profiles().await.unwrap();
    let json = serde_json::to_string(&profiles).unwrap();
    assert!(
        json.contains("env:OPENAI_API_KEY"),
        "locator should be present"
    );
    assert!(
        !json.contains("sk-secret-example"),
        "RAW OAUTH/API SECRETS STORED IN PROJECT DB: NO"
    );
    // The serialized profile is a locator, not a value.
    for p in &profiles {
        if let Some(r) = &p.secret_reference {
            assert!(r.redacted().starts_with("env:") || r.redacted().starts_with("cli:"));
        }
    }

    pool.close().await;
}

// ============================================================
// Test 8 — restart persistence
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn state_survives_restart() {
    let _guard = AUTH_GUARD.lock().await;
    let pool = connect().await;
    reset_auth(&pool).await;
    let project_id = new_project(&pool, "prov-restart").await;
    let svc = service(&pool, CLI, true);
    register(
        &svc,
        RuntimeProvider::ClaudeCodeCli,
        AuthKind::SubscriptionLogin,
        "cli:claude",
    )
    .await;
    let mut policy = argus_core::provider::ProjectProviderPolicy::default_for(project_id);
    policy.subscription_first = false;
    policy.api_fallback = FallbackPolicy::Never;
    policy.max_tier = Some(ModelTier::T3);
    svc.upsert_policy(policy.clone()).await.unwrap();
    svc.route(input(project_id, ModelTier::T2, vec![]))
        .await
        .unwrap();

    // New pool + service (simulated restart).
    let pool2 = connect().await;
    let svc2 = service(&pool2, CLI, true);
    let reloaded = svc2.get_policy(project_id).await.unwrap();
    assert!(!reloaded.subscription_first);
    assert_eq!(reloaded.api_fallback, FallbackPolicy::Never);
    assert_eq!(reloaded.max_tier, Some(ModelTier::T3));
    let profiles = svc2.list_auth_profiles().await.unwrap();
    assert!(
        profiles
            .iter()
            .any(|p| p.provider_id == RuntimeProvider::ClaudeCodeCli)
    );
    let selections = svc2.list_selections(project_id, 10).await.unwrap();
    assert!(!selections.is_empty());

    pool.close().await;
    pool2.close().await;
}

// ============================================================
// Test 9 — discovery reflects installed runtimes; unavailable skipped
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn unavailable_runtime_is_skipped() {
    let _guard = AUTH_GUARD.lock().await;
    let pool = connect().await;
    reset_auth(&pool).await;
    let project_id = new_project(&pool, "prov-unavail").await;
    let svc = service(
        &pool,
        &[
            (RuntimeProvider::ClaudeCodeCli, false),
            (RuntimeProvider::CodexCli, true),
        ],
        true,
    );
    register(
        &svc,
        RuntimeProvider::ClaudeCodeCli,
        AuthKind::SubscriptionLogin,
        "cli:claude",
    )
    .await;
    register(
        &svc,
        RuntimeProvider::CodexCli,
        AuthKind::SubscriptionLogin,
        "cli:codex",
    )
    .await;

    let discovery = svc.discover_all().await;
    assert!(
        discovery
            .iter()
            .any(|(p, d)| *p == RuntimeProvider::ClaudeCodeCli && !d.installed)
    );

    let out = svc
        .route(input(project_id, ModelTier::T2, vec![]))
        .await
        .unwrap();
    assert_eq!(
        out.result.selected_target().unwrap().runtime_provider,
        RuntimeProvider::CodexCli
    );

    pool.close().await;
}

// ============================================================
// Test 10 — provider failover does not escalate tier
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn failover_keeps_required_tier() {
    let _guard = AUTH_GUARD.lock().await;
    let pool = connect().await;
    reset_auth(&pool).await;
    let project_id = new_project(&pool, "prov-failover").await;
    let svc = service(
        &pool,
        &[
            (RuntimeProvider::ClaudeCodeCli, false),
            (RuntimeProvider::CodexCli, true),
        ],
        true,
    );
    register(
        &svc,
        RuntimeProvider::ClaudeCodeCli,
        AuthKind::SubscriptionLogin,
        "cli:claude",
    )
    .await;
    register(
        &svc,
        RuntimeProvider::CodexCli,
        AuthKind::SubscriptionLogin,
        "cli:codex",
    )
    .await;

    let out = svc
        .route(input(
            project_id,
            ModelTier::T2,
            vec![CapabilityRequirement::ToolUse],
        ))
        .await
        .unwrap();
    let target = out.result.selected_target().unwrap();
    assert_eq!(target.runtime_provider, RuntimeProvider::CodexCli);
    assert_eq!(target.capability_tier, ModelTier::T2);

    pool.close().await;
}

// ============================================================
// Test 11 — a persisted project allow-list is a ceiling the caller cannot widen
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn caller_cannot_widen_project_policy() {
    let _guard = AUTH_GUARD.lock().await;
    let pool = connect().await;
    reset_auth(&pool).await;
    let project_id = new_project(&pool, "prov-ceiling").await;
    let svc = service(&pool, CLI, true);
    register(
        &svc,
        RuntimeProvider::ClaudeCodeCli,
        AuthKind::SubscriptionLogin,
        "cli:claude",
    )
    .await;
    register(
        &svc,
        RuntimeProvider::OpenAiApi,
        AuthKind::ApiKey,
        "env:OPENAI_API_KEY",
    )
    .await;

    // The project only ever allows the API.
    let mut policy = argus_core::provider::ProjectProviderPolicy::default_for(project_id);
    policy.allowed_providers = vec![RuntimeProvider::OpenAiApi];
    svc.upsert_policy(policy).await.unwrap();

    // A caller asking for the subscription CLI must not get it.
    let mut i = input(project_id, ModelTier::T2, vec![]);
    i.policy.allowed_providers = Some(vec![RuntimeProvider::ClaudeCodeCli]);
    let out = svc.route(i).await.unwrap();
    assert!(
        !matches!(out.result, ProviderRoutingResult::Selected { .. }),
        "a caller must not widen past the project allow-list"
    );

    // Asking for both still resolves to the API only.
    let mut i2 = input(project_id, ModelTier::T2, vec![]);
    i2.policy.allowed_providers = Some(vec![
        RuntimeProvider::ClaudeCodeCli,
        RuntimeProvider::OpenAiApi,
    ]);
    let out2 = svc.route(i2).await.unwrap();
    assert_eq!(
        out2.result.selected_target().unwrap().runtime_provider,
        RuntimeProvider::OpenAiApi
    );

    pool.close().await;
}

// ============================================================
// Test 12 — a stricter project fallback policy beats a looser caller
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn project_fallback_policy_wins() {
    let _guard = AUTH_GUARD.lock().await;
    let pool = connect().await;
    reset_auth(&pool).await;
    let project_id = new_project(&pool, "prov-fallback").await;
    let svc = service(&pool, CLI, true);
    let sub = register(
        &svc,
        RuntimeProvider::ClaudeCodeCli,
        AuthKind::SubscriptionLogin,
        "cli:claude",
    )
    .await;
    register(
        &svc,
        RuntimeProvider::OpenAiApi,
        AuthKind::ApiKey,
        "env:OPENAI_API_KEY",
    )
    .await;
    repo(&pool)
        .save_quota(exhausted(RuntimeProvider::ClaudeCodeCli, sub))
        .await
        .unwrap();

    // The project forbids API spend even though this caller would allow it.
    let mut policy = argus_core::provider::ProjectProviderPolicy::default_for(project_id);
    policy.api_fallback = FallbackPolicy::Never;
    svc.upsert_policy(policy).await.unwrap();

    let mut i = input(project_id, ModelTier::T2, vec![]);
    i.policy.allowed_providers = Some(vec![
        RuntimeProvider::ClaudeCodeCli,
        RuntimeProvider::OpenAiApi,
    ]);
    i.policy.fallback_policy = FallbackPolicy::AutomaticWithinBudget;
    let out = svc.route(i).await.unwrap();
    assert!(
        !matches!(out.result, ProviderRoutingResult::Selected { .. }),
        "project `Never` must not be relaxed by a permissive caller"
    );
    assert!(out.selection.is_none(), "no spend may be recorded");

    pool.close().await;
}

// ============================================================
// Test 13 — model deny list is applied during routing
// ============================================================

#[tokio::test]
#[ignore = "requires postgres"]
async fn denied_model_is_not_selected() {
    let _guard = AUTH_GUARD.lock().await;
    let pool = connect().await;
    reset_auth(&pool).await;
    let project_id = new_project(&pool, "prov-modeldeny").await;
    let svc = service(&pool, CLI, true);
    register(
        &svc,
        RuntimeProvider::ClaudeCodeCli,
        AuthKind::SubscriptionLogin,
        "cli:claude",
    )
    .await;
    register(
        &svc,
        RuntimeProvider::OpenAiApi,
        AuthKind::ApiKey,
        "env:OPENAI_API_KEY",
    )
    .await;

    // `claude-code-default` is Claude's only T2+ model, so denying it removes
    // Claude from the capable set without touching provider-level policy.
    let mut policy = argus_core::provider::ProjectProviderPolicy::default_for(project_id);
    policy.denied_models = vec!["claude-code-default".to_string()];
    svc.upsert_policy(policy).await.unwrap();

    let mut i = input(project_id, ModelTier::T2, vec![]);
    i.policy.allowed_providers = Some(vec![
        RuntimeProvider::ClaudeCodeCli,
        RuntimeProvider::OpenAiApi,
    ]);
    let out = svc.route(i).await.unwrap();
    let target = out.result.selected_target().expect("selected");
    assert_ne!(target.runtime_provider, RuntimeProvider::ClaudeCodeCli);
    assert_ne!(
        target.model_id.as_deref(),
        Some("claude-code-default"),
        "a denied model id must never be selected"
    );

    pool.close().await;
}
