//! V0.44 deterministic provider router.
//!
//! Filters by capability → tier → runtime → auth → known quota, then applies
//! policy (subscription-first, cost, health) with a stable tie-break. It never
//! lowers a tier for cost, never treats unknown quota as exhausted, and keeps
//! *provider failover* separate from *model tier escalation*.

use uuid::Uuid;

use crate::intelligence::{CapabilityRequirement, CostPreference, ModelTier, ReasoningEffort};

use super::types::*;

#[derive(Clone, Debug)]
pub struct ProviderRoutingInput {
    pub project_id: Uuid,
    pub routing_decision_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub role: String,
    pub purpose: String,
    pub required_tier: ModelTier,
    pub required_capabilities: Vec<CapabilityRequirement>,
    pub reasoning_effort: ReasoningEffort,
    pub estimated_cost_usd: Option<f64>,
    pub policy: SelectionPolicy,
}

#[derive(Clone, Debug)]
pub struct RouterAttempt {
    pub candidate_provider: RuntimeProvider,
    pub candidate_model: Option<String>,
    pub outcome: String,
    pub reason: String,
}

#[derive(Clone, Debug)]
pub struct RouterOutput {
    pub result: ProviderRoutingResult,
    pub attempts: Vec<RouterAttempt>,
}

struct Candidate {
    provider: RuntimeProvider,
    provider_priority: i32,
    preferred: bool,
    model_id: String,
    tier: ModelTier,
    billing: BillingMode,
    auth_profile_id: Option<Uuid>,
    quota_snapshot_id: Option<Uuid>,
    health: ProviderHealthStatus,
    execution_mode: ExecutionMode,
    reasons: Vec<String>,
}

impl Candidate {
    fn to_target(&self, reasoning: ReasoningEffort) -> ExecutionTarget {
        ExecutionTarget {
            runtime_provider: self.provider,
            provider_id: self.provider,
            model_id: Some(self.model_id.clone()),
            auth_profile_id: self.auth_profile_id,
            capability_tier: self.tier,
            reasoning_effort: reasoning,
            billing_mode: self.billing,
            execution_mode: self.execution_mode,
            quota_snapshot_id: self.quota_snapshot_id,
            selection_reasons: self.reasons.clone(),
        }
    }
}

pub struct ProviderRouter;

impl ProviderRouter {
    pub fn select(input: &ProviderRoutingInput, state: &ProviderState) -> RouterOutput {
        let mut attempts = Vec::new();
        let mut subscription_candidates: Vec<Candidate> = Vec::new();
        let mut api_candidates: Vec<Candidate> = Vec::new();
        let mut manual_candidates: Vec<Candidate> = Vec::new();
        let mut need_login: Vec<(RuntimeProvider, Option<Uuid>)> = Vec::new();
        let mut wait_quota: Vec<(RuntimeProvider, Option<chrono::DateTime<chrono::Utc>>)> =
            Vec::new();
        let mut subscription_excluded = false;
        let mut saw_capable_provider = false;
        let mut runtime_missing: Vec<RuntimeProvider> = Vec::new();

        for provider in &state.providers {
            let supports_subscription = provider
                .billing_modes
                .contains(&BillingMode::SubscriptionIncluded);

            if !provider.enabled {
                attempts.push(attempt(
                    provider.provider_id,
                    None,
                    "policy_denied",
                    "provider disabled",
                ));
                continue;
            }
            let outside_allow_list = input
                .policy
                .allowed_providers
                .as_ref()
                .is_some_and(|list| !list.contains(&provider.provider_id));
            if outside_allow_list
                || input
                    .policy
                    .denied_providers
                    .contains(&provider.provider_id)
            {
                attempts.push(attempt(
                    provider.provider_id,
                    None,
                    "policy_denied",
                    "provider not allowed by policy",
                ));
                if supports_subscription {
                    subscription_excluded = true;
                }
                continue;
            }

            // Runtime installed?
            let needs_runtime = matches!(provider.kind, ProviderKind::Cli);
            if needs_runtime {
                let installed = state
                    .discovery_for(provider.provider_id)
                    .map(|d| d.installed)
                    .unwrap_or(false);
                if !installed {
                    attempts.push(attempt(
                        provider.provider_id,
                        None,
                        "runtime_missing",
                        "runtime binary not installed",
                    ));
                    runtime_missing.push(provider.provider_id);
                    if supports_subscription {
                        subscription_excluded = true;
                    }
                    continue;
                }
            }

            for model in state.models_for(provider.provider_id) {
                if !model.availability {
                    attempts.push(attempt(
                        provider.provider_id,
                        Some(model.model_id.clone()),
                        "capability_mismatch",
                        "model unavailable",
                    ));
                    continue;
                }
                if !input.policy.model_permitted(&model.model_id) {
                    attempts.push(attempt(
                        provider.provider_id,
                        Some(model.model_id.clone()),
                        "policy_denied",
                        "model denied by policy",
                    ));
                    continue;
                }
                if model.capability_tier < input.required_tier {
                    attempts.push(attempt(
                        provider.provider_id,
                        Some(model.model_id.clone()),
                        "capability_mismatch",
                        "model tier below required tier",
                    ));
                    continue;
                }
                if let Some(max) = input.policy.max_tier {
                    if model.capability_tier > max {
                        attempts.push(attempt(
                            provider.provider_id,
                            Some(model.model_id.clone()),
                            "policy_denied",
                            "model tier above policy max_tier",
                        ));
                        continue;
                    }
                }
                if !provider.allowed_roles.is_empty()
                    && !provider.allowed_roles.contains(&input.role)
                {
                    continue;
                }
                if !model.supported_roles.is_empty() && !model.supported_roles.contains(&input.role)
                {
                    continue;
                }
                if !model.supported_purposes.is_empty()
                    && !model.supported_purposes.contains(&input.purpose)
                {
                    continue;
                }

                let effective = model.effective_capabilities(provider);
                let missing: Vec<&CapabilityRequirement> = input
                    .required_capabilities
                    .iter()
                    .filter(|c| !effective.contains(c))
                    .collect();
                if !missing.is_empty() {
                    attempts.push(attempt(
                        provider.provider_id,
                        Some(model.model_id.clone()),
                        "capability_mismatch",
                        &format!(
                            "missing capabilities: {}",
                            missing
                                .iter()
                                .map(|c| c.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    ));
                    continue;
                }
                if !model.reasoning_levels.is_empty()
                    && !model.reasoning_levels.contains(&input.reasoning_effort)
                {
                    attempts.push(attempt(
                        provider.provider_id,
                        Some(model.model_id.clone()),
                        "capability_mismatch",
                        "model does not support the required reasoning effort",
                    ));
                    continue;
                }

                saw_capable_provider = true;

                // Auth.
                let auths = state.auth_for(provider.provider_id);
                let available_auth = auths
                    .iter()
                    .copied()
                    .find(|a| a.status == AuthStatus::Available);
                let requires_auth = !matches!(
                    provider.provider_id,
                    RuntimeProvider::ManualWeb | RuntimeProvider::Test
                );
                if requires_auth && available_auth.is_none() {
                    let login = auths
                        .iter()
                        .copied()
                        .find(|a| matches!(a.status, AuthStatus::NeedsLogin | AuthStatus::Expired))
                        .map(|a| a.auth_profile_id);
                    if login.is_some() {
                        need_login.push((provider.provider_id, login));
                    } else {
                        attempts.push(attempt(
                            provider.provider_id,
                            Some(model.model_id.clone()),
                            "auth_unavailable",
                            "no available auth profile",
                        ));
                    }
                    if supports_subscription {
                        subscription_excluded = true;
                    }
                    continue;
                }

                let billing = match available_auth {
                    Some(a) => a.auth_kind.billing_mode(),
                    None => provider
                        .billing_modes
                        .first()
                        .copied()
                        .unwrap_or(BillingMode::Unknown),
                };

                // Health.
                let health = state.health_for(provider.provider_id);
                if health == ProviderHealthStatus::Unavailable {
                    attempts.push(attempt(
                        provider.provider_id,
                        Some(model.model_id.clone()),
                        "provider_unhealthy",
                        "provider is unavailable",
                    ));
                    if supports_subscription {
                        subscription_excluded = true;
                    }
                    continue;
                }

                // Quota.
                let quota = state.latest_quota(
                    provider.provider_id,
                    available_auth.map(|a| a.auth_profile_id),
                );
                let quota_snapshot_id = quota.and_then(|q| {
                    if q.is_known_exhausted() {
                        None
                    } else {
                        Some(q.quota_snapshot_id)
                    }
                });
                if let Some(q) = quota {
                    if q.is_known_exhausted() {
                        attempts.push(attempt(
                            provider.provider_id,
                            Some(model.model_id.clone()),
                            "quota_unavailable",
                            "known quota exhausted",
                        ));
                        wait_quota.push((provider.provider_id, q.reset_at));
                        if supports_subscription {
                            subscription_excluded = true;
                        }
                        continue;
                    }
                }

                let mut reasons = vec![
                    format!(
                        "capability tier {} satisfies required {}",
                        model.capability_tier, input.required_tier
                    ),
                    "required capabilities satisfied".to_string(),
                ];
                if billing == BillingMode::SubscriptionIncluded {
                    reasons.push("subscription allowance preferred".to_string());
                }
                match quota.map(|q| q.availability) {
                    Some(QuotaAvailability::Available) => {
                        reasons.push("quota available".to_string())
                    }
                    Some(QuotaAvailability::Unknown) | None => {
                        reasons.push("quota unknown; best-effort".to_string())
                    }
                    Some(QuotaAvailability::Unavailable) => {}
                }
                reasons.push(match health {
                    ProviderHealthStatus::Healthy => "provider healthy".to_string(),
                    ProviderHealthStatus::Degraded => "provider degraded".to_string(),
                    _ => "provider health unknown".to_string(),
                });

                let candidate = Candidate {
                    provider: provider.provider_id,
                    provider_priority: provider.priority,
                    preferred: input.policy.preferred_runtime == Some(provider.provider_id),
                    model_id: model.model_id.clone(),
                    tier: model.capability_tier,
                    billing,
                    auth_profile_id: available_auth.map(|a| a.auth_profile_id),
                    quota_snapshot_id,
                    health,
                    execution_mode: provider.execution_mode,
                    reasons,
                };
                match billing {
                    BillingMode::SubscriptionIncluded => subscription_candidates.push(candidate),
                    BillingMode::ApiMetered | BillingMode::Credits => {
                        api_candidates.push(candidate)
                    }
                    BillingMode::LocalCompute | BillingMode::Unknown => {
                        manual_candidates.push(candidate)
                    }
                }
            }
        }

        // Category preference.
        let prefer_api = input.policy.subscription_preference == SubscriptionPreference::PreferApi;
        let (primary, secondary) = if prefer_api {
            (&mut api_candidates, &mut subscription_candidates)
        } else {
            (&mut subscription_candidates, &mut api_candidates)
        };
        sort_candidates(primary, input.policy.cost_preference);
        sort_candidates(secondary, input.policy.cost_preference);
        sort_candidates(&mut manual_candidates, input.policy.cost_preference);

        if let Some(best) = primary.first() {
            return RouterOutput {
                result: ProviderRoutingResult::Selected {
                    target: best.to_target(input.reasoning_effort),
                },
                attempts,
            };
        }
        if let Some(best) = secondary.first() {
            let is_fallback = subscription_excluded;
            if is_fallback && input.policy.fallback_policy == FallbackPolicy::Ask {
                return RouterOutput {
                    result: ProviderRoutingResult::NeedsApproval {
                        target: best.to_target(input.reasoning_effort),
                        reason: "subscription unavailable; API fallback requires approval"
                            .to_string(),
                    },
                    attempts,
                };
            }
            if is_fallback && input.policy.fallback_policy == FallbackPolicy::Never {
                return RouterOutput {
                    result: ProviderRoutingResult::NoCapableProvider {
                        reasons: vec![
                            "subscription unavailable and API fallback is disabled".to_string(),
                        ],
                    },
                    attempts,
                };
            }
            return RouterOutput {
                result: ProviderRoutingResult::Selected {
                    target: best.to_target(input.reasoning_effort),
                },
                attempts,
            };
        }
        // Auth/quota problems surface before a manual fallback, so an operator
        // is told to log in rather than silently switching to manual planning.
        if let Some((provider, profile)) = need_login.first() {
            return RouterOutput {
                result: ProviderRoutingResult::NeedsLogin {
                    provider: *provider,
                    auth_profile_id: *profile,
                    action: "login via the official CLI/OAuth flow".to_string(),
                },
                attempts,
            };
        }
        if let Some((provider, reset_at)) = wait_quota.first() {
            return RouterOutput {
                result: ProviderRoutingResult::WaitForQuota {
                    provider: *provider,
                    reset_at: *reset_at,
                    reason: "quota exhausted".to_string(),
                },
                attempts,
            };
        }
        if let Some(best) = manual_candidates.first() {
            // Manual providers are selected last; the caller exports a packet
            // and never auto-invokes anything.
            return RouterOutput {
                result: ProviderRoutingResult::Selected {
                    target: best.to_target(input.reasoning_effort),
                },
                attempts,
            };
        }
        if !saw_capable_provider {
            // Report the runtime that is actually missing rather than naming a
            // provider that was never involved in this route.
            if let Some(provider) = runtime_missing.first().copied() {
                return RouterOutput {
                    result: ProviderRoutingResult::ProviderUnavailable {
                        provider,
                        reason: format!(
                            "{provider} runtime is not installed; {} other runtime(s) also missing",
                            runtime_missing.len() - 1
                        ),
                    },
                    attempts,
                };
            }
        }
        RouterOutput {
            result: ProviderRoutingResult::NoCapableProvider {
                reasons: vec![
                    "no provider/model satisfies the required tier and capabilities".to_string(),
                ],
            },
            attempts,
        }
    }
}

fn attempt(
    provider: RuntimeProvider,
    model: Option<String>,
    outcome: &str,
    reason: &str,
) -> RouterAttempt {
    RouterAttempt {
        candidate_provider: provider,
        candidate_model: model,
        outcome: outcome.to_string(),
        reason: reason.to_string(),
    }
}

fn sort_candidates(candidates: &mut [Candidate], cost: CostPreference) {
    candidates.sort_by_key(|c| candidate_key(c, cost));
}

fn candidate_key(
    c: &Candidate,
    cost: CostPreference,
) -> (u8, u8, u8, u8, i32, RuntimeProvider, String, String) {
    let preferred_rank = if c.preferred { 0 } else { 1 };
    let billing_rank = match c.billing {
        BillingMode::SubscriptionIncluded | BillingMode::LocalCompute | BillingMode::Credits => 0,
        BillingMode::ApiMetered => 1,
        BillingMode::Unknown => 2,
    };
    let health = c.health.rank();
    let tier = c.tier.rank();
    let (cost_rank, tier_rank) = match cost {
        CostPreference::QualityFirst => (0, u8::MAX - tier),
        CostPreference::CheapestSufficient | CostPreference::MinimizeCost => (billing_rank, tier),
        CostPreference::Balanced => (billing_rank, tier),
    };
    (
        preferred_rank,
        cost_rank,
        tier_rank,
        health,
        c.provider_priority,
        c.provider,
        c.model_id.clone(),
        c.auth_profile_id
            .map(|id| id.to_string())
            .unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::registry::ProviderRegistry;

    fn state_with_fixture() -> ProviderState {
        let registry = ProviderRegistry::new_default();
        let mut state = ProviderState {
            providers: registry.providers().to_vec(),
            models: registry.models().to_vec(),
            ..Default::default()
        };
        // All CLI runtimes installed.
        for p in [
            RuntimeProvider::ClaudeCodeCli,
            RuntimeProvider::CodexCli,
            RuntimeProvider::OpenCodeCli,
        ] {
            state.discovery.push((
                p,
                ProviderDiscovery {
                    installed: true,
                    ..Default::default()
                },
            ));
        }
        state
    }

    fn subscription_auth(state: &mut ProviderState, provider: RuntimeProvider) -> Uuid {
        let id = Uuid::new_v4();
        state.auth_profiles.push(AuthProfile {
            auth_profile_id: id,
            provider_id: provider,
            auth_kind: AuthKind::SubscriptionLogin,
            account_label: Some("sub".to_string()),
            workspace_label: None,
            status: AuthStatus::Available,
            secret_reference: Some(SecretReference::cli_managed(provider.as_str())),
            credential_source: Some("cli-managed".to_string()),
            expires_at: None,
            last_verified_at: None,
            metadata: serde_json::json!({}),
            created_at: chrono::Utc::now(),
        });
        id
    }

    fn api_auth(state: &mut ProviderState, provider: RuntimeProvider) -> Uuid {
        let id = Uuid::new_v4();
        state.auth_profiles.push(AuthProfile {
            auth_profile_id: id,
            provider_id: provider,
            auth_kind: AuthKind::ApiKey,
            account_label: Some("api".to_string()),
            workspace_label: None,
            status: AuthStatus::Available,
            secret_reference: Some(SecretReference::env("API_KEY")),
            credential_source: Some("env".to_string()),
            expires_at: None,
            last_verified_at: None,
            metadata: serde_json::json!({}),
            created_at: chrono::Utc::now(),
        });
        id
    }

    fn input(tier: ModelTier, caps: Vec<CapabilityRequirement>) -> ProviderRoutingInput {
        ProviderRoutingInput {
            project_id: Uuid::nil(),
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

    #[test]
    fn subscription_preferred_when_capable() {
        let mut state = state_with_fixture();
        subscription_auth(&mut state, RuntimeProvider::ClaudeCodeCli);
        api_auth(&mut state, RuntimeProvider::OpenAiApi);
        let out = ProviderRouter::select(&input(ModelTier::T2, vec![]), &state);
        let target = out.result.selected_target().unwrap();
        assert_eq!(target.runtime_provider, RuntimeProvider::ClaudeCodeCli);
        assert_eq!(target.billing_mode, BillingMode::SubscriptionIncluded);
    }

    #[test]
    fn capability_beats_subscription() {
        let mut state = state_with_fixture();
        // OpenCode (subscription) is capable at T2 but lacks SecurityReasoning;
        // the API (metered) T3 model has it.
        subscription_auth(&mut state, RuntimeProvider::OpenCodeCli);
        api_auth(&mut state, RuntimeProvider::OpenAiApi);
        let mut i = input(
            ModelTier::T2,
            vec![CapabilityRequirement::SecurityReasoning],
        );
        i.policy.allowed_providers = Some(vec![
            RuntimeProvider::OpenCodeCli,
            RuntimeProvider::OpenAiApi,
        ]);
        let out = ProviderRouter::select(&i, &state);
        let target = out.result.selected_target().unwrap();
        assert_ne!(target.billing_mode, BillingMode::SubscriptionIncluded);
        assert_eq!(target.runtime_provider, RuntimeProvider::OpenAiApi);
    }

    #[test]
    fn subscription_exhausted_falls_back_when_automatic() {
        let mut state = state_with_fixture();
        let sub = subscription_auth(&mut state, RuntimeProvider::ClaudeCodeCli);
        api_auth(&mut state, RuntimeProvider::OpenAiApi);
        state
            .quota
            .push(exhausted_quota(RuntimeProvider::ClaudeCodeCli, sub));
        let out = ProviderRouter::select(&input(ModelTier::T2, vec![]), &state);
        assert_eq!(
            out.result.selected_target().unwrap().runtime_provider,
            RuntimeProvider::OpenAiApi
        );
    }

    #[test]
    fn ask_before_api_fallback() {
        let mut state = state_with_fixture();
        let sub = subscription_auth(&mut state, RuntimeProvider::ClaudeCodeCli);
        api_auth(&mut state, RuntimeProvider::OpenAiApi);
        state
            .quota
            .push(exhausted_quota(RuntimeProvider::ClaudeCodeCli, sub));
        let mut i = input(ModelTier::T2, vec![]);
        i.policy.fallback_policy = FallbackPolicy::Ask;
        let out = ProviderRouter::select(&i, &state);
        assert!(matches!(
            out.result,
            ProviderRoutingResult::NeedsApproval { .. }
        ));
    }

    #[test]
    fn never_falls_back_to_api() {
        let mut state = state_with_fixture();
        let sub = subscription_auth(&mut state, RuntimeProvider::ClaudeCodeCli);
        api_auth(&mut state, RuntimeProvider::OpenAiApi);
        state
            .quota
            .push(exhausted_quota(RuntimeProvider::ClaudeCodeCli, sub));
        let mut i = input(ModelTier::T2, vec![]);
        i.policy.fallback_policy = FallbackPolicy::Never;
        let out = ProviderRouter::select(&i, &state);
        assert!(matches!(
            out.result,
            ProviderRoutingResult::NoCapableProvider { .. }
                | ProviderRoutingResult::WaitForQuota { .. }
        ));
    }

    #[test]
    fn unknown_quota_is_not_exhausted() {
        let mut state = state_with_fixture();
        let sub = subscription_auth(&mut state, RuntimeProvider::ClaudeCodeCli);
        state.quota.push(QuotaSnapshot {
            quota_snapshot_id: Uuid::new_v4(),
            provider_id: RuntimeProvider::ClaudeCodeCli,
            auth_profile_id: Some(sub),
            quota_kind: QuotaKind::SubscriptionAllowance,
            availability: QuotaAvailability::Unknown,
            remaining: None,
            quota_limit: None,
            unit: "unknown".to_string(),
            reset_at: None,
            observed_at: chrono::Utc::now(),
            source: QuotaSource::CliStatus,
            confidence: QuotaConfidence::Low,
            raw_metadata: serde_json::json!({}),
        });
        let out = ProviderRouter::select(&input(ModelTier::T2, vec![]), &state);
        assert_eq!(
            out.result.selected_target().unwrap().runtime_provider,
            RuntimeProvider::ClaudeCodeCli
        );
    }

    #[test]
    fn auth_missing_returns_needs_login() {
        let mut state = state_with_fixture();
        state.auth_profiles.push(AuthProfile {
            auth_profile_id: Uuid::new_v4(),
            provider_id: RuntimeProvider::ClaudeCodeCli,
            auth_kind: AuthKind::SubscriptionLogin,
            account_label: None,
            workspace_label: None,
            status: AuthStatus::NeedsLogin,
            secret_reference: Some(SecretReference::cli_managed("claude")),
            credential_source: Some("cli-managed".to_string()),
            expires_at: None,
            last_verified_at: None,
            metadata: serde_json::json!({}),
            created_at: chrono::Utc::now(),
        });
        let out = ProviderRouter::select(&input(ModelTier::T2, vec![]), &state);
        assert!(matches!(
            out.result,
            ProviderRoutingResult::NeedsLogin { .. }
        ));
    }

    #[test]
    fn unhealthy_provider_is_skipped() {
        let mut state = state_with_fixture();
        subscription_auth(&mut state, RuntimeProvider::ClaudeCodeCli);
        subscription_auth(&mut state, RuntimeProvider::CodexCli);
        state.health.push(ProviderHealth {
            provider_id: Some(RuntimeProvider::ClaudeCodeCli),
            status: Some(ProviderHealthStatus::Unavailable),
            ..Default::default()
        });
        let out = ProviderRouter::select(&input(ModelTier::T2, vec![]), &state);
        assert_eq!(
            out.result.selected_target().unwrap().runtime_provider,
            RuntimeProvider::CodexCli
        );
    }

    #[test]
    fn failover_does_not_escalate_tier() {
        let mut state = state_with_fixture();
        subscription_auth(&mut state, RuntimeProvider::ClaudeCodeCli);
        subscription_auth(&mut state, RuntimeProvider::CodexCli);
        state.health.push(ProviderHealth {
            provider_id: Some(RuntimeProvider::ClaudeCodeCli),
            status: Some(ProviderHealthStatus::Unavailable),
            ..Default::default()
        });
        let out = ProviderRouter::select(
            &input(ModelTier::T2, vec![CapabilityRequirement::ToolUse]),
            &state,
        );
        let target = out.result.selected_target().unwrap();
        assert_eq!(target.capability_tier, ModelTier::T2);
        assert_eq!(target.runtime_provider, RuntimeProvider::CodexCli);
    }

    #[test]
    fn denied_model_is_skipped() {
        let mut state = state_with_fixture();
        subscription_auth(&mut state, RuntimeProvider::ClaudeCodeCli);
        api_auth(&mut state, RuntimeProvider::OpenAiApi);
        let mut i = input(ModelTier::T2, vec![]);
        i.policy.allowed_providers = Some(vec![RuntimeProvider::ClaudeCodeCli]);
        // Claude's only T2+ model is denied, leaving nothing capable in project.
        i.policy.denied_models = vec!["claude-code-default".to_string()];
        let out = ProviderRouter::select(&i, &state);
        assert!(
            !matches!(out.result, ProviderRoutingResult::Selected { .. }),
            "denying the only capable model must remove the provider"
        );
    }

    #[test]
    fn missing_runtime_names_the_actual_provider() {
        let mut state = state_with_fixture();
        subscription_auth(&mut state, RuntimeProvider::ClaudeCodeCli);
        // Nothing is installed.
        state.discovery.clear();
        let mut i = input(ModelTier::T2, vec![]);
        i.policy.allowed_providers = Some(vec![RuntimeProvider::ClaudeCodeCli]);
        let out = ProviderRouter::select(&i, &state);
        match out.result {
            ProviderRoutingResult::ProviderUnavailable { provider, .. } => {
                assert_eq!(
                    provider,
                    RuntimeProvider::ClaudeCodeCli,
                    "the report must name the runtime that is actually missing"
                );
            }
            other => panic!("expected ProviderUnavailable, got {other:?}"),
        }
    }

    #[test]
    fn determinism_same_state_same_selection() {
        let mut state = state_with_fixture();
        subscription_auth(&mut state, RuntimeProvider::ClaudeCodeCli);
        subscription_auth(&mut state, RuntimeProvider::CodexCli);
        let i = input(ModelTier::T2, vec![]);
        let a = ProviderRouter::select(&i, &state);
        let b = ProviderRouter::select(&i, &state);
        assert_eq!(
            a.result.selected_target().cloned(),
            b.result.selected_target().cloned()
        );
    }

    fn exhausted_quota(provider: RuntimeProvider, auth: Uuid) -> QuotaSnapshot {
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
}
