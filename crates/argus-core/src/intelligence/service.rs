//! V0.42 TaskIntelligenceService — classify, route, record outcomes, and
//! decide escalation. Read-mostly: it persists profiles/decisions/outcomes
//! but never changes Project State or runs a model.

use std::sync::Arc;

use uuid::Uuid;

use crate::context::{ContextCompiler, ContextPurpose, ContextRequest, ContextRole};
use crate::project_state::ProjectStateRepository;
use crate::task_types::TaskErrorType;
use crate::types::{ArgusError, ArgusResult};

use super::classifier::TaskClassifier;
use super::repository::TaskIntelligenceRepository;
use super::router::RoutingPolicy;
use super::types::*;

pub struct TaskIntelligenceService {
    state: Arc<dyn ProjectStateRepository>,
    intel: Arc<dyn TaskIntelligenceRepository>,
}

#[derive(Clone, Debug, Default)]
pub struct OutcomeInput {
    pub success: bool,
    pub failure_class: Option<FailureClass>,
    pub attempts: u32,
    pub duration_ms: Option<u32>,
    pub artifact_id: Option<Uuid>,
}

#[derive(Clone, Debug, Default)]
pub struct FailureInput {
    pub failure_class: Option<FailureClass>,
    pub error_type: Option<TaskErrorType>,
    pub attempts: Option<u32>,
    pub context_truncated: bool,
}

impl TaskIntelligenceService {
    pub fn new(
        state: Arc<dyn ProjectStateRepository>,
        intel: Arc<dyn TaskIntelligenceRepository>,
    ) -> Self {
        Self { state, intel }
    }

    /// Classify a WorkItem and route it for the given role/purpose. Both are
    /// persisted and returned.
    pub async fn classify_work_item(
        &self,
        project_id: Uuid,
        work_item_id: Uuid,
        role: ContextRole,
        purpose: ContextPurpose,
    ) -> ArgusResult<(TaskProfile, RoutingDecision)> {
        self.state
            .get_project(project_id)
            .await?
            .ok_or_else(|| ArgusError::NotFound(format!("project {project_id} not found")))?;
        let work_item = self
            .state
            .get_work_item(work_item_id)
            .await?
            .ok_or_else(|| ArgusError::NotFound(format!("work item {work_item_id} not found")))?;
        if work_item.project_id != project_id {
            return Err(ArgusError::Validation(
                "work item does not belong to this project".to_string(),
            ));
        }

        let dependencies = self.state.list_work_item_dependencies(project_id).await?;
        let dependency_count = dependencies
            .iter()
            .filter(|d| d.work_item_id == work_item_id)
            .count();

        // Context metadata from the V0.41 Context Compiler (structured packet,
        // never the Markdown renderer).
        let context = ContextCompiler::new(self.state.clone())
            .compile(
                &ContextRequest::new(
                    project_id,
                    ContextRole::Developer,
                    ContextPurpose::Execution,
                )
                .with_work_item(work_item_id),
            )
            .await?;

        let outcomes = self
            .intel
            .list_outcomes_for_work_item(project_id, work_item_id)
            .await?;
        let mut prior_failures: Vec<FailureClass> =
            outcomes.iter().filter_map(|o| o.failure_class).collect();
        prior_failures.sort();
        prior_failures.dedup();

        let description = match &work_item.definition_of_done {
            Some(dod) if !work_item.description.is_empty() => {
                format!("{} Definition of done: {dod}", work_item.description)
            }
            Some(dod) => format!("Definition of done: {dod}"),
            None => work_item.description.clone(),
        };

        let input = TaskIntelligenceInput {
            project_id,
            work_item_id: Some(work_item_id),
            task_id: None,
            title: work_item.title.clone(),
            description,
            task_type: None,
            tags: Vec::new(),
            acceptance_criteria: Vec::new(),
            test_commands: Vec::new(),
            dependency_count,
            has_definition_of_done: work_item.definition_of_done.is_some(),
            priority: work_item.priority,
            context_truncated: context.truncation.truncated,
            context_item_count: context.estimated_size.item_count,
            context_decision_count: context.relevant_decisions.len(),
            context_risk_count: context.relevant_risks.len(),
            prior_attempts: outcomes.len() as u32,
            prior_failures,
            prior_tier: outcomes.last().map(|o| o.selected_tier),
            prior_success: outcomes.iter().any(|o| o.success),
        };

        let profile = self
            .intel
            .save_profile(TaskClassifier::classify(&input))
            .await?;
        let decision = self
            .intel
            .save_routing_decision(RoutingPolicy::route(&profile, role, purpose))
            .await?;
        Ok((profile, decision))
    }

    /// Record how a routing decision actually turned out.
    pub async fn record_outcome(
        &self,
        routing_id: Uuid,
        input: OutcomeInput,
    ) -> ArgusResult<RoutingOutcome> {
        let decision = self.routing_decision(routing_id).await?;
        self.intel
            .record_outcome(NewRoutingOutcome {
                routing_id,
                project_id: decision.project_id,
                work_item_id: decision.work_item_id,
                task_id: decision.task_id,
                selected_tier: decision.required_tier,
                success: input.success,
                failure_class: input.failure_class,
                attempts: input.attempts.max(1),
                duration_ms: input.duration_ms,
                artifact_id: input.artifact_id,
            })
            .await
    }

    /// Classify a failure and decide retry/escalate/repair. Persists the
    /// decision so "why did it escalate?" is answerable later.
    pub async fn record_failure(
        &self,
        routing_id: Uuid,
        input: FailureInput,
    ) -> ArgusResult<EscalationDecision> {
        let decision = self.routing_decision(routing_id).await?;
        let failure = input
            .failure_class
            .or_else(|| {
                input
                    .error_type
                    .as_ref()
                    .map(|e| super::escalation::EscalationPolicy::classify_error(Some(e)))
            })
            .unwrap_or(FailureClass::Unknown);

        let attempts = match input.attempts {
            Some(a) => a,
            None => self
                .intel
                .list_outcomes_for_routing(routing_id)
                .await?
                .len() as u32,
        };

        let choice = decision.escalation_policy.decide(
            failure,
            decision.required_tier,
            attempts,
            input.context_truncated,
        );

        self.intel
            .record_escalation(NewEscalationDecision {
                routing_id: Some(routing_id),
                project_id: decision.project_id,
                action: choice.action,
                from_tier: decision.required_tier,
                to_tier: choice.to_tier,
                failure_class: choice.failure_class,
                reason: choice.reason,
                retry_after: None,
                requires_human: choice.requires_human,
            })
            .await
    }

    pub async fn routing_stats(&self, project_id: Uuid) -> ArgusResult<Vec<TaskKindTierStats>> {
        self.intel.routing_stats(project_id).await
    }

    pub async fn get_routing_decision(
        &self,
        routing_id: Uuid,
    ) -> ArgusResult<Option<RoutingDecision>> {
        self.intel.get_routing_decision(routing_id).await
    }

    pub async fn list_profiles(
        &self,
        project_id: Uuid,
        limit: i64,
    ) -> ArgusResult<Vec<TaskProfile>> {
        self.intel.list_profiles(project_id, limit).await
    }

    pub async fn list_routing_decisions(
        &self,
        project_id: Uuid,
        limit: i64,
    ) -> ArgusResult<Vec<RoutingDecision>> {
        self.intel.list_routing_decisions(project_id, limit).await
    }

    pub async fn list_escalations(
        &self,
        project_id: Uuid,
        limit: i64,
    ) -> ArgusResult<Vec<EscalationDecision>> {
        self.intel.list_escalations(project_id, limit).await
    }

    async fn routing_decision(&self, routing_id: Uuid) -> ArgusResult<RoutingDecision> {
        self.intel
            .get_routing_decision(routing_id)
            .await?
            .ok_or_else(|| ArgusError::NotFound(format!("routing decision {routing_id} not found")))
    }
}
