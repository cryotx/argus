//! V0.44 ProviderService — builds routing state, runs the router, persists
//! selections/attempts, refreshes quota, and gates API fallback through the
//! existing BudgetService.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::budget_service::{BudgetDecision, BudgetService};
use crate::types::ArgusResult;

use super::adapter::ProviderAdapter;
use super::quota::QuotaProvider;
use super::registry::ProviderRegistry;
use super::repository::ProviderRepository;
use super::router::{ProviderRouter, ProviderRoutingInput};
use super::types::*;

const DISCOVERY_TTL: Duration = Duration::from_secs(60);

/// Result of a budget preflight, abstracted so routing can be tested without a
/// database.
#[derive(Clone, Debug, PartialEq)]
pub enum BudgetGateDecision {
    Allowed,
    Blocked(String),
    CheckFailed(String),
}

#[async_trait]
pub trait BudgetGate: Send + Sync {
    async fn preflight(
        &self,
        project_id: Uuid,
        task_id: Option<Uuid>,
        estimated_cost_usd: Option<f64>,
    ) -> BudgetGateDecision;
}

/// Adapter over the real V0.37 [`BudgetService`]. API fallback can never bypass
/// budget enforcement.
#[async_trait]
impl BudgetGate for BudgetService {
    async fn preflight(
        &self,
        project_id: Uuid,
        task_id: Option<Uuid>,
        estimated_cost_usd: Option<f64>,
    ) -> BudgetGateDecision {
        match self
            .preflight_check(
                project_id,
                task_id.unwrap_or_else(Uuid::nil),
                estimated_cost_usd,
            )
            .await
        {
            BudgetDecision::Allowed { .. } => BudgetGateDecision::Allowed,
            BudgetDecision::Blocked { reason, .. } => BudgetGateDecision::Blocked(reason),
            BudgetDecision::CheckFailed { error } => BudgetGateDecision::CheckFailed(error),
        }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct RoutingOutcome {
    pub result: ProviderRoutingResult,
    pub selection: Option<ProviderRoutingSelection>,
}

pub struct ProviderService {
    repo: Arc<dyn ProviderRepository>,
    quota: Arc<dyn QuotaProvider>,
    budget: Option<Arc<dyn BudgetGate>>,
    adapters: HashMap<RuntimeProvider, Arc<dyn ProviderAdapter>>,
    registry: RwLock<Option<Arc<ProviderRegistry>>>,
    discovery_cache: RwLock<HashMap<RuntimeProvider, ProviderDiscovery>>,
}

impl ProviderService {
    pub fn new(
        repo: Arc<dyn ProviderRepository>,
        quota: Arc<dyn QuotaProvider>,
        budget: Option<Arc<dyn BudgetGate>>,
        adapters: Vec<Arc<dyn ProviderAdapter>>,
    ) -> Self {
        let mut map = HashMap::new();
        for a in adapters {
            map.insert(a.id(), a);
        }
        Self {
            repo,
            quota,
            budget,
            adapters: map,
            registry: RwLock::new(None),
            discovery_cache: RwLock::new(HashMap::new()),
        }
    }

    pub async fn registry(&self) -> Arc<ProviderRegistry> {
        if let Some(r) = self.registry.read().await.as_ref() {
            return r.clone();
        }
        let mut registry = ProviderRegistry::new_default();
        if let Ok(overrides) = self.repo.list_provider_overrides().await {
            registry.apply_overrides(&overrides);
        }
        let arc = Arc::new(registry);
        *self.registry.write().await = Some(arc.clone());
        arc
    }

    /// Force the registry to reload (after a config override).
    pub async fn reload_registry(&self) {
        *self.registry.write().await = None;
    }

    async fn discovery(&self, provider: RuntimeProvider) -> ProviderDiscovery {
        if let Some(d) = self.discovery_cache.read().await.get(&provider) {
            if d.checked_at
                .map(|t| {
                    chrono::Utc::now()
                        .signed_duration_since(t)
                        .to_std()
                        .unwrap_or_default()
                        < DISCOVERY_TTL
                })
                .unwrap_or(false)
            {
                return d.clone();
            }
        }
        let discovery = match self.adapters.get(&provider) {
            Some(adapter) => adapter.discover().await,
            None if provider.cli_binary().is_none() => ProviderDiscovery {
                installed: true,
                checked_at: Some(chrono::Utc::now()),
                ..Default::default()
            },
            None => ProviderDiscovery {
                installed: false,
                checked_at: Some(chrono::Utc::now()),
                note: Some("no adapter registered".to_string()),
                ..Default::default()
            },
        };
        self.discovery_cache
            .write()
            .await
            .insert(provider, discovery.clone());
        discovery
    }

    /// Discover every CLI provider (safe: binary/version only).
    pub async fn discover_all(&self) -> Vec<(RuntimeProvider, ProviderDiscovery)> {
        let registry = self.registry().await;
        let mut out = Vec::new();
        for p in registry.providers() {
            if p.provider_id.cli_binary().is_some() {
                out.push((p.provider_id, self.discovery(p.provider_id).await));
            }
        }
        out
    }

    pub async fn state(&self) -> ArgusResult<ProviderState> {
        let registry = self.registry().await;
        let auth_profiles = self.repo.list_auth_profiles().await?;

        let mut quota = Vec::new();
        for a in &auth_profiles {
            if let Some(q) = self
                .repo
                .latest_quota(a.provider_id, Some(a.auth_profile_id))
                .await?
            {
                quota.push(q);
            }
        }
        for p in registry.providers() {
            if let Some(q) = self.repo.latest_quota(p.provider_id, None).await? {
                quota.push(q);
            }
        }

        let mut discovery = Vec::new();
        let mut health = Vec::new();
        for p in registry.providers() {
            if p.provider_id.cli_binary().is_some() {
                let d = self.discovery(p.provider_id).await;
                health.push(ProviderHealth {
                    provider_id: Some(p.provider_id),
                    status: Some(if d.installed {
                        ProviderHealthStatus::Healthy
                    } else {
                        ProviderHealthStatus::Unavailable
                    }),
                    auth_status: Some(d.auth_status),
                    checked_at: d.checked_at,
                    ..Default::default()
                });
                discovery.push((p.provider_id, d));
            } else {
                health.push(ProviderHealth {
                    provider_id: Some(p.provider_id),
                    status: Some(ProviderHealthStatus::Unknown),
                    ..Default::default()
                });
            }
        }

        Ok(ProviderState {
            providers: registry.providers().to_vec(),
            models: registry.models().to_vec(),
            auth_profiles,
            quota,
            health,
            discovery,
            attempts: Vec::new(),
        })
    }

    /// Resolve the caller's policy against the persisted project policy.
    ///
    /// The project policy is authoritative: a caller may narrow it, never widen
    /// it. Done here rather than at each entrypoint so no call path can skip it.
    pub async fn resolve_policy(
        &self,
        project_id: Uuid,
        caller: &SelectionPolicy,
    ) -> ArgusResult<SelectionPolicy> {
        let project = self
            .repo
            .get_policy(project_id)
            .await?
            .unwrap_or_else(|| ProjectProviderPolicy::default_for(project_id));
        Ok(project.resolve(caller))
    }

    pub async fn route(&self, input: ProviderRoutingInput) -> ArgusResult<RoutingOutcome> {
        let policy = self.resolve_policy(input.project_id, &input.policy).await?;
        let input = ProviderRoutingInput { policy, ..input };
        let state = self.state().await?;
        let output = ProviderRouter::select(&input, &state);

        for attempt in &output.attempts {
            let _ = self
                .repo
                .record_attempt(NewRoutingAttempt {
                    project_id: input.project_id,
                    routing_decision_id: input.routing_decision_id,
                    candidate_provider: attempt.candidate_provider,
                    candidate_model: attempt.candidate_model.clone(),
                    outcome: attempt.outcome.clone(),
                    reason: attempt.reason.clone(),
                })
                .await;
        }

        let mut result = output.result;

        // Budget gate: API-billed targets must pass the V0.37 BudgetService.
        // Credits are metered spend too, so they gate identically — a provider
        // fallback must never become a way around budget enforcement.
        if let ProviderRoutingResult::Selected { target } = &result {
            if matches!(
                target.billing_mode,
                BillingMode::ApiMetered | BillingMode::Credits
            ) {
                if let Some(gate) = &self.budget {
                    match gate
                        .preflight(input.project_id, input.task_id, input.estimated_cost_usd)
                        .await
                    {
                        BudgetGateDecision::Allowed => {}
                        BudgetGateDecision::Blocked(reason)
                        | BudgetGateDecision::CheckFailed(reason) => {
                            result = ProviderRoutingResult::BudgetBlocked {
                                target: target.clone(),
                                reason,
                            };
                        }
                    }
                }
            }
        }

        let selection = match &result {
            ProviderRoutingResult::Selected { target } => Some(
                self.repo
                    .save_selection(NewRoutingSelection {
                        project_id: input.project_id,
                        routing_decision_id: input.routing_decision_id,
                        target: target.clone(),
                        required_tier: input.required_tier,
                        reasoning_effort: input.reasoning_effort,
                        policy: input.policy.clone(),
                    })
                    .await?,
            ),
            _ => None,
        };

        Ok(RoutingOutcome { result, selection })
    }

    /// Refresh (or record unknown) quota for a provider/auth profile.
    pub async fn refresh_quota(
        &self,
        provider: RuntimeProvider,
        auth_profile_id: Option<Uuid>,
    ) -> ArgusResult<QuotaSnapshot> {
        let snapshot = match self.quota.refresh(provider, auth_profile_id).await {
            Some(s) => s,
            None => super::quota::unknown_snapshot(provider, auth_profile_id),
        };
        self.repo.save_quota(snapshot).await
    }

    pub async fn ensure_auth_profile(&self, profile: NewAuthProfile) -> ArgusResult<AuthProfile> {
        self.repo.upsert_auth_profile(profile).await
    }

    pub async fn latest_quota(
        &self,
        provider: RuntimeProvider,
        auth_profile_id: Option<Uuid>,
    ) -> ArgusResult<Option<QuotaSnapshot>> {
        self.repo.latest_quota(provider, auth_profile_id).await
    }

    pub async fn list_auth_profiles(&self) -> ArgusResult<Vec<AuthProfile>> {
        self.repo.list_auth_profiles().await
    }

    pub async fn set_auth_status(&self, id: Uuid, status: AuthStatus) -> ArgusResult<()> {
        self.repo.set_auth_status(id, status).await
    }

    pub async fn get_policy(&self, project_id: Uuid) -> ArgusResult<ProjectProviderPolicy> {
        Ok(self
            .repo
            .get_policy(project_id)
            .await?
            .unwrap_or_else(|| ProjectProviderPolicy::default_for(project_id)))
    }

    pub async fn upsert_policy(
        &self,
        policy: ProjectProviderPolicy,
    ) -> ArgusResult<ProjectProviderPolicy> {
        self.repo.upsert_policy(policy).await
    }

    pub async fn list_selections(
        &self,
        project_id: Uuid,
        limit: i64,
    ) -> ArgusResult<Vec<ProviderRoutingSelection>> {
        self.repo.list_selections(project_id, limit).await
    }

    pub async fn list_attempts(
        &self,
        project_id: Uuid,
        limit: i64,
    ) -> ArgusResult<Vec<ProviderRoutingAttempt>> {
        self.repo.list_attempts(project_id, limit).await
    }
}

/// Deterministic budget gate for tests.
pub struct FakeBudgetGate {
    pub allow: bool,
    pub reason: String,
}

#[async_trait]
impl BudgetGate for FakeBudgetGate {
    async fn preflight(
        &self,
        _project_id: Uuid,
        _task_id: Option<Uuid>,
        _estimated_cost_usd: Option<f64>,
    ) -> BudgetGateDecision {
        if self.allow {
            BudgetGateDecision::Allowed
        } else {
            BudgetGateDecision::Blocked(self.reason.clone())
        }
    }
}
