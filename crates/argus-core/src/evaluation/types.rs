use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::project_state::WorkItemRecord;
use crate::types::ArgusResult;

pub const EVALUATOR_VERSION: &str = "evaluation-v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvaluationOutcome {
    Passed,
    Failed,
    Indeterminate,
}

impl EvaluationOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Indeterminate => "indeterminate",
        }
    }
}

/// Evidence is loaded by ARGUS, never supplied as an Agent's claimed result.
#[derive(Clone, Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct TaskEvidence {
    pub task_id: Uuid,
    pub project_id: Uuid,
    pub state: String,
    pub execution_id: Option<Uuid>,
    pub artifact_id: Option<Uuid>,
    pub artifact_project_id: Option<Uuid>,
    pub artifact_execution_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub workspace_project_id: Option<Uuid>,
    pub workspace_task_id: Option<Uuid>,
    pub workspace_execution_id: Option<Uuid>,
    pub worktree_path: Option<String>,
    pub base_revision: Option<String>,
    pub commit_sha: Option<String>,
    pub files_changed: Option<serde_json::Value>,
    pub quality_status: Option<String>,
    pub quality_summary: Option<String>,
    pub task_contract: serde_json::Value,
}

#[derive(Clone, Debug)]
pub struct EvaluationContext {
    pub work_item: WorkItemRecord,
    pub tasks: Vec<TaskEvidence>,
}

impl EvaluationContext {
    /// Lifecycle timestamps/status are excluded: promotion itself must not
    /// invalidate evidence. Changes to the task contract or DoD do invalidate it.
    pub fn input_hash(&self) -> ArgusResult<String> {
        let bytes = serde_json::to_vec(&serde_json::json!({
            "version": EVALUATOR_VERSION,
            "work_item": {
                "id": self.work_item.work_item_id,
                "project_id": self.work_item.project_id,
                "title": self.work_item.title,
                "description": self.work_item.description,
                "definition_of_done": self.work_item.definition_of_done,
            },
            "tasks": self.tasks,
        }))?;
        Ok(format!("sha256:{}", hex::encode(Sha256::digest(bytes))))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EvaluationCheck {
    pub evaluator: String,
    pub outcome: EvaluationOutcome,
    pub summary: String,
    pub artifact_ids: Vec<Uuid>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EvaluationReport {
    pub evaluation_id: Uuid,
    pub project_id: Uuid,
    pub work_item_id: Uuid,
    pub input_hash: String,
    pub evaluator_version: String,
    pub outcome: EvaluationOutcome,
    pub checks: Vec<EvaluationCheck>,
    pub created_at: DateTime<Utc>,
}

/// Every configured evaluator is required. No evaluator, skipped evidence,
/// or an unavailable verifier can ever produce a passing report.
pub fn aggregate(checks: &[EvaluationCheck]) -> EvaluationOutcome {
    if checks
        .iter()
        .any(|c| c.outcome == EvaluationOutcome::Failed)
    {
        EvaluationOutcome::Failed
    } else if checks.is_empty()
        || checks
            .iter()
            .any(|c| c.outcome == EvaluationOutcome::Indeterminate)
    {
        EvaluationOutcome::Indeterminate
    } else {
        EvaluationOutcome::Passed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_or_failed_required_checks_cannot_pass() {
        let check = |outcome| EvaluationCheck {
            evaluator: "test".into(),
            outcome,
            summary: String::new(),
            artifact_ids: vec![],
        };
        assert_eq!(aggregate(&[]), EvaluationOutcome::Indeterminate);
        assert_eq!(
            aggregate(&[
                check(EvaluationOutcome::Passed),
                check(EvaluationOutcome::Indeterminate)
            ]),
            EvaluationOutcome::Indeterminate
        );
        assert_eq!(
            aggregate(&[
                check(EvaluationOutcome::Indeterminate),
                check(EvaluationOutcome::Failed)
            ]),
            EvaluationOutcome::Failed
        );
        assert_eq!(
            aggregate(&[check(EvaluationOutcome::Passed)]),
            EvaluationOutcome::Passed
        );
    }
}
