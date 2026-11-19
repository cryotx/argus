//! Complexity assessment for tasks — determines collaboration mode and risk level.
//!
//! Based on task characteristics, determines whether to use Solo, PairReview, or FullTeam mode.

use crate::decomposer::AtomicTask;
use serde::{Deserialize, Serialize};

/// Individual risk/complexity signals detected in a task.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComplexityFactor {
    /// Touches multiple modules/packages.
    MultiModule,
    /// Touches multiple files across different directories.
    CrossCutting,
    /// Security-sensitive (authentication, authorization, crypto).
    SecuritySensitive,
    /// Involves database schema changes (migration risk).
    DatabaseMigration,
    /// Network or async code with race condition potential.
    ConcurrencyHazard,
    /// Payment, pricing, or financial transaction logic.
    FinancialLogic,
    /// Breaks or changes public API contracts.
    ApiBreakingChange,
    /// Requires coordination with external services/systems.
    ExternalDependency,
    /// Legacy code with limited test coverage.
    LegacyCode,
    /// Performance-critical path.
    PerformanceCritical,
    /// Compliance-related (GDPR, PCI-DSS, etc.).
    ComplianceRelated,
    /// Unfamiliar technology or domain for the team.
    UnfamiliarTech,
}

/// Risk level drives review policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    /// Safe operations — no review needed.
    Low,
    /// Some risk — PairReview recommended.
    Medium,
    /// Significant risk — FullTeam + 2 review rounds.
    High,
    /// Critical risk — FullTeam + 2 rounds + Security Auditor + owner approval.
    Critical,
}

impl RiskLevel {
    /// Get the number of review rounds required.
    pub fn review_rounds(self) -> u8 {
        match self {
            RiskLevel::Low => 0,
            RiskLevel::Medium => 1,
            RiskLevel::High => 2,
            RiskLevel::Critical => 2,
        }
    }

    /// Whether to force a different model for review.
    pub fn requires_cross_model_review(self) -> bool {
        matches!(self, RiskLevel::High | RiskLevel::Critical)
    }

    /// Whether security audit is required.
    pub fn requires_security_audit(self) -> bool {
        matches!(self, RiskLevel::High | RiskLevel::Critical)
    }

    /// Whether owner approval is required.
    pub fn requires_owner_approval(self) -> bool {
        matches!(self, RiskLevel::Critical)
    }
}

/// Collaboration mode based on complexity score.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationMode {
    /// Single worker, no collaboration (complexity < 0.3).
    Solo,
    /// One writes + one reviews (0.3 ≤ complexity < 0.6).
    PairReview,
    /// Full team: Architect → Developer(s) → Reviewer → Tester → Security Auditor → DevOps (complexity ≥ 0.6).
    FullTeam,
}

impl CollaborationMode {
    /// Create mode from a complexity score (0.0 - 1.0).
    pub fn from_score(score: f32) -> Self {
        if score < 0.3 {
            CollaborationMode::Solo
        } else if score < 0.6 {
            CollaborationMode::PairReview
        } else {
            CollaborationMode::FullTeam
        }
    }

    /// Get the minimum complexity score for this mode.
    pub fn min_score(self) -> f32 {
        match self {
            CollaborationMode::Solo => 0.0,
            CollaborationMode::PairReview => 0.3,
            CollaborationMode::FullTeam => 0.6,
        }
    }

    /// Get the maximum complexity score for this mode.
    pub fn max_score(self) -> f32 {
        match self {
            CollaborationMode::Solo => 0.3,
            CollaborationMode::PairReview => 0.6,
            CollaborationMode::FullTeam => 1.0,
        }
    }
}

/// Full complexity assessment for a task.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComplexityAssessment {
    /// Complexity score (0.0 - 1.0).
    pub score: f32,
    /// Detected complexity factors.
    pub factors: Vec<ComplexityFactor>,
    /// Determined collaboration mode.
    pub mode: CollaborationMode,
    /// Determined risk level.
    pub risk_level: RiskLevel,
    /// Human-readable explanation.
    pub explanation: String,
}

impl ComplexityAssessment {
    /// Create a new assessment.
    pub fn new(score: f32, factors: Vec<ComplexityFactor>) -> Self {
        let mode = CollaborationMode::from_score(score);
        let risk_level = Self::determine_risk_level(&factors, score);
        let explanation = Self::generate_explanation(&factors, score, mode, risk_level);

        Self {
            score,
            factors,
            mode,
            risk_level,
            explanation,
        }
    }

    /// Determine risk level based on factors and score.
    fn determine_risk_level(factors: &[ComplexityFactor], score: f32) -> RiskLevel {
        // Critical factors automatically raise to Critical risk
        let has_critical_factor = factors.iter().any(|f| {
            matches!(
                f,
                ComplexityFactor::FinancialLogic | ComplexityFactor::SecuritySensitive
            )
        });

        if has_critical_factor && score >= 0.7 {
            return RiskLevel::Critical;
        }

        // High-risk factors raise to High
        let has_high_risk_factor = factors.iter().any(|f| {
            matches!(
                f,
                ComplexityFactor::DatabaseMigration
                    | ComplexityFactor::ConcurrencyHazard
                    | ComplexityFactor::ApiBreakingChange
                    | ComplexityFactor::ComplianceRelated
            )
        });

        if has_high_risk_factor || score >= 0.7 {
            return RiskLevel::High;
        }

        // Medium-risk factors raise to Medium
        let has_medium_risk_factor = factors.iter().any(|f| {
            matches!(
                f,
                ComplexityFactor::MultiModule
                    | ComplexityFactor::CrossCutting
                    | ComplexityFactor::ExternalDependency
                    | ComplexityFactor::PerformanceCritical
            )
        });

        if has_medium_risk_factor || score >= 0.4 {
            return RiskLevel::Medium;
        }

        RiskLevel::Low
    }

    /// Generate human-readable explanation.
    fn generate_explanation(
        factors: &[ComplexityFactor],
        score: f32,
        mode: CollaborationMode,
        risk: RiskLevel,
    ) -> String {
        let factor_names: Vec<_> = factors
            .iter()
            .map(|f| format!("{:?}", f).to_lowercase().replace("_", " "))
            .collect();

        if factors.is_empty() {
            format!(
                "Score {:.2} → {:?} mode, {:?} risk. No special complexity factors detected.",
                score, mode, risk
            )
        } else {
            format!(
                "Score {:.2} → {:?} mode, {:?} risk. Detected factors: {}.",
                score,
                mode,
                risk,
                factor_names.join(", ")
            )
        }
    }

    /// Check if the assessment is actionable without clarification.
    pub fn is_actionable(&self) -> bool {
        self.score >= 0.0 && self.score <= 1.0
    }
}

/// Assesses task complexity to determine collaboration mode.
#[derive(Clone, Debug, Default)]
pub struct ComplexityAssessor;

impl ComplexityAssessor {
    /// Create a new assessor.
    pub fn new() -> Self {
        Self
    }

    /// Assess the complexity of a task.
    ///
    /// In production, this would:
    /// 1. Run static analysis on task metadata
    /// 2. Call an LLM with an assessment prompt
    /// 3. Combine signals for final score
    pub fn assess(
        &self,
        _task: &AtomicTask,
        _project_context: &ProjectContext,
    ) -> ComplexityAssessment {
        // Placeholder — in production would use real assessment
        ComplexityAssessment::new(0.2, vec![])
    }

    /// Quick assessment based on simple heuristics (no LLM).
    pub fn assess_heuristic(&self, task: &AtomicTask) -> ComplexityAssessment {
        let mut factors = Vec::new();
        let mut score = 0.1_f32; // Base score

        // Factor: Multiple dependencies = more complex
        match task.depends_on.len() {
            0 => {}
            1..=2 => {
                score += 0.05;
            }
            3..=5 => {
                factors.push(ComplexityFactor::MultiModule);
                score += 0.15;
            }
            _ => {
                factors.push(ComplexityFactor::MultiModule);
                factors.push(ComplexityFactor::CrossCutting);
                score += 0.25;
            }
        }

        // Factor: Long estimated time = more complex
        match task.estimated_minutes {
            0..=30 => {}
            31..=120 => {
                score += 0.1;
            }
            121..=240 => {
                score += 0.2;
            }
            _ => {
                factors.push(ComplexityFactor::UnfamiliarTech);
                score += 0.3;
            }
        }

        // Factor: Keywords in description/title
        let text = format!("{} {}", task.title, task.description).to_lowercase();
        if text.contains("database") || text.contains("migration") || text.contains("schema") {
            factors.push(ComplexityFactor::DatabaseMigration);
            score += 0.2;
        }
        if text.contains("security")
            || text.contains("auth")
            || text.contains("password")
            || text.contains("token")
        {
            factors.push(ComplexityFactor::SecuritySensitive);
            score += 0.25;
        }
        if text.contains("payment")
            || text.contains("price")
            || text.contains("transaction")
            || text.contains("billing")
        {
            factors.push(ComplexityFactor::FinancialLogic);
            score += 0.3;
        }
        if text.contains("concurrent")
            || text.contains("async")
            || text.contains("thread")
            || text.contains("race")
        {
            factors.push(ComplexityFactor::ConcurrencyHazard);
            score += 0.25;
        }
        if text.contains("api") && (text.contains("breaking") || text.contains("deprecat")) {
            factors.push(ComplexityFactor::ApiBreakingChange);
            score += 0.2;
        }
        if text.contains("performance") || text.contains("optimize") || text.contains("cache") {
            factors.push(ComplexityFactor::PerformanceCritical);
            score += 0.15;
        }

        // Cap at 1.0
        score = score.min(1.0);

        ComplexityAssessment::new(score, factors)
    }
}

/// Project context for complexity assessment.
#[derive(Clone, Debug, Default)]
pub struct ProjectContext {
    pub tech_stack: Vec<String>,
    pub team_familiarity: f32, // 0.0 - 1.0
    pub test_coverage: f32,    // 0.0 - 1.0
    pub legacy_ratio: f32,     // 0.0 - 1.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decomposer::AcceptanceCriteria;
    use crate::worker_types::WorkerType;
    use std::collections::HashMap;

    fn create_task_with_description(
        title: &str,
        desc: &str,
        minutes: u32,
        deps: Vec<String>,
    ) -> AtomicTask {
        AtomicTask {
            id: "T1".to_string(),
            title: title.to_string(),
            description: desc.to_string(),
            acceptance: AcceptanceCriteria {
                description: "Done".to_string(),
                test_commands: vec![],
                artifacts: vec![],
            },
            depends_on: deps,
            worker_type: WorkerType::OpenCode,
            estimated_minutes: minutes,
            metadata: HashMap::new(),
        }
    }

    #[test]
    fn test_collaboration_mode_from_score() {
        assert_eq!(CollaborationMode::from_score(0.1), CollaborationMode::Solo);
        assert_eq!(
            CollaborationMode::from_score(0.3),
            CollaborationMode::PairReview
        );
        assert_eq!(
            CollaborationMode::from_score(0.5),
            CollaborationMode::PairReview
        );
        assert_eq!(
            CollaborationMode::from_score(0.7),
            CollaborationMode::FullTeam
        );
        assert_eq!(
            CollaborationMode::from_score(0.9),
            CollaborationMode::FullTeam
        );
    }

    #[test]
    fn test_risk_level_review_policy() {
        assert_eq!(RiskLevel::Low.review_rounds(), 0);
        assert_eq!(RiskLevel::Medium.review_rounds(), 1);
        assert_eq!(RiskLevel::High.review_rounds(), 2);
        assert_eq!(RiskLevel::Critical.review_rounds(), 2);

        assert!(!RiskLevel::Low.requires_cross_model_review());
        assert!(!RiskLevel::Medium.requires_cross_model_review());
        assert!(RiskLevel::High.requires_cross_model_review());
        assert!(RiskLevel::Critical.requires_cross_model_review());

        assert!(!RiskLevel::Low.requires_owner_approval());
        assert!(!RiskLevel::Medium.requires_owner_approval());
        assert!(!RiskLevel::High.requires_owner_approval());
        assert!(RiskLevel::Critical.requires_owner_approval());
    }

    #[test]
    fn test_complexity_assessment_basic() {
        let assessment = ComplexityAssessment::new(0.2, vec![]);
        assert_eq!(assessment.mode, CollaborationMode::Solo);
        assert_eq!(assessment.risk_level, RiskLevel::Low);
    }

    #[test]
    fn test_complexity_assessment_with_security_factor() {
        let assessment = ComplexityAssessment::new(0.8, vec![ComplexityFactor::SecuritySensitive]);
        assert_eq!(assessment.mode, CollaborationMode::FullTeam);
        assert_eq!(assessment.risk_level, RiskLevel::Critical);
    }

    #[test]
    fn test_heuristic_simple_task() {
        let assessor = ComplexityAssessor::new();
        let task = create_task_with_description("Fix typo", "Fix a typo in README", 5, vec![]);
        let assessment = assessor.assess_heuristic(&task);

        assert!(assessment.score < 0.3);
        assert_eq!(assessment.mode, CollaborationMode::Solo);
    }

    #[test]
    fn test_heuristic_complex_task() {
        let assessor = ComplexityAssessor::new();
        let task = create_task_with_description(
            "Implement payment flow",
            "Add Stripe payment processing with webhook handling",
            300,
            vec![
                "T1".to_string(),
                "T2".to_string(),
                "T3".to_string(),
                "T4".to_string(),
            ],
        );
        let assessment = assessor.assess_heuristic(&task);

        assert!(assessment.score >= 0.6);
        assert_eq!(assessment.mode, CollaborationMode::FullTeam);
        assert!(
            assessment
                .factors
                .contains(&ComplexityFactor::FinancialLogic)
        );
    }

    #[test]
    fn test_heuristic_database_migration() {
        let assessor = ComplexityAssessor::new();
        let task = create_task_with_description(
            "Add user schema",
            "Create database migration for users table",
            60,
            vec![],
        );
        let assessment = assessor.assess_heuristic(&task);

        assert!(
            assessment
                .factors
                .contains(&ComplexityFactor::DatabaseMigration)
        );
        assert!(assessment.score >= 0.3);
    }

    #[test]
    fn test_heuristic_many_dependencies() {
        let assessor = ComplexityAssessor::new();
        let task = create_task_with_description(
            "Integration task",
            "Wire everything together",
            30,
            vec![
                "A".to_string(),
                "B".to_string(),
                "C".to_string(),
                "D".to_string(),
                "E".to_string(),
                "F".to_string(),
            ],
        );
        let assessment = assessor.assess_heuristic(&task);

        assert!(assessment.factors.contains(&ComplexityFactor::CrossCutting));
        assert!(assessment.score > 0.3);
    }
}
