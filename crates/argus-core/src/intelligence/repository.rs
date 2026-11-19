//! V0.42 persistence for task profiles, routing decisions, outcomes, and
//! escalation decisions. One repository for the whole aggregate, like the
//! other ARGUS repositories.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value as JsonValue;
use sqlx::PgPool;
use uuid::Uuid;

use crate::types::{ArgusError, ArgusResult};

use super::escalation::EscalationPolicy;
use super::types::*;

// ============================================================
// Helpers
// ============================================================

fn strings_from_json(v: JsonValue) -> Vec<String> {
    match v {
        JsonValue::Array(items) => items
            .into_iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

fn strings_to_json(v: &[String]) -> JsonValue {
    JsonValue::Array(v.iter().map(|s| JsonValue::String(s.clone())).collect())
}

fn capabilities_from_json(v: JsonValue) -> Vec<CapabilityRequirement> {
    strings_from_json(v)
        .iter()
        .filter_map(|s| CapabilityRequirement::parse(s))
        .collect()
}

fn capabilities_to_json(v: &[CapabilityRequirement]) -> JsonValue {
    JsonValue::Array(
        v.iter()
            .map(|c| JsonValue::String(c.as_str().to_string()))
            .collect(),
    )
}

fn tier_or_default(s: &str) -> ModelTier {
    ModelTier::parse(s).unwrap_or(ModelTier::T1)
}

fn kind_or_unknown(s: &str) -> TaskKind {
    TaskKind::parse(s).unwrap_or(TaskKind::Unknown)
}

// ============================================================
// Rows
// ============================================================

#[derive(sqlx::FromRow)]
struct TaskProfileRow {
    profile_id: Uuid,
    project_id: Uuid,
    work_item_id: Option<Uuid>,
    task_id: Option<Uuid>,
    task_kind: String,
    secondary_flags: JsonValue,
    complexity_score: f32,
    risk_score: f32,
    ambiguity_score: f32,
    reasoning_score: f32,
    scope_score: f32,
    verification_score: f32,
    context_score: f32,
    reversibility_score: f32,
    recommended_tier: String,
    verification_requirement: String,
    required_capabilities: JsonValue,
    confidence: f32,
    reasons: JsonValue,
    classifier_version: String,
    input_hash: String,
    created_at: DateTime<Utc>,
}

impl TaskProfileRow {
    fn into_record(self) -> TaskProfile {
        TaskProfile {
            profile_id: self.profile_id,
            project_id: self.project_id,
            work_item_id: self.work_item_id,
            task_id: self.task_id,
            task_kind: kind_or_unknown(&self.task_kind),
            secondary_flags: strings_from_json(self.secondary_flags),
            complexity_score: self.complexity_score,
            risk_score: self.risk_score,
            ambiguity_score: self.ambiguity_score,
            reasoning_score: self.reasoning_score,
            scope_score: self.scope_score,
            verification_score: self.verification_score,
            context_score: self.context_score,
            reversibility_score: self.reversibility_score,
            recommended_tier: tier_or_default(&self.recommended_tier),
            verification_requirement: VerificationRequirement::parse(
                &self.verification_requirement,
            )
            .unwrap_or(VerificationRequirement::Standard),
            required_capabilities: capabilities_from_json(self.required_capabilities),
            confidence: self.confidence,
            reasons: strings_from_json(self.reasons),
            classifier_version: self.classifier_version,
            input_hash: self.input_hash,
            created_at: self.created_at,
        }
    }
}

const PROFILE_COLUMNS: &str = "profile_id, project_id, work_item_id, task_id, task_kind, \
    secondary_flags, complexity_score, risk_score, ambiguity_score, reasoning_score, scope_score, \
    verification_score, context_score, reversibility_score, recommended_tier, \
    verification_requirement, required_capabilities, confidence, reasons, classifier_version, \
    input_hash, created_at";

#[derive(sqlx::FromRow)]
struct RoutingDecisionRow {
    routing_id: Uuid,
    profile_id: Uuid,
    project_id: Uuid,
    work_item_id: Option<Uuid>,
    task_id: Option<Uuid>,
    role: String,
    purpose: String,
    required_tier: String,
    preferred_tier: String,
    required_capabilities: JsonValue,
    reasoning_effort: String,
    context_requirement: String,
    verification_requirement: String,
    retry_policy: JsonValue,
    escalation_policy: JsonValue,
    cost_preference: String,
    reasons: JsonValue,
    router_version: String,
    input_hash: String,
    created_at: DateTime<Utc>,
}

impl RoutingDecisionRow {
    fn into_record(self) -> RoutingDecision {
        RoutingDecision {
            routing_id: self.routing_id,
            profile_id: self.profile_id,
            project_id: self.project_id,
            work_item_id: self.work_item_id,
            task_id: self.task_id,
            role: self.role,
            purpose: self.purpose,
            required_tier: tier_or_default(&self.required_tier),
            preferred_tier: tier_or_default(&self.preferred_tier),
            required_capabilities: capabilities_from_json(self.required_capabilities),
            reasoning_effort: ReasoningEffort::parse(&self.reasoning_effort)
                .unwrap_or(ReasoningEffort::Medium),
            context_requirement: ContextRequirement::parse(&self.context_requirement)
                .unwrap_or(ContextRequirement::Medium),
            verification_requirement: VerificationRequirement::parse(
                &self.verification_requirement,
            )
            .unwrap_or(VerificationRequirement::Standard),
            retry_policy: serde_json::from_value(self.retry_policy)
                .unwrap_or_else(|_| RetryPolicy::default()),
            escalation_policy: serde_json::from_value(self.escalation_policy)
                .unwrap_or_else(|_| EscalationPolicy::default()),
            cost_preference: CostPreference::parse(&self.cost_preference)
                .unwrap_or(CostPreference::CheapestSufficient),
            reasons: strings_from_json(self.reasons),
            router_version: self.router_version,
            input_hash: self.input_hash,
            created_at: self.created_at,
        }
    }
}

const ROUTING_COLUMNS: &str = "routing_id, profile_id, project_id, work_item_id, task_id, role, \
    purpose, required_tier, preferred_tier, required_capabilities, reasoning_effort, \
    context_requirement, verification_requirement, retry_policy, escalation_policy, \
    cost_preference, reasons, router_version, input_hash, created_at";

#[derive(sqlx::FromRow)]
struct RoutingOutcomeRow {
    outcome_id: Uuid,
    routing_id: Uuid,
    project_id: Uuid,
    work_item_id: Option<Uuid>,
    task_id: Option<Uuid>,
    selected_tier: String,
    success: bool,
    failure_class: Option<String>,
    attempts: i32,
    duration_ms: Option<i32>,
    artifact_id: Option<Uuid>,
    created_at: DateTime<Utc>,
}

impl RoutingOutcomeRow {
    fn into_record(self) -> RoutingOutcome {
        RoutingOutcome {
            outcome_id: self.outcome_id,
            routing_id: self.routing_id,
            project_id: self.project_id,
            work_item_id: self.work_item_id,
            task_id: self.task_id,
            selected_tier: tier_or_default(&self.selected_tier),
            success: self.success,
            failure_class: self.failure_class.as_deref().and_then(FailureClass::parse),
            attempts: self.attempts.max(0) as u32,
            duration_ms: self.duration_ms.map(|d| d.max(0) as u32),
            artifact_id: self.artifact_id,
            created_at: self.created_at,
        }
    }
}

const OUTCOME_COLUMNS: &str = "outcome_id, routing_id, project_id, work_item_id, task_id, \
    selected_tier, success, failure_class, attempts, duration_ms, artifact_id, created_at";

#[derive(sqlx::FromRow)]
struct EscalationRow {
    escalation_id: Uuid,
    routing_id: Option<Uuid>,
    project_id: Uuid,
    action: String,
    from_tier: String,
    to_tier: Option<String>,
    failure_class: String,
    reason: String,
    retry_after: Option<DateTime<Utc>>,
    requires_human: bool,
    created_at: DateTime<Utc>,
}

impl EscalationRow {
    fn into_record(self) -> EscalationDecision {
        EscalationDecision {
            escalation_id: self.escalation_id,
            routing_id: self.routing_id,
            project_id: self.project_id,
            action: EscalationAction::parse(&self.action).unwrap_or(EscalationAction::Abort),
            from_tier: tier_or_default(&self.from_tier),
            to_tier: self.to_tier.as_deref().and_then(ModelTier::parse),
            failure_class: FailureClass::parse(&self.failure_class)
                .unwrap_or(FailureClass::Unknown),
            reason: self.reason,
            retry_after: self.retry_after,
            requires_human: self.requires_human,
            created_at: self.created_at,
        }
    }
}

const ESCALATION_COLUMNS: &str = "escalation_id, routing_id, project_id, action, from_tier, \
    to_tier, failure_class, reason, retry_after, requires_human, created_at";

// ============================================================
// Trait
// ============================================================

#[async_trait]
pub trait TaskIntelligenceRepository: Send + Sync {
    async fn save_profile(&self, profile: NewTaskProfile) -> ArgusResult<TaskProfile>;
    async fn get_profile(&self, profile_id: Uuid) -> ArgusResult<Option<TaskProfile>>;
    async fn list_profiles(&self, project_id: Uuid, limit: i64) -> ArgusResult<Vec<TaskProfile>>;

    async fn save_routing_decision(
        &self,
        decision: NewRoutingDecision,
    ) -> ArgusResult<RoutingDecision>;
    async fn get_routing_decision(&self, routing_id: Uuid) -> ArgusResult<Option<RoutingDecision>>;
    async fn list_routing_decisions(
        &self,
        project_id: Uuid,
        limit: i64,
    ) -> ArgusResult<Vec<RoutingDecision>>;

    async fn record_outcome(&self, outcome: NewRoutingOutcome) -> ArgusResult<RoutingOutcome>;
    async fn list_outcomes_for_work_item(
        &self,
        project_id: Uuid,
        work_item_id: Uuid,
    ) -> ArgusResult<Vec<RoutingOutcome>>;
    async fn list_outcomes_for_routing(&self, routing_id: Uuid)
    -> ArgusResult<Vec<RoutingOutcome>>;

    async fn record_escalation(
        &self,
        escalation: NewEscalationDecision,
    ) -> ArgusResult<EscalationDecision>;
    async fn list_escalations(
        &self,
        project_id: Uuid,
        limit: i64,
    ) -> ArgusResult<Vec<EscalationDecision>>;

    async fn routing_stats(&self, project_id: Uuid) -> ArgusResult<Vec<TaskKindTierStats>>;
}

// ============================================================
// Postgres
// ============================================================

pub struct PostgresTaskIntelligenceRepository {
    pool: PgPool,
}

impl PostgresTaskIntelligenceRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl TaskIntelligenceRepository for PostgresTaskIntelligenceRepository {
    async fn save_profile(&self, profile: NewTaskProfile) -> ArgusResult<TaskProfile> {
        let query = format!(
            r#"
            INSERT INTO task_profiles (
                profile_id, project_id, work_item_id, task_id, task_kind, secondary_flags,
                complexity_score, risk_score, ambiguity_score, reasoning_score, scope_score,
                verification_score, context_score, reversibility_score, recommended_tier,
                verification_requirement, required_capabilities, confidence, reasons,
                classifier_version, input_hash, created_at
            )
            VALUES (
                gen_random_uuid(), $1, $2, $3, $4, $5,
                $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, NOW()
            )
            ON CONFLICT (project_id, input_hash) DO UPDATE SET input_hash = EXCLUDED.input_hash
            RETURNING {PROFILE_COLUMNS}
            "#
        );
        let row = sqlx::query_as::<_, TaskProfileRow>(&query)
            .bind(profile.project_id)
            .bind(profile.work_item_id)
            .bind(profile.task_id)
            .bind(profile.task_kind.as_str())
            .bind(strings_to_json(&profile.secondary_flags))
            .bind(profile.complexity_score)
            .bind(profile.risk_score)
            .bind(profile.ambiguity_score)
            .bind(profile.reasoning_score)
            .bind(profile.scope_score)
            .bind(profile.verification_score)
            .bind(profile.context_score)
            .bind(profile.reversibility_score)
            .bind(profile.recommended_tier.as_str())
            .bind(profile.verification_requirement.as_str())
            .bind(capabilities_to_json(&profile.required_capabilities))
            .bind(profile.confidence)
            .bind(strings_to_json(&profile.reasons))
            .bind(&profile.classifier_version)
            .bind(&profile.input_hash)
            .fetch_one(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(row.into_record())
    }

    async fn get_profile(&self, profile_id: Uuid) -> ArgusResult<Option<TaskProfile>> {
        let query = format!("SELECT {PROFILE_COLUMNS} FROM task_profiles WHERE profile_id = $1");
        let row = sqlx::query_as::<_, TaskProfileRow>(&query)
            .bind(profile_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(row.map(TaskProfileRow::into_record))
    }

    async fn list_profiles(&self, project_id: Uuid, limit: i64) -> ArgusResult<Vec<TaskProfile>> {
        let query = format!(
            "SELECT {PROFILE_COLUMNS} FROM task_profiles WHERE project_id = $1 \
             ORDER BY created_at DESC LIMIT $2"
        );
        let rows = sqlx::query_as::<_, TaskProfileRow>(&query)
            .bind(project_id)
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(rows.into_iter().map(TaskProfileRow::into_record).collect())
    }

    async fn save_routing_decision(
        &self,
        decision: NewRoutingDecision,
    ) -> ArgusResult<RoutingDecision> {
        let query = format!(
            r#"
            INSERT INTO routing_decisions (
                routing_id, profile_id, project_id, work_item_id, task_id, role, purpose,
                required_tier, preferred_tier, required_capabilities, reasoning_effort,
                context_requirement, verification_requirement, retry_policy, escalation_policy,
                cost_preference, reasons, router_version, input_hash, created_at
            )
            VALUES (
                gen_random_uuid(), $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
                $13, $14, $15, $16, $17, $18, NOW()
            )
            ON CONFLICT (project_id, input_hash) DO UPDATE SET input_hash = EXCLUDED.input_hash
            RETURNING {ROUTING_COLUMNS}
            "#
        );
        let row = sqlx::query_as::<_, RoutingDecisionRow>(&query)
            .bind(decision.profile_id)
            .bind(decision.project_id)
            .bind(decision.work_item_id)
            .bind(decision.task_id)
            .bind(&decision.role)
            .bind(&decision.purpose)
            .bind(decision.required_tier.as_str())
            .bind(decision.preferred_tier.as_str())
            .bind(capabilities_to_json(&decision.required_capabilities))
            .bind(decision.reasoning_effort.as_str())
            .bind(decision.context_requirement.as_str())
            .bind(decision.verification_requirement.as_str())
            .bind(serde_json::to_value(&decision.retry_policy).unwrap_or(JsonValue::Null))
            .bind(serde_json::to_value(&decision.escalation_policy).unwrap_or(JsonValue::Null))
            .bind(decision.cost_preference.as_str())
            .bind(strings_to_json(&decision.reasons))
            .bind(&decision.router_version)
            .bind(&decision.input_hash)
            .fetch_one(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(row.into_record())
    }

    async fn get_routing_decision(&self, routing_id: Uuid) -> ArgusResult<Option<RoutingDecision>> {
        let query =
            format!("SELECT {ROUTING_COLUMNS} FROM routing_decisions WHERE routing_id = $1");
        let row = sqlx::query_as::<_, RoutingDecisionRow>(&query)
            .bind(routing_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(row.map(RoutingDecisionRow::into_record))
    }

    async fn list_routing_decisions(
        &self,
        project_id: Uuid,
        limit: i64,
    ) -> ArgusResult<Vec<RoutingDecision>> {
        let query = format!(
            "SELECT {ROUTING_COLUMNS} FROM routing_decisions WHERE project_id = $1 \
             ORDER BY created_at DESC LIMIT $2"
        );
        let rows = sqlx::query_as::<_, RoutingDecisionRow>(&query)
            .bind(project_id)
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(rows
            .into_iter()
            .map(RoutingDecisionRow::into_record)
            .collect())
    }

    async fn record_outcome(&self, outcome: NewRoutingOutcome) -> ArgusResult<RoutingOutcome> {
        let query = format!(
            r#"
            INSERT INTO routing_outcomes (
                outcome_id, routing_id, project_id, work_item_id, task_id, selected_tier,
                success, failure_class, attempts, duration_ms, artifact_id, created_at
            )
            VALUES (gen_random_uuid(), $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, NOW())
            RETURNING {OUTCOME_COLUMNS}
            "#
        );
        let row = sqlx::query_as::<_, RoutingOutcomeRow>(&query)
            .bind(outcome.routing_id)
            .bind(outcome.project_id)
            .bind(outcome.work_item_id)
            .bind(outcome.task_id)
            .bind(outcome.selected_tier.as_str())
            .bind(outcome.success)
            .bind(outcome.failure_class.map(|f| f.as_str().to_string()))
            .bind(outcome.attempts as i32)
            .bind(outcome.duration_ms.map(|d| d as i32))
            .bind(outcome.artifact_id)
            .fetch_one(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(row.into_record())
    }

    async fn list_outcomes_for_work_item(
        &self,
        project_id: Uuid,
        work_item_id: Uuid,
    ) -> ArgusResult<Vec<RoutingOutcome>> {
        let query = format!(
            "SELECT {OUTCOME_COLUMNS} FROM routing_outcomes \
             WHERE project_id = $1 AND work_item_id = $2 ORDER BY created_at ASC"
        );
        let rows = sqlx::query_as::<_, RoutingOutcomeRow>(&query)
            .bind(project_id)
            .bind(work_item_id)
            .fetch_all(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(rows
            .into_iter()
            .map(RoutingOutcomeRow::into_record)
            .collect())
    }

    async fn list_outcomes_for_routing(
        &self,
        routing_id: Uuid,
    ) -> ArgusResult<Vec<RoutingOutcome>> {
        let query = format!(
            "SELECT {OUTCOME_COLUMNS} FROM routing_outcomes WHERE routing_id = $1 \
             ORDER BY created_at ASC"
        );
        let rows = sqlx::query_as::<_, RoutingOutcomeRow>(&query)
            .bind(routing_id)
            .fetch_all(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(rows
            .into_iter()
            .map(RoutingOutcomeRow::into_record)
            .collect())
    }

    async fn record_escalation(
        &self,
        escalation: NewEscalationDecision,
    ) -> ArgusResult<EscalationDecision> {
        let query = format!(
            r#"
            INSERT INTO escalation_decisions (
                escalation_id, routing_id, project_id, action, from_tier, to_tier,
                failure_class, reason, retry_after, requires_human, created_at
            )
            VALUES (gen_random_uuid(), $1, $2, $3, $4, $5, $6, $7, $8, $9, NOW())
            RETURNING {ESCALATION_COLUMNS}
            "#
        );
        let row = sqlx::query_as::<_, EscalationRow>(&query)
            .bind(escalation.routing_id)
            .bind(escalation.project_id)
            .bind(escalation.action.as_str())
            .bind(escalation.from_tier.as_str())
            .bind(escalation.to_tier.map(|t| t.as_str().to_string()))
            .bind(escalation.failure_class.as_str())
            .bind(&escalation.reason)
            .bind(escalation.retry_after)
            .bind(escalation.requires_human)
            .fetch_one(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(row.into_record())
    }

    async fn list_escalations(
        &self,
        project_id: Uuid,
        limit: i64,
    ) -> ArgusResult<Vec<EscalationDecision>> {
        let query = format!(
            "SELECT {ESCALATION_COLUMNS} FROM escalation_decisions WHERE project_id = $1 \
             ORDER BY created_at DESC LIMIT $2"
        );
        let rows = sqlx::query_as::<_, EscalationRow>(&query)
            .bind(project_id)
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .map_err(ArgusError::Database)?;
        Ok(rows.into_iter().map(EscalationRow::into_record).collect())
    }

    async fn routing_stats(&self, project_id: Uuid) -> ArgusResult<Vec<TaskKindTierStats>> {
        let rows: Vec<(String, String, i64, i64, f64)> = sqlx::query_as(
            "SELECT p.task_kind, o.selected_tier, COUNT(*)::bigint, \
             COALESCE(SUM(CASE WHEN o.success THEN 1 ELSE 0 END), 0)::bigint, \
             COALESCE(AVG(o.attempts), 0)::float8 \
             FROM routing_outcomes o \
             JOIN routing_decisions d ON d.routing_id = o.routing_id \
             JOIN task_profiles p ON p.profile_id = d.profile_id \
             WHERE o.project_id = $1 \
             GROUP BY p.task_kind, o.selected_tier \
             ORDER BY p.task_kind, o.selected_tier",
        )
        .bind(project_id)
        .fetch_all(&self.pool)
        .await
        .map_err(ArgusError::Database)?;

        Ok(rows
            .into_iter()
            .map(|(kind, tier, attempts, successes, avg)| {
                let failure_rate = if attempts > 0 {
                    (attempts - successes) as f64 / attempts as f64
                } else {
                    0.0
                };
                TaskKindTierStats {
                    task_kind: kind_or_unknown(&kind),
                    tier: tier_or_default(&tier),
                    attempts,
                    successes,
                    failure_rate,
                    average_attempts: avg,
                }
            })
            .collect())
    }
}
