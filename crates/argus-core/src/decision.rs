//! Decision Engine — determines execution strategy based on action risk.
//!
//! Decides whether to auto-execute, wait for approval, or use timed auto-execution.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use uuid::Uuid;

/// Decision level — how much human oversight is required.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionLevel {
    /// Execute immediately, report after.
    Auto,
    /// Notify owner, execute after delay if no objection.
    TimedAuto { delay_seconds: u64 },
    /// Wait for explicit owner approval.
    WaitForApproval,
}

impl DecisionLevel {
    /// Get the default delay for timed auto.
    pub fn default_delay() -> Duration {
        Duration::from_secs(600) // 10 minutes
    }

    /// Create a TimedAuto with default delay.
    pub fn timed_auto_default() -> Self {
        DecisionLevel::TimedAuto { delay_seconds: 600 }
    }

    /// Create a TimedAuto with custom delay.
    pub fn timed_auto(seconds: u64) -> Self {
        DecisionLevel::TimedAuto {
            delay_seconds: seconds,
        }
    }

    /// Whether this level requires explicit approval.
    pub fn requires_approval(self) -> bool {
        matches!(self, DecisionLevel::WaitForApproval)
    }

    /// Whether this level can proceed without approval.
    pub fn can_proceed(self) -> bool {
        !matches!(self, DecisionLevel::WaitForApproval)
    }
}

/// Action types that can be decided upon.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    // === Always Auto ===
    CreateFile { path: String },
    ModifyCode { path: String },
    RunTests { command: String },
    GitCommit { message: String },
    GitPush { branch: String, protected: bool },
    InstallDependency { name: String },
    RunLint { command: String },

    // === Timed Auto ===
    CreateProject { name: String },
    ChangeTechStack { from: String, to: String },
    DeployStaging { environment: String },
    InstallMcpServer { name: String },
    InstallSkill { name: String },

    // === Must Approve ===
    DeployProduction { environment: String },
    DeleteProject { name: String },
    DeleteData { description: String },
    SpendMoney { amount_usd: f64 },
    ModifySecurityConfig { component: String },
    AccessSensitiveData { data_type: String },
    SelfCoreModification { component: String },

    // === Generic actions ===
    Generic { name: String, risk: RiskLevel },
}

/// Risk level for actions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
}

/// Owner approval mode preference.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    /// Everything requires approval.
    ManualAll,
    /// Everything auto-executes.
    FullAuto,
    /// Use Argus's risk assessment.
    Smart,
}

/// Owner profile for decision making.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OwnerProfile {
    pub approval_mode: ApprovalMode,
    pub budget_threshold_usd: f64,
}

impl Default for OwnerProfile {
    fn default() -> Self {
        Self {
            approval_mode: ApprovalMode::Smart,
            budget_threshold_usd: 100.0,
        }
    }
}

/// Decision Engine — determines execution strategy.
#[derive(Clone, Debug)]
pub struct DecisionEngine {
    owner: OwnerProfile,
}

impl DecisionEngine {
    /// Create a new decision engine.
    pub fn new(owner: OwnerProfile) -> Self {
        Self { owner }
    }

    /// Determine the decision level for an action.
    pub fn decide(&self, action: &Action, _context: &DecisionContext) -> DecisionLevel {
        // Owner's global preference takes precedence
        match self.owner.approval_mode {
            ApprovalMode::FullAuto => return DecisionLevel::Auto,
            ApprovalMode::ManualAll => return DecisionLevel::WaitForApproval,
            ApprovalMode::Smart => {}
        }

        // Risk-based assessment
        match action {
            // === Always Auto ===
            Action::CreateFile { .. } => DecisionLevel::Auto,
            Action::ModifyCode { .. } => DecisionLevel::Auto,
            Action::RunTests { .. } => DecisionLevel::Auto,
            Action::GitCommit { .. } => DecisionLevel::Auto,
            Action::GitPush {
                protected: false, ..
            } => DecisionLevel::Auto,
            Action::InstallDependency { .. } => DecisionLevel::Auto,
            Action::RunLint { .. } => DecisionLevel::Auto,

            // === Timed Auto (notify, proceed if no objection) ===
            Action::CreateProject { .. } => DecisionLevel::timed_auto(600),
            Action::ChangeTechStack { .. } => DecisionLevel::timed_auto(600),
            Action::GitPush {
                protected: true, ..
            } => DecisionLevel::timed_auto(300),
            Action::DeployStaging { .. } => DecisionLevel::timed_auto(300),
            Action::InstallMcpServer { .. } => DecisionLevel::timed_auto(600),
            Action::InstallSkill { .. } => DecisionLevel::timed_auto(600),

            // === Must Approve ===
            Action::DeployProduction { .. } => DecisionLevel::WaitForApproval,
            Action::DeleteProject { .. } => DecisionLevel::WaitForApproval,
            Action::DeleteData { .. } => DecisionLevel::WaitForApproval,
            Action::SpendMoney { amount_usd } => {
                if *amount_usd > self.owner.budget_threshold_usd {
                    DecisionLevel::WaitForApproval
                } else {
                    DecisionLevel::timed_auto(300)
                }
            }
            Action::ModifySecurityConfig { .. } => DecisionLevel::WaitForApproval,
            Action::AccessSensitiveData { .. } => DecisionLevel::WaitForApproval,
            Action::SelfCoreModification { .. } => DecisionLevel::WaitForApproval,

            // === Default: based on risk assessment ===
            Action::Generic { risk, .. } => match risk {
                RiskLevel::Low => DecisionLevel::Auto,
                RiskLevel::Medium => DecisionLevel::timed_auto(600),
                RiskLevel::High => DecisionLevel::WaitForApproval,
            },
        }
    }

    /// Batch decide on multiple actions.
    pub fn decide_batch(
        &self,
        actions: &[Action],
        context: &DecisionContext,
    ) -> Vec<DecisionLevel> {
        actions.iter().map(|a| self.decide(a, context)).collect()
    }
}

/// Context for decision making.
#[derive(Clone, Debug, Default)]
pub struct DecisionContext {
    pub project_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub time_of_day: Option<DateTime<Utc>>,
}

/// Outcome of a decision.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionOutcome {
    Success { details: String },
    Failure { error: String, lesson: String },
    Overridden { by: String, reason: String },
}

/// Record of a decision for learning and auditing.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DecisionRecord {
    pub id: Uuid,
    pub timestamp: DateTime<Utc>,

    // Context
    pub project_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub trigger: String,

    // Decision
    pub options_considered: Vec<String>,
    pub chosen_option: usize,
    pub reasoning: String,
    pub confidence: f32,

    // Outcome (filled later)
    pub outcome: Option<DecisionOutcome>,
    pub outcome_recorded_at: Option<DateTime<Utc>>,
}

impl DecisionRecord {
    /// Create a new decision record.
    pub fn new(
        trigger: impl Into<String>,
        options: Vec<impl Into<String>>,
        chosen: usize,
        reasoning: impl Into<String>,
        confidence: f32,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            project_id: None,
            task_id: None,
            trigger: trigger.into(),
            options_considered: options.into_iter().map(Into::into).collect(),
            chosen_option: chosen,
            reasoning: reasoning.into(),
            confidence: confidence.clamp(0.0, 1.0),
            outcome: None,
            outcome_recorded_at: None,
        }
    }

    /// Record the outcome of this decision.
    pub fn record_outcome(&mut self, outcome: DecisionOutcome) {
        self.outcome = Some(outcome);
        self.outcome_recorded_at = Some(Utc::now());
    }

    /// Check if this decision has been evaluated.
    pub fn is_evaluated(&self) -> bool {
        self.outcome.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decision_level_defaults() {
        assert!(DecisionLevel::Auto.can_proceed());
        assert!(!DecisionLevel::Auto.requires_approval());
        assert!(!DecisionLevel::WaitForApproval.can_proceed());
        assert!(DecisionLevel::WaitForApproval.requires_approval());
    }

    #[test]
    fn test_decision_level_timed_auto() {
        let level = DecisionLevel::timed_auto(300);
        assert!(level.can_proceed());
        assert!(!level.requires_approval());
    }

    #[test]
    fn test_decision_engine_full_auto_mode() {
        let owner = OwnerProfile {
            approval_mode: ApprovalMode::FullAuto,
            budget_threshold_usd: 100.0,
        };
        let engine = DecisionEngine::new(owner);
        let context = DecisionContext::default();

        // Everything should be Auto in FullAuto mode
        let deploy_prod = Action::DeployProduction {
            environment: "prod".to_string(),
        };
        assert_eq!(engine.decide(&deploy_prod, &context), DecisionLevel::Auto);

        let delete_project = Action::DeleteProject {
            name: "test".to_string(),
        };
        assert_eq!(
            engine.decide(&delete_project, &context),
            DecisionLevel::Auto
        );
    }

    #[test]
    fn test_decision_engine_manual_mode() {
        let owner = OwnerProfile {
            approval_mode: ApprovalMode::ManualAll,
            budget_threshold_usd: 100.0,
        };
        let engine = DecisionEngine::new(owner);
        let context = DecisionContext::default();

        // Everything should require approval in ManualAll mode
        let create_file = Action::CreateFile {
            path: "test.txt".to_string(),
        };
        assert_eq!(
            engine.decide(&create_file, &context),
            DecisionLevel::WaitForApproval
        );
    }

    #[test]
    fn test_decision_engine_smart_mode_auto() {
        let owner = OwnerProfile::default(); // Smart mode
        let engine = DecisionEngine::new(owner);
        let context = DecisionContext::default();

        // Safe actions are Auto
        let create_file = Action::CreateFile {
            path: "test.txt".to_string(),
        };
        assert_eq!(engine.decide(&create_file, &context), DecisionLevel::Auto);

        let git_commit = Action::GitCommit {
            message: "test".to_string(),
        };
        assert_eq!(engine.decide(&git_commit, &context), DecisionLevel::Auto);
    }

    #[test]
    fn test_decision_engine_smart_mode_timed() {
        let owner = OwnerProfile::default();
        let engine = DecisionEngine::new(owner);
        let context = DecisionContext::default();

        // Medium-risk actions are TimedAuto
        let create_project = Action::CreateProject {
            name: "new-project".to_string(),
        };
        let level = engine.decide(&create_project, &context);
        assert!(matches!(level, DecisionLevel::TimedAuto { .. }));
    }

    #[test]
    fn test_decision_engine_smart_mode_approval() {
        let owner = OwnerProfile::default();
        let engine = DecisionEngine::new(owner);
        let context = DecisionContext::default();

        // High-risk actions require approval
        let deploy_prod = Action::DeployProduction {
            environment: "prod".to_string(),
        };
        assert_eq!(
            engine.decide(&deploy_prod, &context),
            DecisionLevel::WaitForApproval
        );

        let delete_project = Action::DeleteProject {
            name: "test".to_string(),
        };
        assert_eq!(
            engine.decide(&delete_project, &context),
            DecisionLevel::WaitForApproval
        );
    }

    #[test]
    fn test_decision_engine_budget_threshold() {
        let owner = OwnerProfile {
            approval_mode: ApprovalMode::Smart,
            budget_threshold_usd: 50.0,
        };
        let engine = DecisionEngine::new(owner);
        let context = DecisionContext::default();

        // Under threshold: TimedAuto
        let small_spend = Action::SpendMoney { amount_usd: 30.0 };
        let level = engine.decide(&small_spend, &context);
        assert!(matches!(level, DecisionLevel::TimedAuto { .. }));

        // Over threshold: WaitForApproval
        let large_spend = Action::SpendMoney { amount_usd: 100.0 };
        assert_eq!(
            engine.decide(&large_spend, &context),
            DecisionLevel::WaitForApproval
        );
    }

    #[test]
    fn test_decision_record_creation() {
        let record = DecisionRecord::new(
            "Deploy request",
            vec!["Deploy now", "Wait for tests"],
            0,
            "All tests passed",
            0.85,
        );

        assert_eq!(record.trigger, "Deploy request");
        assert_eq!(record.options_considered.len(), 2);
        assert_eq!(record.chosen_option, 0);
        assert!(record.confidence > 0.8);
        assert!(!record.is_evaluated());
    }

    #[test]
    fn test_decision_record_outcome() {
        let mut record =
            DecisionRecord::new("Test decision", vec!["Option A"], 0, "Reasoning", 1.0);

        record.record_outcome(DecisionOutcome::Success {
            details: "Worked perfectly".to_string(),
        });

        assert!(record.is_evaluated());
        assert!(record.outcome_recorded_at.is_some());
    }

    #[test]
    fn test_decision_engine_batch() {
        let owner = OwnerProfile::default();
        let engine = DecisionEngine::new(owner);
        let context = DecisionContext::default();

        let actions = vec![
            Action::CreateFile {
                path: "a.txt".to_string(),
            },
            Action::CreateFile {
                path: "b.txt".to_string(),
            },
            Action::DeployProduction {
                environment: "prod".to_string(),
            },
        ];

        let decisions = engine.decide_batch(&actions, &context);
        assert_eq!(decisions.len(), 3);
        assert_eq!(decisions[0], DecisionLevel::Auto);
        assert_eq!(decisions[1], DecisionLevel::Auto);
        assert_eq!(decisions[2], DecisionLevel::WaitForApproval);
    }
}
