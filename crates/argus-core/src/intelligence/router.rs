//! V0.42 model routing policy.
//!
//! Maps a [`TaskProfile`] + role + purpose onto a [`RoutingDecision`]: a
//! required capability tier, capabilities, reasoning effort, context and
//! verification requirements, and retry/escalation policy. It names no
//! provider or model — V0.44 selects the cheapest model that meets the tier.

use sha2::{Digest, Sha256};

use crate::context::{ContextPurpose, ContextRole};

use super::escalation::EscalationPolicy;
use super::types::*;

pub struct RoutingPolicy;

impl RoutingPolicy {
    pub fn route(
        profile: &TaskProfile,
        role: ContextRole,
        purpose: ContextPurpose,
    ) -> NewRoutingDecision {
        let mut required = profile.recommended_tier;
        let mut verification = profile.verification_requirement;
        let mut capabilities: std::collections::BTreeSet<CapabilityRequirement> =
            profile.required_capabilities.iter().copied().collect();
        let mut reasons = vec![format!(
            "base tier {} from task profile ({})",
            profile.recommended_tier, profile.task_kind
        )];

        // --- Role sensitivity ---
        match role {
            ContextRole::Reviewer => {
                if profile.risk_score >= 0.6 {
                    required = required.max(ModelTier::T2);
                    reasons.push("reviewer role on a risky change requires T2+".to_string());
                }
                verification = verification.max(VerificationRequirement::Strong);
                capabilities.insert(CapabilityRequirement::HighReliability);
            }
            ContextRole::Tester => {
                verification = verification.max(VerificationRequirement::Standard);
                capabilities.insert(CapabilityRequirement::TestGeneration);
            }
            ContextRole::ProjectManager | ContextRole::Chief => {
                if purpose == ContextPurpose::Planning {
                    required = required.max(ModelTier::T1);
                    if profile.complexity_score >= 0.7 {
                        required = required.max(ModelTier::T3);
                        reasons.push("planning a high-complexity change requires T3".to_string());
                    }
                    capabilities.insert(CapabilityRequirement::LongContext);
                }
            }
            _ => {}
        }

        // --- Purpose sensitivity ---
        match purpose {
            ContextPurpose::Planning => {
                if required < ModelTier::T1 {
                    required = ModelTier::T1;
                    reasons.push("planning requires at least T1".to_string());
                }
            }
            ContextPurpose::Review | ContextPurpose::Integration => {
                if profile.risk_score >= 0.6 {
                    required = required.max(ModelTier::T2);
                    reasons.push(format!("{purpose} of a risky change requires T2+"));
                }
                verification = verification.max(VerificationRequirement::Strong);
            }
            _ => {}
        }

        // --- Verification floor (independent of tier) ---
        if profile.risk_score >= 0.85 {
            verification = VerificationRequirement::Critical;
            reasons.push("critical risk: verification floor is Critical".to_string());
        }
        reasons.push(format!("verification requirement {verification}"));

        let reasoning_effort = match required {
            ModelTier::T0 => ReasoningEffort::Low,
            ModelTier::T1 => ReasoningEffort::Medium,
            ModelTier::T2 => ReasoningEffort::High,
            ModelTier::T3 => {
                if profile.risk_score >= 0.85 {
                    ReasoningEffort::VeryHigh
                } else {
                    ReasoningEffort::High
                }
            }
        };

        let context_requirement = match profile.context_score {
            c if c < 0.25 => ContextRequirement::Low,
            c if c < 0.5 => ContextRequirement::Medium,
            c if c < 0.75 => ContextRequirement::High,
            _ => ContextRequirement::VeryHigh,
        };

        let input_hash = routing_hash(&profile.input_hash, role, purpose);
        let required_capabilities: Vec<CapabilityRequirement> = capabilities.into_iter().collect();

        NewRoutingDecision {
            profile_id: profile.profile_id,
            project_id: profile.project_id,
            work_item_id: profile.work_item_id,
            task_id: profile.task_id,
            role: role.as_str().to_string(),
            purpose: purpose.as_str().to_string(),
            required_tier: required,
            preferred_tier: required,
            required_capabilities,
            reasoning_effort,
            context_requirement,
            verification_requirement: verification,
            retry_policy: RetryPolicy::default(),
            escalation_policy: EscalationPolicy::default(),
            cost_preference: CostPreference::CheapestSufficient,
            reasons,
            router_version: ROUTER_VERSION.to_string(),
            input_hash,
        }
    }
}

fn routing_hash(profile_hash: &str, role: ContextRole, purpose: ContextPurpose) -> String {
    let bytes = serde_json::to_vec(&(
        ROUTER_VERSION,
        profile_hash,
        role.as_str(),
        purpose.as_str(),
    ))
    .unwrap_or_default();
    let digest = Sha256::digest(&bytes);
    format!("sha256:{}", hex::encode(digest))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile_with(
        tier: ModelTier,
        risk: f32,
        verification: VerificationRequirement,
    ) -> TaskProfile {
        TaskProfile {
            profile_id: uuid::Uuid::new_v4(),
            project_id: uuid::Uuid::nil(),
            work_item_id: None,
            task_id: None,
            task_kind: TaskKind::FeatureImplementation,
            secondary_flags: Vec::new(),
            complexity_score: 0.4,
            risk_score: risk,
            ambiguity_score: 0.2,
            reasoning_score: 0.4,
            scope_score: 0.4,
            verification_score: 0.4,
            context_score: 0.4,
            reversibility_score: 0.8,
            recommended_tier: tier,
            verification_requirement: verification,
            required_capabilities: vec![CapabilityRequirement::CodeGeneration],
            confidence: 0.7,
            reasons: vec!["base".to_string()],
            classifier_version: CLASSIFIER_VERSION.to_string(),
            input_hash: "sha256:abc".to_string(),
            created_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn reviewer_of_risky_change_raises_tier() {
        let profile = profile_with(ModelTier::T1, 0.7, VerificationRequirement::Strong);
        let d = RoutingPolicy::route(&profile, ContextRole::Reviewer, ContextPurpose::Review);
        assert!(d.required_tier >= ModelTier::T2);
        assert!(d.verification_requirement >= VerificationRequirement::Strong);
        assert!(
            d.required_capabilities
                .contains(&CapabilityRequirement::HighReliability)
        );
    }

    #[test]
    fn developer_security_task_keeps_risk_floor() {
        let profile = profile_with(ModelTier::T2, 0.9, VerificationRequirement::Critical);
        let d = RoutingPolicy::route(&profile, ContextRole::Developer, ContextPurpose::Execution);
        assert_eq!(d.required_tier, ModelTier::T2);
        assert_eq!(
            d.verification_requirement,
            VerificationRequirement::Critical
        );
        assert_eq!(d.reasoning_effort, ReasoningEffort::High);
        assert_eq!(d.cost_preference, CostPreference::CheapestSufficient);
        assert!(!d.reasons.is_empty());
    }

    #[test]
    fn routing_hash_is_deterministic_and_role_sensitive() {
        let profile = profile_with(ModelTier::T1, 0.2, VerificationRequirement::Standard);
        let dev = RoutingPolicy::route(&profile, ContextRole::Developer, ContextPurpose::Execution);
        let dev2 =
            RoutingPolicy::route(&profile, ContextRole::Developer, ContextPurpose::Execution);
        let rev = RoutingPolicy::route(&profile, ContextRole::Reviewer, ContextPurpose::Review);
        assert_eq!(dev.input_hash, dev2.input_hash);
        assert_ne!(dev.input_hash, rev.input_hash);
    }
}
