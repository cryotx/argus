//! Intent classification for incoming user messages.
//!
//! Uses a fast, cheap model to categorize intent before routing to appropriate handlers.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Classification confidence level.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Confidence {
    /// 0.9-1.0 — Very confident
    High,
    /// 0.7-0.9 — Moderately confident
    Medium,
    /// 0.5-0.7 — Low confidence, may need clarification
    Low,
    /// <0.5 — Too uncertain, treat as unclear
    Uncertain,
}

impl Confidence {
    /// Create confidence from a float value.
    pub fn from_f32(value: f32) -> Self {
        if value >= 0.9 {
            Confidence::High
        } else if value >= 0.7 {
            Confidence::Medium
        } else if value >= 0.5 {
            Confidence::Low
        } else {
            Confidence::Uncertain
        }
    }

    /// Create confidence from raw float (alias for from_f32).
    pub fn from_raw(value: f32) -> Self {
        Self::from_f32(value)
    }

    /// Convert confidence to raw float (uses midpoint of range).
    pub fn to_raw(&self) -> f32 {
        match self {
            Confidence::High => 0.95,
            Confidence::Medium => 0.8,
            Confidence::Low => 0.6,
            Confidence::Uncertain => 0.25,
        }
    }

    /// Get the confidence as a float range (min, max).
    pub fn as_range(&self) -> (f32, f32) {
        match self {
            Confidence::High => (0.9, 1.0),
            Confidence::Medium => (0.7, 0.9),
            Confidence::Low => (0.5, 0.7),
            Confidence::Uncertain => (0.0, 0.5),
        }
    }

    /// Check if confidence is high enough to proceed without clarification.
    pub fn is_actionable(&self) -> bool {
        matches!(self, Confidence::High | Confidence::Medium)
    }
}

/// Priority levels for tasks and projects.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Priority {
    Critical,
    High,
    Medium,
    Low,
    Lowest,
}

/// Scope for status queries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueryScope {
    AllProjects,
    ActiveProjects,
    Project(Uuid),
    Task(Uuid),
}

/// User intent classification result.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "intent", rename_all = "snake_case")]
pub enum Intent {
    // === Project Management ===
    CreateProject {
        description: String,
    },
    ModifyProject {
        project_hint: String,
        changes: String,
    },
    PauseProject {
        project_hint: String,
    },
    ResumeProject {
        project_hint: String,
    },
    DeleteProject {
        project_hint: String,
    },
    ProjectStatus {
        project_hint: Option<String>,
    },
    ListProjects,
    SetProjectPriority {
        project_hint: String,
        priority: Priority,
    },

    // === Feature / Task ===
    SuggestFeature {
        project_hint: String,
        feature: String,
    },
    PrioritizeTask {
        task_hint: String,
        priority: Priority,
    },
    CancelTask {
        task_hint: String,
    },
    RetryTask {
        task_hint: String,
    },

    // === Configuration ===
    ChangeConfig {
        domain: String,
        natural_description: String,
    },

    // === Query ===
    QueryStatus {
        scope: QueryScope,
    },
    QueryLogs {
        project_hint: Option<String>,
        filter: Option<String>,
    },
    QueryBudget,
    QueryWorkers,
    QueryMemory {
        query: String,
    },

    // === Direct Operations ===
    RunCommand {
        target: String,
        command: String,
    },
    GitOperation {
        project_hint: String,
        operation: String,
    },
    DeployNow {
        project_hint: String,
        environment: String,
    },
    Rollback {
        project_hint: String,
    },

    // === Memory ===
    RememberPermanent {
        content: String,
    },
    Forget {
        content: String,
    },
    Recall {
        query: String,
    },

    // === Tools ===
    InstallMcp {
        description: String,
    },
    InstallSkill {
        description: String,
    },
    RemoveTool {
        name: String,
    },
    ListTools,

    // === Social / Platform ===
    ConnectPlatform {
        platform: String,
        config: String,
    },
    DisconnectPlatform {
        platform: String,
    },

    // === Environment ===
    SetupServer {
        description: String,
    },
    DiagnoseServer,

    // === Self-Evolution ===
    SelfImprove {
        description: String,
    },

    // === Multi-Project ===
    CrossProjectAction {
        description: String,
    },

    // === Conversational ===
    Greeting,
    Help,
    Feedback {
        content: String,
    },
    Clarification {
        context: String,
    },
    Approval {
        action_id: String,
    },
    Rejection {
        action_id: String,
        reason: Option<String>,
    },

    // === Unknown ===
    Unclear {
        original: String,
    },
}

/// Full classification result including confidence.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClassificationResult {
    pub intent: Intent,
    pub confidence: Confidence,
    pub raw_confidence: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_confidence_from_f32() {
        assert_eq!(Confidence::from_f32(0.95), Confidence::High);
        assert_eq!(Confidence::from_f32(0.8), Confidence::Medium);
        assert_eq!(Confidence::from_f32(0.6), Confidence::Low);
        assert_eq!(Confidence::from_f32(0.3), Confidence::Uncertain);
    }

    #[test]
    fn test_confidence_actionable() {
        assert!(Confidence::High.is_actionable());
        assert!(Confidence::Medium.is_actionable());
        assert!(!Confidence::Low.is_actionable());
        assert!(!Confidence::Uncertain.is_actionable());
    }

    #[test]
    fn test_intent_serialization() {
        let intent = Intent::CreateProject {
            description: "Build a web app".to_string(),
        };
        let json = serde_json::to_string(&intent).unwrap();
        assert!(json.contains("create_project"));

        let deserialized: Intent = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, intent);
    }

    #[test]
    fn test_classification_result_serialization() {
        let result = ClassificationResult {
            intent: Intent::Greeting,
            confidence: Confidence::High,
            raw_confidence: 0.95,
        };

        let json = serde_json::to_string(&result).unwrap();
        let deserialized: ClassificationResult = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.confidence, Confidence::High);
        assert!((deserialized.raw_confidence - 0.95).abs() < 0.001);
    }
}
