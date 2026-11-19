//! V0.44 persistence for auth profiles, quota snapshots, project provider
//! policy, provider overrides, and routing selections/attempts. No raw secret
//! is stored — only a credential locator.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value as JsonValue;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::intelligence::{CapabilityRequirement, ModelTier, ReasoningEffort};
use crate::types::{ArgusError, ArgusResult};

use super::registry::ProviderOverride;
use super::types::*;

fn strings(v: JsonValue) -> Vec<String> {
    match v {
        JsonValue::Array(items) => items
            .into_iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

fn strings_json(v: &[String]) -> JsonValue {
    JsonValue::Array(v.iter().map(|s| JsonValue::String(s.clone())).collect())
}

fn providers_from_json(v: JsonValue) -> Vec<RuntimeProvider> {
    strings(v)
        .iter()
        .filter_map(|s| RuntimeProvider::parse(s))
        .collect()
}

fn providers_to_json(v: &[RuntimeProvider]) -> JsonValue {
    strings_json(&v.iter().map(|p| p.as_str().to_string()).collect::<Vec<_>>())
}

#[derive(sqlx::FromRow)]
struct AuthRow {
    auth_profile_id: Uuid,
    provider_id: String,
    auth_kind: String,
    account_label: Option<String>,
    workspace_label: Option<String>,
    status: String,
    secret_reference: Option<String>,
    credential_source: Option<String>,
    expires_at: Option<DateTime<Utc>>,
    last_verified_at: Option<DateTime<Utc>>,
    metadata: JsonValue,
    created_at: DateTime<Utc>,
}

impl AuthRow {
    fn into_record(self) -> AuthProfile {
        AuthProfile {
            auth_profile_id: self.auth_profile_id,
            provider_id: RuntimeProvider::parse(&self.provider_id).unwrap_or(RuntimeProvider::Test),
            auth_kind: AuthKind::parse(&self.auth_kind).unwrap_or(AuthKind::None),
            account_label: self.account_label,
            workspace_label: self.workspace_label,
            status: AuthStatus::parse(&self.status).unwrap_or(AuthStatus::Unknown),
            secret_reference: self.secret_reference.map(SecretReference::new),
            credential_source: self.credential_source,
            expires_at: self.expires_at,
            last_verified_at: self.last_verified_at,
            metadata: self.metadata,
            created_at: self.created_at,
        }
    }
}

const AUTH_COLUMNS: &str = "auth_profile_id, provider_id, auth_kind, account_label, \
    workspace_label, status, secret_reference, credential_source, expires_at, last_verified_at, \
    metadata, created_at";

#[derive(sqlx::FromRow)]
struct QuotaRow {
    quota_snapshot_id: Uuid,
    provider_id: String,
    auth_profile_id: Option<Uuid>,
    quota_kind: String,
    availability: String,
    remaining: Option<f64>,
    quota_limit: Option<f64>,
    unit: String,
    reset_at: Option<DateTime<Utc>>,
    observed_at: DateTime<Utc>,
    source: String,
    confidence: String,
    raw_metadata: JsonValue,
}

impl QuotaRow {
    fn into_record(self) -> QuotaSnapshot {
        QuotaSnapshot {
            quota_snapshot_id: self.quota_snapshot_id,
            provider_id: RuntimeProvider::parse(&self.provider_id).unwrap_or(RuntimeProvider::Test),
            auth_profile_id: self.auth_profile_id,
            quota_kind: QuotaKind::parse(&self.quota_kind).unwrap_or(QuotaKind::Unknown),
            availability: QuotaAvailability::parse(&self.availability)
                .unwrap_or(QuotaAvailability::Unknown),
            remaining: self.remaining,
            quota_limit: self.quota_limit,
            unit: self.unit,
            reset_at: self.reset_at,
            observed_at: self.observed_at,
            source: QuotaSource::parse(&self.source).unwrap_or(QuotaSource::Unknown),
            confidence: QuotaConfidence::parse(&self.confidence)
                .unwrap_or(QuotaConfidence::Unknown),
            raw_metadata: self.raw_metadata,
        }
    }
}

const QUOTA_COLUMNS: &str = "quota_snapshot_id, provider_id, auth_profile_id, quota_kind, \
    availability, remaining, quota_limit, unit, reset_at, observed_at, source, confidence, \
    raw_metadata";

#[derive(sqlx::FromRow)]
struct SelectionRow {
    selection_id: Uuid,
    project_id: Uuid,
    routing_decision_id: Option<Uuid>,
    provider_id: String,
    runtime_provider: String,
    model_id: Option<String>,
    auth_profile_id: Option<Uuid>,
    billing_mode: String,
    execution_mode: String,
    required_tier: String,
    reasoning_effort: String,
    policy: JsonValue,
    quota_snapshot_ref: Option<Uuid>,
    reasons: JsonValue,
    router_version: String,
    created_at: DateTime<Utc>,
}

impl SelectionRow {
    fn into_record(self) -> ProviderRoutingSelection {
        ProviderRoutingSelection {
            selection_id: self.selection_id,
            project_id: self.project_id,
            routing_decision_id: self.routing_decision_id,
            provider_id: RuntimeProvider::parse(&self.provider_id).unwrap_or(RuntimeProvider::Test),
            runtime_provider: RuntimeProvider::parse(&self.runtime_provider)
                .unwrap_or(RuntimeProvider::Test),
            model_id: self.model_id,
            auth_profile_id: self.auth_profile_id,
            billing_mode: BillingMode::parse(&self.billing_mode).unwrap_or(BillingMode::Unknown),
            execution_mode: ExecutionMode::parse(&self.execution_mode)
                .unwrap_or(ExecutionMode::Automatic),
            required_tier: ModelTier::parse(&self.required_tier).unwrap_or(ModelTier::T1),
            reasoning_effort: ReasoningEffort::parse(&self.reasoning_effort)
                .unwrap_or(ReasoningEffort::Medium),
            policy: serde_json::from_value(self.policy)
                .unwrap_or_else(|_| SelectionPolicy::default()),
            quota_snapshot_ref: self.quota_snapshot_ref,
            reasons: strings(self.reasons),
            router_version: self.router_version,
            created_at: self.created_at,
        }
    }
}

const SELECTION_COLUMNS: &str = "selection_id, project_id, routing_decision_id, provider_id, \
    runtime_provider, model_id, auth_profile_id, billing_mode, execution_mode, required_tier, \
    reasoning_effort, policy, quota_snapshot_ref, reasons, router_version, created_at";

#[async_trait]
pub trait ProviderRepository: Send + Sync {
    async fn list_auth_profiles(&self) -> ArgusResult<Vec<AuthProfile>>;
    async fn get_auth_profile(&self, id: Uuid) -> ArgusResult<Option<AuthProfile>>;
    async fn upsert_auth_profile(&self, profile: NewAuthProfile) -> ArgusResult<AuthProfile>;
    async fn set_auth_status(&self, id: Uuid, status: AuthStatus) -> ArgusResult<()>;

    async fn save_quota(&self, snapshot: QuotaSnapshot) -> ArgusResult<QuotaSnapshot>;
    async fn latest_quota(
        &self,
        provider: RuntimeProvider,
        auth_profile_id: Option<Uuid>,
    ) -> ArgusResult<Option<QuotaSnapshot>>;

    async fn get_policy(&self, project_id: Uuid) -> ArgusResult<Option<ProjectProviderPolicy>>;
    async fn upsert_policy(
        &self,
        policy: ProjectProviderPolicy,
    ) -> ArgusResult<ProjectProviderPolicy>;

    async fn list_provider_overrides(&self) -> ArgusResult<Vec<ProviderOverride>>;
    async fn upsert_provider_override(&self, over: ProviderOverride) -> ArgusResult<()>;

    async fn save_selection(
        &self,
        selection: NewRoutingSelection,
    ) -> ArgusResult<ProviderRoutingSelection>;
    async fn list_selections(
        &self,
        project_id: Uuid,
        limit: i64,
    ) -> ArgusResult<Vec<ProviderRoutingSelection>>;

    async fn record_attempt(
        &self,
        attempt: NewRoutingAttempt,
    ) -> ArgusResult<ProviderRoutingAttempt>;
    async fn list_attempts(
        &self,
        project_id: Uuid,
        limit: i64,
    ) -> ArgusResult<Vec<ProviderRoutingAttempt>>;
}

pub struct PostgresProviderRepository {
    pool: PgPool,
}

impl PostgresProviderRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ProviderRepository for PostgresProviderRepository {
    async fn list_auth_profiles(&self) -> ArgusResult<Vec<AuthProfile>> {
        let query = format!("SELECT {AUTH_COLUMNS} FROM auth_profiles ORDER BY created_at ASC");
        let rows = sqlx::query_as::<_, AuthRow>(&query)
            .fetch_all(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(rows.into_iter().map(AuthRow::into_record).collect())
    }

    async fn get_auth_profile(&self, id: Uuid) -> ArgusResult<Option<AuthProfile>> {
        let query = format!("SELECT {AUTH_COLUMNS} FROM auth_profiles WHERE auth_profile_id = $1");
        let row = sqlx::query_as::<_, AuthRow>(&query)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(row.map(AuthRow::into_record))
    }

    async fn upsert_auth_profile(&self, profile: NewAuthProfile) -> ArgusResult<AuthProfile> {
        // One profile per (provider, auth_kind, account_label, credential_source).
        let query = format!(
            r#"
            INSERT INTO auth_profiles (
                auth_profile_id, provider_id, auth_kind, account_label, workspace_label,
                status, secret_reference, credential_source, metadata, created_at
            )
            VALUES (gen_random_uuid(), $1, $2, $3, $4, $5, $6, $7, '{{}}', NOW())
            ON CONFLICT DO NOTHING
            RETURNING {AUTH_COLUMNS}
            "#
        );
        let row = sqlx::query_as::<_, AuthRow>(&query)
            .bind(profile.provider_id.as_str())
            .bind(profile.auth_kind.as_str())
            .bind(&profile.account_label)
            .bind(&profile.workspace_label)
            .bind(profile.status.as_str())
            .bind(profile.secret_reference.as_ref().map(|r| r.locator.clone()))
            .bind(&profile.credential_source)
            .fetch_optional(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        if let Some(row) = row {
            return Ok(row.into_record());
        }
        // Already existed: return the existing matching row.
        let query = format!(
            "SELECT {AUTH_COLUMNS} FROM auth_profiles WHERE provider_id = $1 AND auth_kind = $2 \
             AND COALESCE(account_label,'') = COALESCE($3,'') ORDER BY created_at ASC LIMIT 1"
        );
        let row = sqlx::query_as::<_, AuthRow>(&query)
            .bind(profile.provider_id.as_str())
            .bind(profile.auth_kind.as_str())
            .bind(&profile.account_label)
            .fetch_one(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(row.into_record())
    }

    async fn set_auth_status(&self, id: Uuid, status: AuthStatus) -> ArgusResult<()> {
        sqlx::query(
            "UPDATE auth_profiles SET status = $2, last_verified_at = NOW() \
             WHERE auth_profile_id = $1",
        )
        .bind(id)
        .bind(status.as_str())
        .execute(&self.pool)
        .await
        .map_err(ArgusError::Database)?;
        Ok(())
    }

    async fn save_quota(&self, snapshot: QuotaSnapshot) -> ArgusResult<QuotaSnapshot> {
        let query = format!(
            r#"
            INSERT INTO quota_snapshots (
                quota_snapshot_id, provider_id, auth_profile_id, quota_kind, availability,
                remaining, quota_limit, unit, reset_at, observed_at, source, confidence, raw_metadata
            )
            VALUES (gen_random_uuid(), $1, $2, $3, $4, $5, $6, $7, $8, NOW(), $9, $10, $11)
            RETURNING {QUOTA_COLUMNS}
            "#
        );
        let row = sqlx::query_as::<_, QuotaRow>(&query)
            .bind(snapshot.provider_id.as_str())
            .bind(snapshot.auth_profile_id)
            .bind(snapshot.quota_kind.as_str())
            .bind(snapshot.availability.as_str())
            .bind(snapshot.remaining)
            .bind(snapshot.quota_limit)
            .bind(&snapshot.unit)
            .bind(snapshot.reset_at)
            .bind(snapshot.source.as_str())
            .bind(snapshot.confidence.as_str())
            .bind(snapshot.raw_metadata)
            .fetch_one(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(row.into_record())
    }

    async fn latest_quota(
        &self,
        provider: RuntimeProvider,
        auth_profile_id: Option<Uuid>,
    ) -> ArgusResult<Option<QuotaSnapshot>> {
        let query = format!(
            "SELECT {QUOTA_COLUMNS} FROM quota_snapshots \
             WHERE provider_id = $1 AND auth_profile_id IS NOT DISTINCT FROM $2 \
             ORDER BY observed_at DESC LIMIT 1"
        );
        let row = sqlx::query_as::<_, QuotaRow>(&query)
            .bind(provider.as_str())
            .bind(auth_profile_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(row.map(QuotaRow::into_record))
    }

    async fn get_policy(&self, project_id: Uuid) -> ArgusResult<Option<ProjectProviderPolicy>> {
        let row = sqlx::query(
            "SELECT project_id, allowed_providers, denied_providers, subscription_first, \
             api_fallback, max_tier, preferred_runtime, allowed_models, denied_models \
             FROM project_provider_policies WHERE project_id = $1",
        )
        .bind(project_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(ArgusError::Database)?;
        Ok(row.map(|r| ProjectProviderPolicy {
            project_id: r.get("project_id"),
            allowed_providers: providers_from_json(r.get("allowed_providers")),
            denied_providers: providers_from_json(r.get("denied_providers")),
            subscription_first: r.get("subscription_first"),
            api_fallback: FallbackPolicy::parse(r.get::<String, _>("api_fallback").as_str())
                .unwrap_or(FallbackPolicy::AutomaticWithinBudget),
            max_tier: r
                .get::<Option<String>, _>("max_tier")
                .and_then(|s| ModelTier::parse(&s)),
            preferred_runtime: r
                .get::<Option<String>, _>("preferred_runtime")
                .and_then(|s| RuntimeProvider::parse(&s)),
            allowed_models: strings(r.get("allowed_models")),
            denied_models: strings(r.get("denied_models")),
        }))
    }

    async fn upsert_policy(
        &self,
        policy: ProjectProviderPolicy,
    ) -> ArgusResult<ProjectProviderPolicy> {
        sqlx::query(
            r#"
            INSERT INTO project_provider_policies (
                project_id, allowed_providers, denied_providers, subscription_first,
                api_fallback, max_tier, preferred_runtime, allowed_models, denied_models,
                updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NOW())
            ON CONFLICT (project_id) DO UPDATE SET
                allowed_providers = EXCLUDED.allowed_providers,
                denied_providers = EXCLUDED.denied_providers,
                subscription_first = EXCLUDED.subscription_first,
                api_fallback = EXCLUDED.api_fallback,
                max_tier = EXCLUDED.max_tier,
                preferred_runtime = EXCLUDED.preferred_runtime,
                allowed_models = EXCLUDED.allowed_models,
                denied_models = EXCLUDED.denied_models,
                updated_at = NOW()
            "#,
        )
        .bind(policy.project_id)
        .bind(providers_to_json(&policy.allowed_providers))
        .bind(providers_to_json(&policy.denied_providers))
        .bind(policy.subscription_first)
        .bind(policy.api_fallback.as_str())
        .bind(policy.max_tier.map(|t| t.as_str().to_string()))
        .bind(policy.preferred_runtime.map(|p| p.as_str().to_string()))
        .bind(strings_json(&policy.allowed_models))
        .bind(strings_json(&policy.denied_models))
        .execute(&self.pool)
        .await
        .map_err(ArgusError::Database)?;
        Ok(policy)
    }

    async fn list_provider_overrides(&self) -> ArgusResult<Vec<ProviderOverride>> {
        let rows = sqlx::query("SELECT provider_id, enabled, priority, config FROM providers")
            .fetch_all(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                let id = RuntimeProvider::parse(&r.get::<String, _>("provider_id"))?;
                let config: JsonValue = r.get("config");
                let caps = config
                    .get("runtime_capabilities")
                    .map(|v| strings(v.clone()))
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|s| CapabilityRequirement::parse(s))
                    .collect::<Vec<_>>();
                Some(ProviderOverride {
                    provider_id: id,
                    enabled: Some(r.get::<bool, _>("enabled")),
                    priority: Some(r.get::<i32, _>("priority")),
                    runtime_capabilities: if caps.is_empty() { None } else { Some(caps) },
                })
            })
            .collect())
    }

    async fn upsert_provider_override(&self, over: ProviderOverride) -> ArgusResult<()> {
        let config = serde_json::json!({
            "runtime_capabilities": over
                .runtime_capabilities
                .as_ref()
                .map(|caps| caps.iter().map(|c| c.as_str()).collect::<Vec<_>>()),
        });
        sqlx::query(
            r#"
            INSERT INTO providers (provider_id, enabled, priority, config, updated_at)
            VALUES ($1, COALESCE($2, TRUE), COALESCE($3, 100), $4, NOW())
            ON CONFLICT (provider_id) DO UPDATE SET
                enabled = COALESCE(EXCLUDED.enabled, providers.enabled),
                priority = COALESCE(EXCLUDED.priority, providers.priority),
                config = EXCLUDED.config,
                updated_at = NOW()
            "#,
        )
        .bind(over.provider_id.as_str())
        .bind(over.enabled)
        .bind(over.priority)
        .bind(config)
        .execute(&self.pool)
        .await
        .map_err(ArgusError::Database)?;
        Ok(())
    }

    async fn save_selection(
        &self,
        selection: NewRoutingSelection,
    ) -> ArgusResult<ProviderRoutingSelection> {
        let query = format!(
            r#"
            INSERT INTO provider_routing_selections (
                selection_id, project_id, routing_decision_id, provider_id, runtime_provider,
                model_id, auth_profile_id, billing_mode, execution_mode, required_tier,
                reasoning_effort, policy, quota_snapshot_ref, reasons, router_version, created_at
            )
            VALUES (gen_random_uuid(), $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, NOW())
            RETURNING {SELECTION_COLUMNS}
            "#
        );
        let row = sqlx::query_as::<_, SelectionRow>(&query)
            .bind(selection.project_id)
            .bind(selection.routing_decision_id)
            .bind(selection.target.provider_id.as_str())
            .bind(selection.target.runtime_provider.as_str())
            .bind(&selection.target.model_id)
            .bind(selection.target.auth_profile_id)
            .bind(selection.target.billing_mode.as_str())
            .bind(selection.target.execution_mode.as_str())
            .bind(selection.required_tier.as_str())
            .bind(selection.reasoning_effort.as_str())
            .bind(serde_json::to_value(&selection.policy).unwrap_or(JsonValue::Null))
            .bind(selection.target.quota_snapshot_id)
            .bind(strings_json(&selection.target.selection_reasons))
            .bind(PROVIDER_ROUTER_VERSION)
            .fetch_one(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(row.into_record())
    }

    async fn list_selections(
        &self,
        project_id: Uuid,
        limit: i64,
    ) -> ArgusResult<Vec<ProviderRoutingSelection>> {
        let query = format!(
            "SELECT {SELECTION_COLUMNS} FROM provider_routing_selections \
             WHERE project_id = $1 ORDER BY created_at DESC LIMIT $2"
        );
        let rows = sqlx::query_as::<_, SelectionRow>(&query)
            .bind(project_id)
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(rows.into_iter().map(SelectionRow::into_record).collect())
    }

    async fn record_attempt(
        &self,
        attempt: NewRoutingAttempt,
    ) -> ArgusResult<ProviderRoutingAttempt> {
        let row = sqlx::query_as::<
            _,
            (
                Uuid,
                Uuid,
                Option<Uuid>,
                String,
                Option<String>,
                String,
                String,
                DateTime<Utc>,
            ),
        >(
            "INSERT INTO provider_routing_attempts (
                attempt_id, project_id, routing_decision_id, candidate_provider, candidate_model,
                outcome, reason, created_at
             )
             VALUES (gen_random_uuid(), $1, $2, $3, $4, $5, $6, NOW())
             RETURNING attempt_id, project_id, routing_decision_id, candidate_provider, \
                       candidate_model, outcome, reason, created_at",
        )
        .bind(attempt.project_id)
        .bind(attempt.routing_decision_id)
        .bind(attempt.candidate_provider.as_str())
        .bind(&attempt.candidate_model)
        .bind(&attempt.outcome)
        .bind(&attempt.reason)
        .fetch_one(&self.pool)
        .await
        .map_err(ArgusError::Database)?;
        Ok(ProviderRoutingAttempt {
            attempt_id: row.0,
            project_id: row.1,
            routing_decision_id: row.2,
            candidate_provider: RuntimeProvider::parse(&row.3).unwrap_or(RuntimeProvider::Test),
            candidate_model: row.4,
            outcome: row.5,
            reason: row.6,
            created_at: row.7,
        })
    }

    async fn list_attempts(
        &self,
        project_id: Uuid,
        limit: i64,
    ) -> ArgusResult<Vec<ProviderRoutingAttempt>> {
        let rows = sqlx::query_as::<
            _,
            (
                Uuid,
                Uuid,
                Option<Uuid>,
                String,
                Option<String>,
                String,
                String,
                DateTime<Utc>,
            ),
        >(
            "SELECT attempt_id, project_id, routing_decision_id, candidate_provider, \
             candidate_model, outcome, reason, created_at FROM provider_routing_attempts \
             WHERE project_id = $1 ORDER BY created_at DESC LIMIT $2",
        )
        .bind(project_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(ArgusError::Database)?;
        Ok(rows
            .into_iter()
            .map(|r| ProviderRoutingAttempt {
                attempt_id: r.0,
                project_id: r.1,
                routing_decision_id: r.2,
                candidate_provider: RuntimeProvider::parse(&r.3).unwrap_or(RuntimeProvider::Test),
                candidate_model: r.4,
                outcome: r.5,
                reason: r.6,
                created_at: r.7,
            })
            .collect())
    }
}
