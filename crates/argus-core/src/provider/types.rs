//! V0.44 Provider / Auth / Subscription / Quota domain types.
//!
//! Role != Model != Provider != Auth != Subscription != API billing. A tier is
//! a capability requirement (V0.42); this layer maps it onto a concrete
//! execution target across providers, auth profiles, and quota state. It never
//! stores or returns a raw credential.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use uuid::Uuid;

use crate::intelligence::{CapabilityRequirement, CostPreference, ModelTier, ReasoningEffort};

pub const PROVIDER_ROUTER_VERSION: &str = "provider-router-v1";

// ============================================================
// Runtime providers
// ============================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeProvider {
    ClaudeCodeCli,
    CodexCli,
    OpenCodeCli,
    OpenAiApi,
    AnthropicApi,
    ManualWeb,
    Test,
}

impl RuntimeProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeCodeCli => "claude_code_cli",
            Self::CodexCli => "codex_cli",
            Self::OpenCodeCli => "opencode_cli",
            Self::OpenAiApi => "open_ai_api",
            Self::AnthropicApi => "anthropic_api",
            Self::ManualWeb => "manual_web",
            Self::Test => "test",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "claude_code_cli" => Self::ClaudeCodeCli,
            "codex_cli" => Self::CodexCli,
            "opencode_cli" => Self::OpenCodeCli,
            "open_ai_api" | "openai_api" => Self::OpenAiApi,
            "anthropic_api" => Self::AnthropicApi,
            "manual_web" => Self::ManualWeb,
            "test" => Self::Test,
            _ => return None,
        })
    }

    /// The CLI binary name for discovery, when this is a CLI runtime.
    pub fn cli_binary(self) -> Option<&'static str> {
        match self {
            Self::ClaudeCodeCli => Some("claude"),
            Self::CodexCli => Some("codex"),
            Self::OpenCodeCli => Some("opencode"),
            _ => None,
        }
    }

    /// Whether this runtime is an autonomous coding agent with a shell/tool loop.
    pub fn is_agent_runtime(self) -> bool {
        matches!(
            self,
            Self::ClaudeCodeCli | Self::CodexCli | Self::OpenCodeCli
        )
    }
}

impl std::fmt::Display for RuntimeProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Cli,
    Api,
    Manual,
    Test,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingMode {
    SubscriptionIncluded,
    ApiMetered,
    Credits,
    LocalCompute,
    Unknown,
}

impl BillingMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SubscriptionIncluded => "subscription_included",
            Self::ApiMetered => "api_metered",
            Self::Credits => "credits",
            Self::LocalCompute => "local_compute",
            Self::Unknown => "unknown",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "subscription_included" => Self::SubscriptionIncluded,
            "api_metered" => Self::ApiMetered,
            "credits" => Self::Credits,
            "local_compute" => Self::LocalCompute,
            "unknown" => Self::Unknown,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    Automatic,
    Manual,
}

impl ExecutionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Automatic => "automatic",
            Self::Manual => "manual",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "automatic" => Self::Automatic,
            "manual" => Self::Manual,
            _ => return None,
        })
    }
}

// ============================================================
// Auth
// ============================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthKind {
    SubscriptionLogin,
    ApiKey,
    OAuth,
    CliCredential,
    EnvironmentCredential,
    CloudCredential,
    Manual,
    None,
}

impl AuthKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SubscriptionLogin => "subscription_login",
            Self::ApiKey => "api_key",
            Self::OAuth => "oauth",
            Self::CliCredential => "cli_credential",
            Self::EnvironmentCredential => "environment_credential",
            Self::CloudCredential => "cloud_credential",
            Self::Manual => "manual",
            Self::None => "none",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "subscription_login" => Self::SubscriptionLogin,
            "api_key" => Self::ApiKey,
            "oauth" => Self::OAuth,
            "cli_credential" => Self::CliCredential,
            "environment_credential" => Self::EnvironmentCredential,
            "cloud_credential" => Self::CloudCredential,
            "manual" => Self::Manual,
            "none" => Self::None,
            _ => return None,
        })
    }

    /// Subscription-based auth is NOT the same balance as API billing.
    pub fn billing_mode(self) -> BillingMode {
        match self {
            Self::SubscriptionLogin | Self::CliCredential => BillingMode::SubscriptionIncluded,
            Self::ApiKey | Self::EnvironmentCredential | Self::OAuth | Self::CloudCredential => {
                BillingMode::ApiMetered
            }
            Self::Manual | Self::None => BillingMode::Unknown,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthStatus {
    #[default]
    Unknown,
    Available,
    Unavailable,
    Expired,
    NeedsLogin,
    Error,
}

impl AuthStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Available => "available",
            Self::Unavailable => "unavailable",
            Self::Expired => "expired",
            Self::NeedsLogin => "needs_login",
            Self::Error => "error",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "unknown" => Self::Unknown,
            "available" => Self::Available,
            "unavailable" => Self::Unavailable,
            "expired" => Self::Expired,
            "needs_login" => Self::NeedsLogin,
            "error" => Self::Error,
            _ => return None,
        })
    }
}

/// A locator for a credential owned elsewhere (`env:OPENAI_API_KEY`,
/// `keyring:argus/openai`, `cli:claude`). The value is never stored here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretReference {
    pub locator: String,
}

impl SecretReference {
    pub fn new(locator: impl Into<String>) -> Self {
        Self {
            locator: locator.into(),
        }
    }

    pub fn env(var: &str) -> Self {
        Self::new(format!("env:{var}"))
    }

    pub fn cli_managed(provider: &str) -> Self {
        Self::new(format!("cli:{provider}"))
    }

    pub fn keyring(path: &str) -> Self {
        Self::new(format!("keyring:{path}"))
    }

    /// Safe for logs/API: the locator names the secret, it is not the secret.
    pub fn redacted(&self) -> &str {
        &self.locator
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuthProfile {
    pub auth_profile_id: Uuid,
    pub provider_id: RuntimeProvider,
    pub auth_kind: AuthKind,
    pub account_label: Option<String>,
    pub workspace_label: Option<String>,
    pub status: AuthStatus,
    pub secret_reference: Option<SecretReference>,
    pub credential_source: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_verified_at: Option<DateTime<Utc>>,
    pub metadata: JsonValue,
    pub created_at: DateTime<Utc>,
}

// ============================================================
// Quota
// ============================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaKind {
    SubscriptionAllowance,
    ApiRateLimit,
    ApiSpendBudget,
    Credits,
    LocalBudget,
    Unknown,
}

impl QuotaKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SubscriptionAllowance => "subscription_allowance",
            Self::ApiRateLimit => "api_rate_limit",
            Self::ApiSpendBudget => "api_spend_budget",
            Self::Credits => "credits",
            Self::LocalBudget => "local_budget",
            Self::Unknown => "unknown",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "subscription_allowance" => Self::SubscriptionAllowance,
            "api_rate_limit" => Self::ApiRateLimit,
            "api_spend_budget" => Self::ApiSpendBudget,
            "credits" => Self::Credits,
            "local_budget" => Self::LocalBudget,
            "unknown" => Self::Unknown,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaAvailability {
    Available,
    Unavailable,
    Unknown,
}

impl QuotaAvailability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Unavailable => "unavailable",
            Self::Unknown => "unknown",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "available" => Self::Available,
            "unavailable" => Self::Unavailable,
            "unknown" => Self::Unknown,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaSource {
    OfficialMachineReadable,
    CliStatus,
    ApiHeaders,
    LocalObservedUsage,
    RateLimitObservation,
    ManualConfig,
    Unknown,
}

impl QuotaSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OfficialMachineReadable => "official_machine_readable",
            Self::CliStatus => "cli_status",
            Self::ApiHeaders => "api_headers",
            Self::LocalObservedUsage => "local_observed_usage",
            Self::RateLimitObservation => "rate_limit_observation",
            Self::ManualConfig => "manual_config",
            Self::Unknown => "unknown",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "official_machine_readable" => Self::OfficialMachineReadable,
            "cli_status" => Self::CliStatus,
            "api_headers" => Self::ApiHeaders,
            "local_observed_usage" => Self::LocalObservedUsage,
            "rate_limit_observation" => Self::RateLimitObservation,
            "manual_config" => Self::ManualConfig,
            "unknown" => Self::Unknown,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaConfidence {
    High,
    Medium,
    Low,
    Unknown,
}

impl QuotaConfidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
            Self::Unknown => "unknown",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "high" => Self::High,
            "medium" => Self::Medium,
            "low" => Self::Low,
            "unknown" => Self::Unknown,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QuotaSnapshot {
    pub quota_snapshot_id: Uuid,
    pub provider_id: RuntimeProvider,
    pub auth_profile_id: Option<Uuid>,
    pub quota_kind: QuotaKind,
    pub availability: QuotaAvailability,
    /// `None` means unknown — never zero, never unlimited.
    pub remaining: Option<f64>,
    pub quota_limit: Option<f64>,
    pub unit: String,
    pub reset_at: Option<DateTime<Utc>>,
    pub observed_at: DateTime<Utc>,
    pub source: QuotaSource,
    pub confidence: QuotaConfidence,
    pub raw_metadata: JsonValue,
}

impl QuotaSnapshot {
    /// Unknown quota must not be treated as exhausted.
    pub fn is_known_exhausted(&self) -> bool {
        self.availability == QuotaAvailability::Unavailable || self.remaining == Some(0.0)
    }
}

// ============================================================
// Health / descriptors
// ============================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderHealthStatus {
    Healthy,
    Degraded,
    Unavailable,
    Unknown,
}

impl ProviderHealthStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Degraded => "degraded",
            Self::Unavailable => "unavailable",
            Self::Unknown => "unknown",
        }
    }

    pub fn rank(self) -> u8 {
        match self {
            Self::Healthy => 0,
            Self::Degraded => 1,
            Self::Unknown => 2,
            Self::Unavailable => 3,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProviderHealth {
    pub provider_id: Option<RuntimeProvider>,
    pub status: Option<ProviderHealthStatus>,
    pub latency_ms: Option<u64>,
    pub auth_status: Option<AuthStatus>,
    pub last_success: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub checked_at: Option<DateTime<Utc>>,
}

impl ProviderHealth {
    pub fn status_or_unknown(&self) -> ProviderHealthStatus {
        self.status.unwrap_or(ProviderHealthStatus::Unknown)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderDescriptor {
    pub provider_id: RuntimeProvider,
    pub display_name: String,
    pub kind: ProviderKind,
    pub billing_modes: Vec<BillingMode>,
    /// Capabilities the *runtime* itself provides (e.g. a CLI agent has a
    /// tool loop; a plain API runtime does not).
    pub runtime_capabilities: Vec<CapabilityRequirement>,
    pub execution_mode: ExecutionMode,
    pub enabled: bool,
    /// Lower = higher priority. Deterministic tie-break.
    pub priority: i32,
    pub allowed_roles: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelDescriptor {
    pub model_id: String,
    pub provider_id: RuntimeProvider,
    pub display_name: String,
    pub capability_tier: ModelTier,
    pub capabilities: Vec<CapabilityRequirement>,
    pub supported_roles: Vec<String>,
    pub supported_purposes: Vec<String>,
    pub reasoning_levels: Vec<ReasoningEffort>,
    pub context_class: String,
    pub supports_tools: bool,
    pub supports_code_execution: bool,
    pub supports_structured_output: bool,
    pub availability: bool,
    pub metadata_revision: String,
    pub discovered_at: DateTime<Utc>,
}

impl ModelDescriptor {
    /// Effective capability = model capability ∩ runtime capability.
    pub fn effective_capabilities(
        &self,
        provider: &ProviderDescriptor,
    ) -> Vec<CapabilityRequirement> {
        self.capabilities
            .iter()
            .copied()
            .filter(|c| provider.runtime_capabilities.contains(c))
            .collect()
    }
}

/// How much to trust a best-effort parse of an official CLI's human-readable
/// output. Machine-readable official output is `High`; a phrase match on text
/// that the CLI may reword is `Medium`/`Low`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParseConfidence {
    High,
    Medium,
    #[default]
    Low,
    Unknown,
}

impl ParseConfidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProviderDiscovery {
    pub installed: bool,
    pub version: Option<String>,
    pub path: Option<String>,
    pub checked_at: Option<DateTime<Utc>>,
    /// Derived from the CLI's own official, non-interactive status command.
    /// Never scraped from a browser, session file, or token store.
    pub auth_status: AuthStatus,
    pub auth_confidence: ParseConfidence,
    pub auth_parser_version: Option<String>,
    pub note: Option<String>,
}

// ============================================================
// Selection policy
// ============================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionPreference {
    SubscriptionFirst,
    Balanced,
    PreferApi,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FallbackPolicy {
    Never,
    Ask,
    AutomaticWithinBudget,
}

impl FallbackPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::Ask => "ask",
            Self::AutomaticWithinBudget => "automatic_within_budget",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "never" => Self::Never,
            "ask" => Self::Ask,
            "automatic_within_budget" => Self::AutomaticWithinBudget,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SelectionPolicy {
    pub cost_preference: CostPreference,
    pub subscription_preference: SubscriptionPreference,
    pub fallback_policy: FallbackPolicy,
    /// `None` is "no allow-list configured"; `Some(vec![])` is "nothing is
    /// permitted". The two must stay distinct — collapsing an over-narrowed
    /// list into an empty one would silently widen past the project ceiling.
    pub allowed_providers: Option<Vec<RuntimeProvider>>,
    pub denied_providers: Vec<RuntimeProvider>,
    pub max_tier: Option<ModelTier>,
    pub preferred_runtime: Option<RuntimeProvider>,
    /// `None` is "no model allow-list configured"; `Some(vec![])` blocks all.
    #[serde(default)]
    pub allowed_models: Option<Vec<String>>,
    /// Model ids that are always excluded, whatever the allowlist says.
    #[serde(default)]
    pub denied_models: Vec<String>,
}

impl SelectionPolicy {
    /// Whether a model id passes this policy's model allow/deny lists.
    pub fn model_permitted(&self, model_id: &str) -> bool {
        if self.denied_models.iter().any(|m| m == model_id) {
            return false;
        }
        match &self.allowed_models {
            None => true,
            Some(list) => list.iter().any(|m| m == model_id),
        }
    }
}

/// Intersection that preserves "unrestricted" as distinct from "empty".
fn intersect<T: PartialEq + Clone>(caller: Option<&[T]>, ceiling: Option<&[T]>) -> Option<Vec<T>> {
    match (caller, ceiling) {
        (None, None) => None,
        (Some(a), None) | (None, Some(a)) => Some(a.to_vec()),
        (Some(a), Some(b)) => Some(
            a.iter()
                .filter(|x| b.iter().any(|y| y == *x))
                .cloned()
                .collect(),
        ),
    }
}

/// Restriction order for API fallback: `Never` is stricter than `Ask`, which is
/// stricter than automatic spend.
impl FallbackPolicy {
    fn restrictiveness(self) -> u8 {
        match self {
            Self::AutomaticWithinBudget => 0,
            Self::Ask => 1,
            Self::Never => 2,
        }
    }

    /// The stricter of two fallback policies. Used when a caller-supplied policy
    /// is narrowed by a project policy that can never be widened back.
    pub(crate) fn most_restrictive(a: Self, b: Self) -> Self {
        if a.restrictiveness() >= b.restrictiveness() {
            a
        } else {
            b
        }
    }
}

impl Default for SelectionPolicy {
    fn default() -> Self {
        Self {
            cost_preference: CostPreference::CheapestSufficient,
            subscription_preference: SubscriptionPreference::SubscriptionFirst,
            fallback_policy: FallbackPolicy::AutomaticWithinBudget,
            allowed_providers: None,
            denied_providers: Vec::new(),
            max_tier: None,
            preferred_runtime: None,
            allowed_models: None,
            denied_models: Vec::new(),
        }
    }
}

/// Project-level policy. Resolved over global defaults.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectProviderPolicy {
    pub project_id: Uuid,
    pub allowed_providers: Vec<RuntimeProvider>,
    pub denied_providers: Vec<RuntimeProvider>,
    pub subscription_first: bool,
    pub api_fallback: FallbackPolicy,
    pub max_tier: Option<ModelTier>,
    pub preferred_runtime: Option<RuntimeProvider>,
    #[serde(default)]
    pub allowed_models: Vec<String>,
    #[serde(default)]
    pub denied_models: Vec<String>,
}

impl ProjectProviderPolicy {
    pub fn default_for(project_id: Uuid) -> Self {
        Self {
            project_id,
            allowed_providers: Vec::new(),
            denied_providers: Vec::new(),
            subscription_first: true,
            api_fallback: FallbackPolicy::AutomaticWithinBudget,
            max_tier: None,
            preferred_runtime: None,
            allowed_models: Vec::new(),
            denied_models: Vec::new(),
        }
    }

    pub fn to_policy(&self) -> SelectionPolicy {
        SelectionPolicy {
            cost_preference: CostPreference::CheapestSufficient,
            subscription_preference: if self.subscription_first {
                SubscriptionPreference::SubscriptionFirst
            } else {
                SubscriptionPreference::Balanced
            },
            fallback_policy: self.api_fallback,
            allowed_providers: opt_vec(&self.allowed_providers),
            denied_providers: self.denied_providers.clone(),
            max_tier: self.max_tier,
            preferred_runtime: self.preferred_runtime,
            allowed_models: opt_vec(&self.allowed_models),
            denied_models: self.denied_models.clone(),
        }
    }

    /// Resolve a caller-supplied policy against this project policy (§52).
    ///
    /// A work-item/caller policy may only *narrow* what the project allows — it
    /// can never re-allow a denied provider, widen past an allowlist, raise the
    /// tier ceiling, or relax the fallback rule. This keeps the persisted
    /// project policy authoritative no matter which entrypoint calls the router.
    pub fn resolve(&self, caller: &SelectionPolicy) -> SelectionPolicy {
        let mut denied_providers = caller.denied_providers.clone();
        for p in &self.denied_providers {
            if !denied_providers.contains(p) {
                denied_providers.push(*p);
            }
        }

        let allowed_providers = intersect(
            caller.allowed_providers.as_deref(),
            opt_vec(&self.allowed_providers).as_deref(),
        );
        let allowed_models = intersect(
            caller.allowed_models.as_deref(),
            opt_vec(&self.allowed_models).as_deref(),
        );

        SelectionPolicy {
            cost_preference: caller.cost_preference,
            subscription_preference: caller.subscription_preference,
            fallback_policy: FallbackPolicy::most_restrictive(
                caller.fallback_policy,
                self.api_fallback,
            ),
            allowed_providers,
            denied_providers,
            max_tier: match (caller.max_tier, self.max_tier) {
                (Some(c), Some(p)) => Some(c.min(p)),
                (c, p) => c.or(p),
            },
            preferred_runtime: caller.preferred_runtime.or(self.preferred_runtime),
            allowed_models,
            denied_models: union_strings(&caller.denied_models, &self.denied_models),
        }
    }
}

/// An empty persisted list means "not configured", which is distinct from the
/// router's "configured but permitting nothing".
fn opt_vec<T: Clone>(v: &[T]) -> Option<Vec<T>> {
    if v.is_empty() { None } else { Some(v.to_vec()) }
}

fn union_strings(base: &[String], extra: &[String]) -> Vec<String> {
    let mut out = base.to_vec();
    for s in extra {
        if !out.contains(s) {
            out.push(s.clone());
        }
    }
    out
}

// ============================================================
// Target / result
// ============================================================

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExecutionTarget {
    pub runtime_provider: RuntimeProvider,
    pub provider_id: RuntimeProvider,
    pub model_id: Option<String>,
    pub auth_profile_id: Option<Uuid>,
    pub capability_tier: ModelTier,
    pub reasoning_effort: ReasoningEffort,
    pub billing_mode: BillingMode,
    pub execution_mode: ExecutionMode,
    pub quota_snapshot_id: Option<Uuid>,
    pub selection_reasons: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailoverReason {
    AuthUnavailable,
    QuotaUnavailable,
    ProviderUnhealthy,
    RuntimeMissing,
    BudgetBlocked,
    CapabilityMismatch,
    PolicyDenied,
}

impl FailoverReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AuthUnavailable => "auth_unavailable",
            Self::QuotaUnavailable => "quota_unavailable",
            Self::ProviderUnhealthy => "provider_unhealthy",
            Self::RuntimeMissing => "runtime_missing",
            Self::BudgetBlocked => "budget_blocked",
            Self::CapabilityMismatch => "capability_mismatch",
            Self::PolicyDenied => "policy_denied",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum ProviderRoutingResult {
    Selected {
        target: ExecutionTarget,
    },
    NeedsLogin {
        provider: RuntimeProvider,
        auth_profile_id: Option<Uuid>,
        action: String,
    },
    NeedsApproval {
        target: ExecutionTarget,
        reason: String,
    },
    WaitForQuota {
        provider: RuntimeProvider,
        reset_at: Option<DateTime<Utc>>,
        reason: String,
    },
    NoCapableProvider {
        reasons: Vec<String>,
    },
    ProviderUnavailable {
        provider: RuntimeProvider,
        reason: String,
    },
    BudgetBlocked {
        target: ExecutionTarget,
        reason: String,
    },
}

impl ProviderRoutingResult {
    pub fn selected_target(&self) -> Option<&ExecutionTarget> {
        match self {
            Self::Selected { target } => Some(target),
            _ => None,
        }
    }
}

/// The full in-memory state the router reads. Built from the registry +
//  persisted auth/quota/health.
#[derive(Clone, Debug, Default)]
pub struct ProviderState {
    pub providers: Vec<ProviderDescriptor>,
    pub models: Vec<ModelDescriptor>,
    pub auth_profiles: Vec<AuthProfile>,
    pub quota: Vec<QuotaSnapshot>,
    pub health: Vec<ProviderHealth>,
    pub discovery: Vec<(RuntimeProvider, ProviderDiscovery)>,
    /// Provider ids that are present but had an explicit routing attempt fail.
    pub attempts: Vec<ProviderRoutingAttempt>,
}

impl ProviderState {
    pub fn provider(&self, id: RuntimeProvider) -> Option<&ProviderDescriptor> {
        self.providers.iter().find(|p| p.provider_id == id)
    }

    pub fn models_for(&self, id: RuntimeProvider) -> impl Iterator<Item = &ModelDescriptor> {
        self.models.iter().filter(move |m| m.provider_id == id)
    }

    pub fn auth_for(&self, id: RuntimeProvider) -> Vec<&AuthProfile> {
        self.auth_profiles
            .iter()
            .filter(|a| a.provider_id == id)
            .collect()
    }

    pub fn health_for(&self, id: RuntimeProvider) -> ProviderHealthStatus {
        self.health
            .iter()
            .find(|h| h.provider_id == Some(id))
            .map(|h| h.status_or_unknown())
            .unwrap_or(ProviderHealthStatus::Unknown)
    }

    pub fn discovery_for(&self, id: RuntimeProvider) -> Option<&ProviderDiscovery> {
        self.discovery
            .iter()
            .find(|(p, _)| *p == id)
            .map(|(_, d)| d)
    }

    /// Latest quota snapshot for a provider/auth profile pair.
    pub fn latest_quota(
        &self,
        provider: RuntimeProvider,
        auth_profile_id: Option<Uuid>,
    ) -> Option<&QuotaSnapshot> {
        self.quota
            .iter()
            .filter(|q| q.provider_id == provider && q.auth_profile_id == auth_profile_id)
            .max_by_key(|q| q.observed_at)
    }
}

// ============================================================
// Persisted records
// ============================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProviderRoutingSelection {
    pub selection_id: Uuid,
    pub project_id: Uuid,
    pub routing_decision_id: Option<Uuid>,
    pub provider_id: RuntimeProvider,
    pub runtime_provider: RuntimeProvider,
    pub model_id: Option<String>,
    pub auth_profile_id: Option<Uuid>,
    pub billing_mode: BillingMode,
    pub execution_mode: ExecutionMode,
    pub required_tier: ModelTier,
    pub reasoning_effort: ReasoningEffort,
    pub policy: SelectionPolicy,
    pub quota_snapshot_ref: Option<Uuid>,
    pub reasons: Vec<String>,
    pub router_version: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct NewRoutingSelection {
    pub project_id: Uuid,
    pub routing_decision_id: Option<Uuid>,
    pub target: ExecutionTarget,
    pub required_tier: ModelTier,
    pub reasoning_effort: ReasoningEffort,
    pub policy: SelectionPolicy,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProviderRoutingAttempt {
    pub attempt_id: Uuid,
    pub project_id: Uuid,
    pub routing_decision_id: Option<Uuid>,
    pub candidate_provider: RuntimeProvider,
    pub candidate_model: Option<String>,
    pub outcome: String,
    pub reason: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct NewRoutingAttempt {
    pub project_id: Uuid,
    pub routing_decision_id: Option<Uuid>,
    pub candidate_provider: RuntimeProvider,
    pub candidate_model: Option<String>,
    pub outcome: String,
    pub reason: String,
}

#[derive(Clone, Debug)]
pub struct NewAuthProfile {
    pub provider_id: RuntimeProvider,
    pub auth_kind: AuthKind,
    pub account_label: Option<String>,
    pub workspace_label: Option<String>,
    pub status: AuthStatus,
    pub secret_reference: Option<SecretReference>,
    pub credential_source: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enums_round_trip() {
        for p in [
            RuntimeProvider::ClaudeCodeCli,
            RuntimeProvider::CodexCli,
            RuntimeProvider::OpenAiApi,
            RuntimeProvider::ManualWeb,
        ] {
            assert_eq!(RuntimeProvider::parse(p.as_str()), Some(p));
        }
        for a in [AuthKind::SubscriptionLogin, AuthKind::ApiKey] {
            assert_eq!(AuthKind::parse(a.as_str()), Some(a));
        }
        for q in [QuotaKind::SubscriptionAllowance, QuotaKind::ApiSpendBudget] {
            assert_eq!(QuotaKind::parse(q.as_str()), Some(q));
        }
        assert_eq!(FallbackPolicy::parse("ask"), Some(FallbackPolicy::Ask));
    }

    #[test]
    fn resolve_never_widens_project_policy() {
        let mut project = ProjectProviderPolicy::default_for(Uuid::new_v4());
        project.denied_providers = vec![RuntimeProvider::ClaudeCodeCli];
        project.allowed_providers = vec![RuntimeProvider::ClaudeCodeCli, RuntimeProvider::CodexCli];
        project.max_tier = Some(ModelTier::T2);
        project.api_fallback = FallbackPolicy::Never;

        // A caller that omits the denial, re-allows a denied provider, asks for
        // a provider outside the ceiling, raises the tier, and permits spend.
        let caller = SelectionPolicy {
            allowed_providers: Some(vec![RuntimeProvider::ClaudeCodeCli]),
            denied_providers: Vec::new(),
            max_tier: Some(ModelTier::T3),
            fallback_policy: FallbackPolicy::AutomaticWithinBudget,
            ..Default::default()
        };
        let resolved = project.resolve(&caller);

        assert!(
            resolved
                .denied_providers
                .contains(&RuntimeProvider::ClaudeCodeCli)
        );
        assert_eq!(
            resolved.allowed_providers,
            Some(vec![RuntimeProvider::ClaudeCodeCli]),
            "the caller's list is intersected with the project ceiling, not replaced"
        );
        assert!(
            !resolved
                .allowed_providers
                .as_ref()
                .is_some_and(|l| l.contains(&RuntimeProvider::OpenAiApi)),
            "a caller cannot introduce a provider the project never allowed"
        );
        assert_eq!(
            resolved.max_tier,
            Some(ModelTier::T2),
            "the stricter tier ceiling wins"
        );
        assert_eq!(
            resolved.fallback_policy,
            FallbackPolicy::Never,
            "the stricter fallback policy wins"
        );
    }

    #[test]
    fn resolve_keeps_caller_narrowing() {
        let project = ProjectProviderPolicy::default_for(Uuid::new_v4());
        let caller = SelectionPolicy {
            fallback_policy: FallbackPolicy::Ask,
            denied_models: vec!["banned".to_string()],
            ..Default::default()
        };
        let resolved = project.resolve(&caller);
        assert_eq!(
            resolved.fallback_policy,
            FallbackPolicy::Ask,
            "a stricter caller is respected"
        );
        assert!(!resolved.model_permitted("banned"));
        assert!(resolved.model_permitted("anything-else"));
    }

    #[test]
    fn model_allow_deny_lists() {
        let mut p = SelectionPolicy::default();
        assert!(
            p.model_permitted("any"),
            "no allow-list means no restriction"
        );
        p.allowed_models = Some(vec!["a".to_string()]);
        assert!(p.model_permitted("a"));
        assert!(!p.model_permitted("b"));
        p.denied_models = vec!["a".to_string()];
        assert!(
            !p.model_permitted("a"),
            "deny wins over allow for the same id"
        );
    }

    /// Regression: an allow-list narrowed to nothing must stay "nothing", not
    /// fall through the empty-means-unrestricted reading and widen back out.
    #[test]
    fn disjoint_allow_lists_fail_closed_rather_than_open() {
        let mut project = ProjectProviderPolicy::default_for(Uuid::new_v4());
        project.allowed_providers = vec![RuntimeProvider::OpenAiApi];
        let caller = SelectionPolicy {
            allowed_providers: Some(vec![RuntimeProvider::ClaudeCodeCli]),
            ..Default::default()
        };
        let resolved = project.resolve(&caller);
        assert_eq!(
            resolved.allowed_providers,
            Some(Vec::new()),
            "Some([]) (nothing permitted) must not collapse to None (unrestricted)"
        );

        let mut project_models = ProjectProviderPolicy::default_for(Uuid::new_v4());
        project_models.allowed_models = vec!["m-api".to_string()];
        let caller_models = SelectionPolicy {
            allowed_models: Some(vec!["m-cli".to_string()]),
            ..Default::default()
        };
        let resolved_models = project_models.resolve(&caller_models);
        assert_eq!(resolved_models.allowed_models, Some(Vec::new()));
        assert!(
            !resolved_models.model_permitted("m-cli") && !resolved_models.model_permitted("m-api"),
            "a caller may not smuggle a model past the project ceiling"
        );
    }

    #[test]
    fn subscription_and_api_auth_have_distinct_billing() {
        assert_eq!(
            AuthKind::SubscriptionLogin.billing_mode(),
            BillingMode::SubscriptionIncluded
        );
        assert_eq!(AuthKind::ApiKey.billing_mode(), BillingMode::ApiMetered);
        assert_ne!(
            AuthKind::SubscriptionLogin.billing_mode(),
            AuthKind::ApiKey.billing_mode(),
            "SUBSCRIPTION AUTH == API AUTH: NO"
        );
    }

    #[test]
    fn unknown_quota_is_not_exhausted() {
        let snapshot = QuotaSnapshot {
            quota_snapshot_id: Uuid::new_v4(),
            provider_id: RuntimeProvider::ClaudeCodeCli,
            auth_profile_id: None,
            quota_kind: QuotaKind::SubscriptionAllowance,
            availability: QuotaAvailability::Unknown,
            remaining: None,
            quota_limit: None,
            unit: "unknown".to_string(),
            reset_at: None,
            observed_at: Utc::now(),
            source: QuotaSource::CliStatus,
            confidence: QuotaConfidence::Low,
            raw_metadata: serde_json::json!({}),
        };
        assert!(!snapshot.is_known_exhausted());

        let exhausted = QuotaSnapshot {
            availability: QuotaAvailability::Unavailable,
            remaining: Some(0.0),
            ..snapshot
        };
        assert!(exhausted.is_known_exhausted());
    }

    #[test]
    fn secret_reference_holds_a_locator_not_a_value() {
        let r = SecretReference::env("OPENAI_API_KEY");
        assert_eq!(r.redacted(), "env:OPENAI_API_KEY");
        assert!(!r.redacted().contains("sk-"));
    }

    #[test]
    fn effective_capability_intersects_model_and_runtime() {
        let provider = ProviderDescriptor {
            provider_id: RuntimeProvider::OpenAiApi,
            display_name: "OpenAI API".into(),
            kind: ProviderKind::Api,
            billing_modes: vec![BillingMode::ApiMetered],
            runtime_capabilities: vec![
                CapabilityRequirement::CodeGeneration,
                CapabilityRequirement::StrongReasoning,
            ],
            execution_mode: ExecutionMode::Automatic,
            enabled: true,
            priority: 10,
            allowed_roles: vec![],
        };
        let model = ModelDescriptor {
            model_id: "m".into(),
            provider_id: RuntimeProvider::OpenAiApi,
            display_name: "m".into(),
            capability_tier: ModelTier::T3,
            capabilities: vec![
                CapabilityRequirement::CodeGeneration,
                CapabilityRequirement::StrongReasoning,
                CapabilityRequirement::RepositoryNavigation,
            ],
            supported_roles: vec![],
            supported_purposes: vec![],
            reasoning_levels: vec![],
            context_class: "long".into(),
            supports_tools: false,
            supports_code_execution: false,
            supports_structured_output: true,
            availability: true,
            metadata_revision: "1".into(),
            discovered_at: Utc::now(),
        };
        let effective = model.effective_capabilities(&provider);
        assert!(effective.contains(&CapabilityRequirement::StrongReasoning));
        assert!(
            !effective.contains(&CapabilityRequirement::RepositoryNavigation),
            "a plain API runtime does not inherit the model's repo-navigation claim"
        );
    }
}
