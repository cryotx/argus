//! V0.42 failure classification and escalation policy.
//!
//! The single most important rule: **not every failure justifies a bigger
//! model.** Environment, dependency, tool, permission, budget, and quota
//! failures are retried/deferred/repaired; only reasoning/implementation/
//! verification (and context) failures may raise the tier, and never past T3.

use serde::{Deserialize, Serialize};

use crate::task_types::TaskErrorType;

use super::types::{EscalationAction, FailureClass, ModelTier};

pub const ESCALATION_POLICY_VERSION: &str = "escalation-v1";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EscalationPolicy {
    pub version: String,
    /// Same-tier retries allowed before escalating an unclassified failure.
    pub max_same_tier_retries: u32,
    /// Repeated implementation failures before escalating one tier.
    pub escalate_after_implementation_failures: u32,
    /// Repeated verification failures before escalating one tier.
    pub escalate_after_verification_failures: u32,
    pub max_tier: ModelTier,
}

impl Default for EscalationPolicy {
    fn default() -> Self {
        Self {
            version: ESCALATION_POLICY_VERSION.to_string(),
            max_same_tier_retries: 2,
            escalate_after_implementation_failures: 2,
            escalate_after_verification_failures: 2,
            max_tier: ModelTier::T3,
        }
    }
}

/// Pure escalation result — no ids/timestamps, so it is deterministic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EscalationChoice {
    pub action: EscalationAction,
    pub to_tier: Option<ModelTier>,
    pub failure_class: FailureClass,
    pub reason: String,
    pub requires_human: bool,
}

impl EscalationPolicy {
    /// Map a scheduler error type onto a failure class.
    pub fn classify_error(error_type: Option<&TaskErrorType>) -> FailureClass {
        match error_type {
            Some(TaskErrorType::CompilationFailed) => FailureClass::ImplementationFailure,
            Some(TaskErrorType::TestsFailed) => FailureClass::VerificationFailure,
            Some(TaskErrorType::ReviewRejected) => FailureClass::VerificationFailure,
            Some(TaskErrorType::WorkerCrashed) => FailureClass::TransientFailure,
            Some(TaskErrorType::Timeout) => FailureClass::TransientFailure,
            Some(TaskErrorType::DependencyFailed) => FailureClass::DependencyFailure,
            Some(TaskErrorType::ResourceExhausted) => FailureClass::EnvironmentFailure,
            Some(TaskErrorType::SecurityViolation) => FailureClass::SafetyPolicyBlocked,
            Some(TaskErrorType::CheckpointFailed) => FailureClass::ToolFailure,
            Some(TaskErrorType::Unknown) | None => FailureClass::Unknown,
        }
    }

    /// Decide what to do after a failure at `from_tier` with `attempts_at_tier`
    /// attempts already spent at that tier.
    pub fn decide(
        &self,
        failure: FailureClass,
        from_tier: ModelTier,
        attempts_at_tier: u32,
        context_truncated: bool,
    ) -> EscalationChoice {
        use EscalationAction::*;
        use FailureClass as F;

        let choice = |action, to_tier, requires_human, reason: String| EscalationChoice {
            action,
            to_tier,
            failure_class: failure,
            reason,
            requires_human,
        };

        match failure {
            F::TransientFailure => choice(
                RetrySameTier,
                None,
                false,
                "transient failure: retry the same tier".to_string(),
            ),
            F::EnvironmentFailure => choice(
                RepairEnvironment,
                None,
                false,
                "environment failure: repair/defer; a higher tier would not help".to_string(),
            ),
            F::DependencyFailure => choice(
                Defer,
                None,
                false,
                "dependency unavailable: defer until it recovers".to_string(),
            ),
            F::ToolFailure => choice(
                RepairEnvironment,
                None,
                false,
                "tool failure: repair the tool/environment".to_string(),
            ),
            F::PermissionFailure => choice(
                RequestPermission,
                None,
                true,
                "permission denied: request access; not a model-capability problem".to_string(),
            ),
            F::BudgetFailure => choice(
                RequestBudget,
                None,
                true,
                "budget exhausted: request budget; never spend a higher tier".to_string(),
            ),
            F::QuotaFailure => choice(
                WaitForQuota,
                None,
                false,
                "quota exhausted: wait for reset; never spend a higher tier".to_string(),
            ),
            F::SafetyPolicyBlocked => choice(
                EscalateHuman,
                None,
                true,
                "safety policy blocked the action: needs a human decision".to_string(),
            ),
            F::RequirementAmbiguous => choice(
                Replan,
                None,
                false,
                "requirements are ambiguous: replan before spending more".to_string(),
            ),
            F::ContextInsufficient => {
                if context_truncated {
                    choice(
                        RecompileContext,
                        None,
                        false,
                        "context was truncated: recompile context before escalating".to_string(),
                    )
                } else {
                    self.escalate_or_human(failure, from_tier)
                }
            }
            F::ReasoningFailure => self.escalate_or_human(failure, from_tier),
            F::ImplementationFailure => {
                if attempts_at_tier >= self.escalate_after_implementation_failures {
                    self.escalate_or_human(failure, from_tier)
                } else {
                    choice(
                        RetrySameTier,
                        None,
                        false,
                        "implementation failure: retry the same tier once more".to_string(),
                    )
                }
            }
            F::VerificationFailure => {
                if context_truncated {
                    choice(
                        RecompileContext,
                        None,
                        false,
                        "verification failed with truncated context: recompile context first"
                            .to_string(),
                    )
                } else if attempts_at_tier >= self.escalate_after_verification_failures {
                    self.escalate_or_human(failure, from_tier)
                } else {
                    choice(
                        RetrySameTier,
                        None,
                        false,
                        "verification failure: retry the same tier".to_string(),
                    )
                }
            }
            F::Unknown => {
                if attempts_at_tier < self.max_same_tier_retries {
                    choice(
                        RetrySameTier,
                        None,
                        false,
                        "unclassified failure: retry the same tier".to_string(),
                    )
                } else {
                    self.escalate_or_human(failure, from_tier)
                }
            }
        }
    }

    fn escalate_or_human(&self, failure: FailureClass, from_tier: ModelTier) -> EscalationChoice {
        match from_tier.escalate() {
            Some(next) if next.rank() <= self.max_tier.rank() => EscalationChoice {
                action: EscalationAction::EscalateTier,
                to_tier: Some(next),
                failure_class: failure,
                reason: format!("escalating {from_tier} -> {next}: {failure}"),
                requires_human: false,
            },
            _ => EscalationChoice {
                action: EscalationAction::EscalateHuman,
                to_tier: None,
                failure_class: failure,
                reason: format!(
                    "already at {} (max {}): human / replan required",
                    from_tier, self.max_tier
                ),
                requires_human: true,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> EscalationPolicy {
        EscalationPolicy::default()
    }

    #[test]
    fn reasoning_failure_may_escalate_one_tier() {
        let d = policy().decide(FailureClass::ReasoningFailure, ModelTier::T0, 1, false);
        assert_eq!(d.action, EscalationAction::EscalateTier);
        assert_eq!(d.to_tier, Some(ModelTier::T1));
        assert!(!d.requires_human);
    }

    #[test]
    fn transient_failure_retries_same_tier() {
        let d = policy().decide(FailureClass::TransientFailure, ModelTier::T1, 3, false);
        assert_eq!(d.action, EscalationAction::RetrySameTier);
        assert_eq!(d.to_tier, None);
        assert!(!d.action.escalates_tier());
    }

    #[test]
    fn environment_failure_never_escalates_model() {
        let d = policy().decide(FailureClass::EnvironmentFailure, ModelTier::T2, 5, false);
        assert_eq!(d.action, EscalationAction::RepairEnvironment);
        assert_eq!(d.to_tier, None);
        assert!(
            !d.action.escalates_tier(),
            "ENVIRONMENT FAILURE ESCALATES MODEL: NO"
        );
    }

    #[test]
    fn budget_and_quota_failures_never_escalate_model() {
        for f in [FailureClass::BudgetFailure, FailureClass::QuotaFailure] {
            let d = policy().decide(f, ModelTier::T2, 5, false);
            assert!(!d.action.escalates_tier(), "{f} must not escalate");
            assert_eq!(d.to_tier, None);
        }
    }

    #[test]
    fn context_insufficient_recompiles_first() {
        let d = policy().decide(FailureClass::ContextInsufficient, ModelTier::T1, 1, true);
        assert_eq!(d.action, EscalationAction::RecompileContext);
        assert_eq!(d.to_tier, None);
    }

    #[test]
    fn repeated_reasoning_failure_at_t3_escalates_to_human() {
        let d = policy().decide(FailureClass::ReasoningFailure, ModelTier::T3, 4, false);
        assert_eq!(d.action, EscalationAction::EscalateHuman);
        assert!(d.requires_human);
        assert_eq!(d.to_tier, None, "there is no T4");
    }

    #[test]
    fn implementation_failure_retries_then_escalates() {
        let p = policy();
        assert_eq!(
            p.decide(FailureClass::ImplementationFailure, ModelTier::T1, 1, false)
                .action,
            EscalationAction::RetrySameTier
        );
        let d = p.decide(FailureClass::ImplementationFailure, ModelTier::T1, 2, false);
        assert_eq!(d.action, EscalationAction::EscalateTier);
        assert_eq!(d.to_tier, Some(ModelTier::T2));
    }

    #[test]
    fn error_types_map_to_failure_classes() {
        assert_eq!(
            EscalationPolicy::classify_error(Some(&TaskErrorType::ResourceExhausted)),
            FailureClass::EnvironmentFailure
        );
        assert_eq!(
            EscalationPolicy::classify_error(Some(&TaskErrorType::TestsFailed)),
            FailureClass::VerificationFailure
        );
        assert_eq!(
            EscalationPolicy::classify_error(Some(&TaskErrorType::SecurityViolation)),
            FailureClass::SafetyPolicyBlocked
        );
    }
}
