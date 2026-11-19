//! V0.42 Task Intelligence domain types.
//!
//! The classifier is deterministic and explainable: a [`TaskProfile`] carries
//! normalized signals, a capability list, a minimum capability tier, and the
//! reasons behind it. A tier is a *capability requirement*, never a named
//! provider or model — V0.44 maps tiers to concrete providers.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::task_types::Priority;

/// Bumping this changes every classification hash.
pub const CLASSIFIER_VERSION: &str = "task-intelligence-v1";
/// Bumping this changes every routing hash.
pub const ROUTER_VERSION: &str = "model-router-v1";

// ============================================================
// Tiers
// ============================================================

/// Capability tier. `T3` can execute `T2`, but a decision states the
/// *minimum* sufficient tier so a cost policy can pick the cheapest model
/// that meets it (V0.44).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelTier {
    T0,
    T1,
    T2,
    T3,
}

impl ModelTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::T0 => "T0",
            Self::T1 => "T1",
            Self::T2 => "T2",
            Self::T3 => "T3",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_uppercase().as_str() {
            "T0" => Self::T0,
            "T1" => Self::T1,
            "T2" => Self::T2,
            "T3" => Self::T3,
            _ => return None,
        })
    }

    pub fn rank(self) -> u8 {
        self as u8
    }

    /// One tier up, saturating at T3 — there is no T4.
    pub fn escalate(self) -> Option<Self> {
        match self {
            Self::T0 => Some(Self::T1),
            Self::T1 => Some(Self::T2),
            Self::T2 => Some(Self::T3),
            Self::T3 => None,
        }
    }

    pub fn max(self, other: Self) -> Self {
        if self.rank() >= other.rank() {
            self
        } else {
            other
        }
    }
}

impl std::fmt::Display for ModelTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    Documentation,
    Formatting,
    SimpleCodeChange,
    FeatureImplementation,
    BugFix,
    Refactor,
    DatabaseChange,
    Migration,
    Architecture,
    Security,
    Authentication,
    Infrastructure,
    Integration,
    Testing,
    Research,
    IncidentDebugging,
    Unknown,
}

impl TaskKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Documentation => "documentation",
            Self::Formatting => "formatting",
            Self::SimpleCodeChange => "simple_code_change",
            Self::FeatureImplementation => "feature_implementation",
            Self::BugFix => "bug_fix",
            Self::Refactor => "refactor",
            Self::DatabaseChange => "database_change",
            Self::Migration => "migration",
            Self::Architecture => "architecture",
            Self::Security => "security",
            Self::Authentication => "authentication",
            Self::Infrastructure => "infrastructure",
            Self::Integration => "integration",
            Self::Testing => "testing",
            Self::Research => "research",
            Self::IncidentDebugging => "incident_debugging",
            Self::Unknown => "unknown",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "documentation" => Self::Documentation,
            "formatting" => Self::Formatting,
            "simple_code_change" => Self::SimpleCodeChange,
            "feature_implementation" => Self::FeatureImplementation,
            "bug_fix" => Self::BugFix,
            "refactor" => Self::Refactor,
            "database_change" => Self::DatabaseChange,
            "migration" => Self::Migration,
            "architecture" => Self::Architecture,
            "security" => Self::Security,
            "authentication" => Self::Authentication,
            "infrastructure" => Self::Infrastructure,
            "integration" => Self::Integration,
            "testing" => Self::Testing,
            "research" => Self::Research,
            "incident_debugging" => Self::IncidentDebugging,
            "unknown" => Self::Unknown,
            _ => return None,
        })
    }
}

impl std::fmt::Display for TaskKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityRequirement {
    CodeGeneration,
    StrongReasoning,
    RepositoryNavigation,
    MultiFileEditing,
    DatabaseReasoning,
    ArchitectureReasoning,
    SecurityReasoning,
    Debugging,
    TestGeneration,
    LongContext,
    ToolUse,
    HighReliability,
}

impl CapabilityRequirement {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CodeGeneration => "code_generation",
            Self::StrongReasoning => "strong_reasoning",
            Self::RepositoryNavigation => "repository_navigation",
            Self::MultiFileEditing => "multi_file_editing",
            Self::DatabaseReasoning => "database_reasoning",
            Self::ArchitectureReasoning => "architecture_reasoning",
            Self::SecurityReasoning => "security_reasoning",
            Self::Debugging => "debugging",
            Self::TestGeneration => "test_generation",
            Self::LongContext => "long_context",
            Self::ToolUse => "tool_use",
            Self::HighReliability => "high_reliability",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "code_generation" => Self::CodeGeneration,
            "strong_reasoning" => Self::StrongReasoning,
            "repository_navigation" => Self::RepositoryNavigation,
            "multi_file_editing" => Self::MultiFileEditing,
            "database_reasoning" => Self::DatabaseReasoning,
            "architecture_reasoning" => Self::ArchitectureReasoning,
            "security_reasoning" => Self::SecurityReasoning,
            "debugging" => Self::Debugging,
            "test_generation" => Self::TestGeneration,
            "long_context" => Self::LongContext,
            "tool_use" => Self::ToolUse,
            "high_reliability" => Self::HighReliability,
            _ => return None,
        })
    }
}

impl std::fmt::Display for CapabilityRequirement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    Low,
    Medium,
    High,
    VeryHigh,
}

impl ReasoningEffort {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::VeryHigh => "very_high",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "low" => Self::Low,
            "medium" => Self::Medium,
            "high" => Self::High,
            "very_high" => Self::VeryHigh,
            _ => return None,
        })
    }
}

impl std::fmt::Display for ReasoningEffort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextRequirement {
    Low,
    Medium,
    High,
    VeryHigh,
}

impl ContextRequirement {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::VeryHigh => "very_high",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "low" => Self::Low,
            "medium" => Self::Medium,
            "high" => Self::High,
            "very_high" => Self::VeryHigh,
            _ => return None,
        })
    }
}

impl std::fmt::Display for ContextRequirement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::fmt::Display for CostPreference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How strong a verification the task deserves. `Basic < Standard < Strong <
/// Critical` so the engine can take a floor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationRequirement {
    Basic,
    Standard,
    Strong,
    Critical,
}

impl VerificationRequirement {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Basic => "basic",
            Self::Standard => "standard",
            Self::Strong => "strong",
            Self::Critical => "critical",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "basic" => Self::Basic,
            "standard" => Self::Standard,
            "strong" => Self::Strong,
            "critical" => Self::Critical,
            _ => return None,
        })
    }
}

impl std::fmt::Display for VerificationRequirement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostPreference {
    CheapestSufficient,
    MinimizeCost,
    Balanced,
    QualityFirst,
}

impl CostPreference {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CheapestSufficient => "cheapest_sufficient",
            Self::MinimizeCost => "minimize_cost",
            Self::Balanced => "balanced",
            Self::QualityFirst => "quality_first",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "cheapest_sufficient" => Self::CheapestSufficient,
            "minimize_cost" => Self::MinimizeCost,
            "balanced" => Self::Balanced,
            "quality_first" => Self::QualityFirst,
            _ => return None,
        })
    }
}

// ============================================================
// Failure / escalation
// ============================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    ReasoningFailure,
    ImplementationFailure,
    VerificationFailure,
    EnvironmentFailure,
    DependencyFailure,
    TransientFailure,
    ToolFailure,
    PermissionFailure,
    BudgetFailure,
    QuotaFailure,
    ContextInsufficient,
    RequirementAmbiguous,
    SafetyPolicyBlocked,
    Unknown,
}

impl FailureClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReasoningFailure => "reasoning_failure",
            Self::ImplementationFailure => "implementation_failure",
            Self::VerificationFailure => "verification_failure",
            Self::EnvironmentFailure => "environment_failure",
            Self::DependencyFailure => "dependency_failure",
            Self::TransientFailure => "transient_failure",
            Self::ToolFailure => "tool_failure",
            Self::PermissionFailure => "permission_failure",
            Self::BudgetFailure => "budget_failure",
            Self::QuotaFailure => "quota_failure",
            Self::ContextInsufficient => "context_insufficient",
            Self::RequirementAmbiguous => "requirement_ambiguous",
            Self::SafetyPolicyBlocked => "safety_policy_blocked",
            Self::Unknown => "unknown",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "reasoning_failure" => Self::ReasoningFailure,
            "implementation_failure" => Self::ImplementationFailure,
            "verification_failure" => Self::VerificationFailure,
            "environment_failure" => Self::EnvironmentFailure,
            "dependency_failure" => Self::DependencyFailure,
            "transient_failure" => Self::TransientFailure,
            "tool_failure" => Self::ToolFailure,
            "permission_failure" => Self::PermissionFailure,
            "budget_failure" => Self::BudgetFailure,
            "quota_failure" => Self::QuotaFailure,
            "context_insufficient" => Self::ContextInsufficient,
            "requirement_ambiguous" => Self::RequirementAmbiguous,
            "safety_policy_blocked" => Self::SafetyPolicyBlocked,
            "unknown" => Self::Unknown,
            _ => return None,
        })
    }
}

impl std::fmt::Display for FailureClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EscalationAction {
    RetrySameTier,
    EscalateTier,
    RecompileContext,
    Replan,
    Defer,
    RepairEnvironment,
    RequestPermission,
    WaitForQuota,
    RequestBudget,
    EscalateHuman,
    Abort,
}

impl EscalationAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RetrySameTier => "retry_same_tier",
            Self::EscalateTier => "escalate_tier",
            Self::RecompileContext => "recompile_context",
            Self::Replan => "replan",
            Self::Defer => "defer",
            Self::RepairEnvironment => "repair_environment",
            Self::RequestPermission => "request_permission",
            Self::WaitForQuota => "wait_for_quota",
            Self::RequestBudget => "request_budget",
            Self::EscalateHuman => "escalate_human",
            Self::Abort => "abort",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "retry_same_tier" => Self::RetrySameTier,
            "escalate_tier" => Self::EscalateTier,
            "recompile_context" => Self::RecompileContext,
            "replan" => Self::Replan,
            "defer" => Self::Defer,
            "repair_environment" => Self::RepairEnvironment,
            "request_permission" => Self::RequestPermission,
            "wait_for_quota" => Self::WaitForQuota,
            "request_budget" => Self::RequestBudget,
            "escalate_human" => Self::EscalateHuman,
            "abort" => Self::Abort,
            _ => return None,
        })
    }

    /// Whether this action should ever increase the model tier. Environment,
    /// dependency, tool, permission, budget, and quota failures must not.
    pub fn escalates_tier(self) -> bool {
        matches!(self, Self::EscalateTier)
    }
}

impl std::fmt::Display for EscalationAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ============================================================
// Input
// ============================================================

/// Structured, normalized input to the classifier. It never includes prompt
/// length as a difficulty signal.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskIntelligenceInput {
    pub project_id: Uuid,
    pub work_item_id: Option<Uuid>,
    pub task_id: Option<Uuid>,

    pub title: String,
    pub description: String,
    pub task_type: Option<String>,
    pub tags: Vec<String>,
    pub acceptance_criteria: Vec<String>,
    pub test_commands: Vec<String>,

    pub dependency_count: usize,
    pub has_definition_of_done: bool,
    pub priority: Priority,

    // Context metadata from the V0.41 Context Compiler (not the renderer).
    pub context_truncated: bool,
    pub context_item_count: usize,
    pub context_decision_count: usize,
    pub context_risk_count: usize,

    // Historical execution evidence (previous outcomes for this work item).
    pub prior_attempts: u32,
    pub prior_failures: Vec<FailureClass>,
    pub prior_tier: Option<ModelTier>,
    pub prior_success: bool,
}

impl TaskIntelligenceInput {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            project_id: Uuid::nil(),
            work_item_id: None,
            task_id: None,
            title: title.into(),
            description: String::new(),
            task_type: None,
            tags: Vec::new(),
            acceptance_criteria: Vec::new(),
            test_commands: Vec::new(),
            dependency_count: 0,
            has_definition_of_done: false,
            priority: Priority::Normal,
            context_truncated: false,
            context_item_count: 0,
            context_decision_count: 0,
            context_risk_count: 0,
            prior_attempts: 0,
            prior_failures: Vec::new(),
            prior_tier: None,
            prior_success: false,
        }
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    pub fn with_task_type(mut self, task_type: impl Into<String>) -> Self {
        self.task_type = Some(task_type.into());
        self
    }

    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    pub fn with_acceptance(mut self, criteria: Vec<String>) -> Self {
        self.acceptance_criteria = criteria;
        self
    }

    pub fn with_test_commands(mut self, commands: Vec<String>) -> Self {
        self.test_commands = commands;
        self
    }

    pub fn with_dependency_count(mut self, count: usize) -> Self {
        self.dependency_count = count;
        self
    }

    pub fn with_definition_of_done(mut self, present: bool) -> Self {
        self.has_definition_of_done = present;
        self
    }

    pub fn with_priority(mut self, priority: Priority) -> Self {
        self.priority = priority;
        self
    }

    pub fn with_context(
        mut self,
        truncated: bool,
        item_count: usize,
        decision_count: usize,
        risk_count: usize,
    ) -> Self {
        self.context_truncated = truncated;
        self.context_item_count = item_count;
        self.context_decision_count = decision_count;
        self.context_risk_count = risk_count;
        self
    }

    pub fn with_history(
        mut self,
        attempts: u32,
        failures: Vec<FailureClass>,
        prior_tier: Option<ModelTier>,
        prior_success: bool,
    ) -> Self {
        self.prior_attempts = attempts;
        self.prior_failures = failures;
        self.prior_tier = prior_tier;
        self.prior_success = prior_success;
        self
    }

    /// Lowercased, sorted copy used for both hashing and keyword scanning.
    pub fn normalized(&self) -> Self {
        let mut out = self.clone();
        out.tags = sorted_lower(&self.tags);
        out.acceptance_criteria = sorted_lower(&self.acceptance_criteria);
        out.test_commands = sorted_lower(&self.test_commands);
        out.prior_failures.sort();
        out
    }

    pub fn normalized_text(&self) -> String {
        let mut parts = vec![
            self.title.to_lowercase(),
            self.description.to_lowercase(),
            self.task_type.clone().unwrap_or_default().to_lowercase(),
            self.acceptance_criteria.join(" ").to_lowercase(),
            self.test_commands.join(" ").to_lowercase(),
        ];
        parts.push(self.tags.join(" ").to_lowercase());
        parts.join(" \n ")
    }

    /// Deterministic input hash (classifier version + normalized input).
    pub fn input_hash(&self) -> String {
        let normalized = self.normalized();
        let bytes = serde_json::to_vec(&(CLASSIFIER_VERSION, &normalized)).unwrap_or_default();
        let digest = Sha256::digest(&bytes);
        format!("sha256:{}", hex::encode(digest))
    }
}

fn sorted_lower(items: &[String]) -> Vec<String> {
    let mut v: Vec<String> = items.iter().map(|s| s.to_lowercase()).collect();
    v.sort();
    v.dedup();
    v
}

// ============================================================
// Records
// ============================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskProfile {
    pub profile_id: Uuid,
    pub project_id: Uuid,
    pub work_item_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub task_kind: TaskKind,
    pub secondary_flags: Vec<String>,
    pub complexity_score: f32,
    pub risk_score: f32,
    pub ambiguity_score: f32,
    pub reasoning_score: f32,
    pub scope_score: f32,
    pub verification_score: f32,
    pub context_score: f32,
    pub reversibility_score: f32,
    pub recommended_tier: ModelTier,
    pub verification_requirement: VerificationRequirement,
    pub required_capabilities: Vec<CapabilityRequirement>,
    pub confidence: f32,
    pub reasons: Vec<String>,
    pub classifier_version: String,
    pub input_hash: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct NewTaskProfile {
    pub project_id: Uuid,
    pub work_item_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub task_kind: TaskKind,
    pub secondary_flags: Vec<String>,
    pub complexity_score: f32,
    pub risk_score: f32,
    pub ambiguity_score: f32,
    pub reasoning_score: f32,
    pub scope_score: f32,
    pub verification_score: f32,
    pub context_score: f32,
    pub reversibility_score: f32,
    pub recommended_tier: ModelTier,
    pub verification_requirement: VerificationRequirement,
    pub required_capabilities: Vec<CapabilityRequirement>,
    pub confidence: f32,
    pub reasons: Vec<String>,
    pub classifier_version: String,
    pub input_hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RetryPolicy {
    pub max_same_tier_retries: u32,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_same_tier_retries: 2,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoutingDecision {
    pub routing_id: Uuid,
    pub profile_id: Uuid,
    pub project_id: Uuid,
    pub work_item_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub role: String,
    pub purpose: String,
    pub required_tier: ModelTier,
    pub preferred_tier: ModelTier,
    pub required_capabilities: Vec<CapabilityRequirement>,
    pub reasoning_effort: ReasoningEffort,
    pub context_requirement: ContextRequirement,
    pub verification_requirement: VerificationRequirement,
    pub retry_policy: RetryPolicy,
    pub escalation_policy: crate::intelligence::escalation::EscalationPolicy,
    pub cost_preference: CostPreference,
    pub reasons: Vec<String>,
    pub router_version: String,
    pub input_hash: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct NewRoutingDecision {
    pub profile_id: Uuid,
    pub project_id: Uuid,
    pub work_item_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub role: String,
    pub purpose: String,
    pub required_tier: ModelTier,
    pub preferred_tier: ModelTier,
    pub required_capabilities: Vec<CapabilityRequirement>,
    pub reasoning_effort: ReasoningEffort,
    pub context_requirement: ContextRequirement,
    pub verification_requirement: VerificationRequirement,
    pub retry_policy: RetryPolicy,
    pub escalation_policy: crate::intelligence::escalation::EscalationPolicy,
    pub cost_preference: CostPreference,
    pub reasons: Vec<String>,
    pub router_version: String,
    pub input_hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoutingOutcome {
    pub outcome_id: Uuid,
    pub routing_id: Uuid,
    pub project_id: Uuid,
    pub work_item_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub selected_tier: ModelTier,
    pub success: bool,
    pub failure_class: Option<FailureClass>,
    pub attempts: u32,
    pub duration_ms: Option<u32>,
    pub artifact_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct NewRoutingOutcome {
    pub routing_id: Uuid,
    pub project_id: Uuid,
    pub work_item_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub selected_tier: ModelTier,
    pub success: bool,
    pub failure_class: Option<FailureClass>,
    pub attempts: u32,
    pub duration_ms: Option<u32>,
    pub artifact_id: Option<Uuid>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EscalationDecision {
    pub escalation_id: Uuid,
    pub routing_id: Option<Uuid>,
    pub project_id: Uuid,
    pub action: EscalationAction,
    pub from_tier: ModelTier,
    pub to_tier: Option<ModelTier>,
    pub failure_class: FailureClass,
    pub reason: String,
    pub retry_after: Option<DateTime<Utc>>,
    pub requires_human: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct NewEscalationDecision {
    pub routing_id: Option<Uuid>,
    pub project_id: Uuid,
    pub action: EscalationAction,
    pub from_tier: ModelTier,
    pub to_tier: Option<ModelTier>,
    pub failure_class: FailureClass,
    pub reason: String,
    pub retry_after: Option<DateTime<Utc>>,
    pub requires_human: bool,
}

/// Collected routing telemetry: one row per (task kind, selected tier).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskKindTierStats {
    pub task_kind: TaskKind,
    pub tier: ModelTier,
    pub attempts: i64,
    pub successes: i64,
    pub failure_rate: f64,
    pub average_attempts: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enums_round_trip() {
        for t in [ModelTier::T0, ModelTier::T1, ModelTier::T2, ModelTier::T3] {
            assert_eq!(ModelTier::parse(t.as_str()), Some(t));
        }
        assert_eq!(ModelTier::T0.escalate(), Some(ModelTier::T1));
        assert_eq!(ModelTier::T3.escalate(), None, "there is no T4");
        assert_eq!(ModelTier::T0.max(ModelTier::T2), ModelTier::T2);

        for p in [FailureClass::ReasoningFailure, FailureClass::QuotaFailure] {
            assert_eq!(FailureClass::parse(p.as_str()), Some(p));
        }
        for a in [
            EscalationAction::RetrySameTier,
            EscalationAction::EscalateTier,
            EscalationAction::RecompileContext,
        ] {
            assert_eq!(EscalationAction::parse(a.as_str()), Some(a));
        }
        assert!(EscalationAction::EscalateTier.escalates_tier());
        assert!(!EscalationAction::RepairEnvironment.escalates_tier());
        assert!(!EscalationAction::WaitForQuota.escalates_tier());
    }

    #[test]
    fn input_hash_is_order_independent_and_stable() {
        let a = TaskIntelligenceInput::new("Fix thing").with_tags(vec!["b".into(), "a".into()]);
        let b = TaskIntelligenceInput::new("Fix thing").with_tags(vec!["a".into(), "b".into()]);
        assert_eq!(a.input_hash(), b.input_hash());

        let c = TaskIntelligenceInput::new("Fix other thing");
        assert_ne!(a.input_hash(), c.input_hash());
        assert!(a.input_hash().starts_with("sha256:"));
    }
}
