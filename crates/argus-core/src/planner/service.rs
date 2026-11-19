//! V0.43 PlannerHarness — the single entry point shared by CLI and API.
//!
//! ```text
//! create request -> (adapter | manual import) -> validate -> diff
//!   -> approve/reject -> apply (explicit, transactional, idempotent)
//! ```
//!
//! The harness never applies a proposal on its own and never calls a model.

use std::sync::Arc;

use chrono::Utc;
use uuid::Uuid;

use crate::context::{
    ContextCompiler, ContextPurpose, ContextRequest, ContextRole, render_markdown,
};
use crate::intelligence::TaskIntelligenceService;
use crate::project_state::ProjectStateRepository;
use crate::types::{ArgusError, ArgusResult};

use super::adapter::PlannerAdapter;
use super::diff::{DiffLabels, build_plan_diff};
use super::protocol::{compute_request_hash, compute_result_hash, protocol_version_supported};
use super::repository::PlanningRepository;
use super::types::*;
use super::validator;

/// Result of a plan/import run: the result, the persisted proposal, its diff,
/// and any validation errors (a rejected proposal still has a record).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PlanningRunOutcome {
    pub result: PlanningResult,
    pub proposal: ProposalRecord,
    pub diff: PlanDiff,
    pub validation_errors: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct CreatePlanningRequestInput {
    pub project_id: Uuid,
    pub planning_kind: PlanningKind,
    pub role: ContextRole,
    pub purpose: ContextPurpose,
    pub objective: String,
    pub constraints: Vec<String>,
    pub requested_outputs: Vec<RequestedOutput>,
    pub work_item_id: Option<Uuid>,
}

pub struct PlannerHarness {
    state: Arc<dyn ProjectStateRepository>,
    repo: Arc<dyn PlanningRepository>,
    intelligence: Option<Arc<TaskIntelligenceService>>,
}

impl PlannerHarness {
    pub fn new(
        state: Arc<dyn ProjectStateRepository>,
        repo: Arc<dyn PlanningRepository>,
        intelligence: Option<Arc<TaskIntelligenceService>>,
    ) -> Self {
        Self {
            state,
            repo,
            intelligence,
        }
    }

    // ============================================================
    // Requests
    // ============================================================

    pub async fn create_planning_request(
        &self,
        input: CreatePlanningRequestInput,
    ) -> ArgusResult<PlanningRequest> {
        self.state
            .get_project(input.project_id)
            .await?
            .ok_or_else(|| {
                ArgusError::NotFound(format!("project {} not found", input.project_id))
            })?;

        // Bound context via the V0.41 Context Compiler (never re-implemented).
        let mut context_request = ContextRequest::new(input.project_id, input.role, input.purpose);
        if let Some(work_item_id) = input.work_item_id {
            context_request = context_request.with_work_item(work_item_id);
        }
        let packet = ContextCompiler::new(self.state.clone())
            .compile(&context_request)
            .await?;

        // Optional Task Intelligence binding.
        let (task_profile_ref, routing_decision_ref) =
            match (&self.intelligence, input.work_item_id) {
                (Some(intel), Some(work_item_id)) => {
                    let (profile, decision) = intel
                        .classify_work_item(
                            input.project_id,
                            work_item_id,
                            input.role,
                            input.purpose,
                        )
                        .await?;
                    (Some(profile.profile_id), Some(decision.routing_id))
                }
                _ => (None, None),
            };

        let revisions = self.repo.get_revisions(input.project_id).await?;

        let mut request = PlanningRequest {
            planning_request_id: Uuid::new_v4(),
            project_id: input.project_id,
            planning_kind: input.planning_kind,
            role: input.role.as_str().to_string(),
            purpose: input.purpose.as_str().to_string(),
            context_packet_ref: Some(packet.packet_id),
            context_hash: packet.context_hash.clone(),
            compiler_version: packet.compiler_version.clone(),
            work_item_id: input.work_item_id,
            task_profile_ref,
            routing_decision_ref,
            state_revision: revisions.state_revision,
            roadmap_revision: revisions.roadmap_revision,
            objective: input.objective,
            constraints: input.constraints,
            requested_outputs: input.requested_outputs,
            protocol_version: PROTOCOL_VERSION.to_string(),
            request_hash: String::new(),
            created_at: Utc::now(),
        };
        request.request_hash = compute_request_hash(&request);
        self.repo.save_request(request).await
    }

    pub async fn get_request(&self, id: Uuid) -> ArgusResult<PlanningRequest> {
        self.repo
            .get_request(id)
            .await?
            .ok_or_else(|| ArgusError::NotFound(format!("planning request {id} not found")))
    }

    // ============================================================
    // Plan / import
    // ============================================================

    /// Run an adapter against a stored request.
    pub async fn run_adapter(
        &self,
        adapter: &dyn PlannerAdapter,
        request_id: Uuid,
    ) -> ArgusResult<PlanningRunOutcome> {
        let request = self.get_request(request_id).await?;
        let result = adapter.plan(&request).await?;
        self.persist_result_and_proposal(&request, result).await
    }

    /// Import an untrusted `PlanningResult` (e.g. from a web planner).
    pub async fn import_result(&self, result: PlanningResult) -> ArgusResult<PlanningRunOutcome> {
        let request = self
            .repo
            .get_request(result.planning_request_id)
            .await?
            .ok_or_else(|| {
                ArgusError::NotFound(format!(
                    "planning request {} not found",
                    result.planning_request_id
                ))
            })?;
        self.persist_result_and_proposal(&request, result).await
    }

    async fn persist_result_and_proposal(
        &self,
        request: &PlanningRequest,
        mut result: PlanningResult,
    ) -> ArgusResult<PlanningRunOutcome> {
        // Bind the result to the request; detect tampering.
        if !protocol_version_supported(&result.protocol_version) {
            return Err(ArgusError::Validation(format!(
                "unsupported result protocol_version '{}'",
                result.protocol_version
            )));
        }
        if result.request_hash != request.request_hash {
            return Err(ArgusError::Validation(
                "result.request_hash does not match the bound request".to_string(),
            ));
        }
        if result.context_hash != request.context_hash {
            return Err(ArgusError::Validation(
                "result.context_hash does not match the bound context".to_string(),
            ));
        }
        result.planning_request_id = request.planning_request_id;
        result.protocol_version = PROTOCOL_VERSION.to_string();
        let expected_hash = compute_result_hash(&result);
        if !result.result_hash.is_empty() && result.result_hash != expected_hash {
            return Err(ArgusError::Validation(
                "result_hash mismatch (result was modified after hashing)".to_string(),
            ));
        }
        result.result_hash = expected_hash.clone();

        // Duplicate import: reuse the existing proposal.
        if let Some(existing) = self
            .repo
            .get_proposal_by_result_hash(request.project_id, &expected_hash)
            .await?
        {
            let diff = self.diff_for_proposal(&existing).await?;
            return Ok(PlanningRunOutcome {
                result,
                validation_errors: existing.validation_errors.clone(),
                proposal: existing,
                diff,
            });
        }

        let errors = validator::validate(self.state.as_ref(), request, &result).await?;
        let persisted = self
            .repo
            .save_result(result.clone(), request.project_id)
            .await?;

        let status = if errors.is_empty() {
            ProposalStatus::Validated
        } else {
            ProposalStatus::Rejected
        };
        let proposal = ProposalRecord {
            proposal_id: Uuid::new_v4(),
            planning_result_id: persisted.result.planning_result_id,
            planning_request_id: request.planning_request_id,
            project_id: request.project_id,
            status,
            proposal: result.proposal.clone(),
            validation_errors: errors.clone(),
            context_hash: request.context_hash.clone(),
            request_hash: request.request_hash.clone(),
            result_hash: expected_hash,
            state_revision: request.state_revision,
            roadmap_revision: request.roadmap_revision,
            applied_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let proposal = self.repo.save_proposal(proposal).await?;
        self.repo
            .record_lifecycle(
                proposal.proposal_id,
                request.project_id,
                status.as_str(),
                Some(if errors.is_empty() {
                    "validation passed"
                } else {
                    "validation failed"
                }),
            )
            .await?;
        let diff = self.diff_for_proposal(&proposal).await?;

        Ok(PlanningRunOutcome {
            result,
            proposal,
            diff,
            validation_errors: errors,
        })
    }

    // ============================================================
    // Proposals
    // ============================================================

    pub async fn get_proposal(&self, id: Uuid) -> ArgusResult<ProposalRecord> {
        self.repo
            .get_proposal(id)
            .await?
            .ok_or_else(|| ArgusError::NotFound(format!("proposal {id} not found")))
    }

    pub async fn list_proposals(
        &self,
        project_id: Uuid,
        limit: i64,
    ) -> ArgusResult<Vec<ProposalRecord>> {
        self.repo.list_proposals(project_id, limit).await
    }

    pub async fn list_lifecycle(&self, proposal_id: Uuid) -> ArgusResult<Vec<LifecycleEvent>> {
        self.repo.list_lifecycle(proposal_id).await
    }

    pub async fn get_diff(&self, proposal_id: Uuid) -> ArgusResult<PlanDiff> {
        let proposal = self.get_proposal(proposal_id).await?;
        self.diff_for_proposal(&proposal).await
    }

    pub async fn approve_proposal(&self, proposal_id: Uuid) -> ArgusResult<ProposalRecord> {
        let proposal = self.get_proposal(proposal_id).await?;
        match proposal.status {
            ProposalStatus::Applied => {
                return Err(ArgusError::Validation("proposal is already applied".into()));
            }
            ProposalStatus::Rejected => {
                return Err(ArgusError::Validation("proposal was rejected".into()));
            }
            ProposalStatus::Stale => {
                return Err(ArgusError::Validation("proposal is stale".into()));
            }
            _ => {}
        }
        // Stale check at approval time too.
        let current = self.repo.get_revisions(proposal.project_id).await?;
        if current.state_revision != proposal.state_revision
            || current.roadmap_revision != proposal.roadmap_revision
        {
            self.repo
                .set_proposal_status(
                    proposal_id,
                    ProposalStatus::Stale,
                    proposal.validation_errors.clone(),
                )
                .await?;
            self.repo
                .record_lifecycle(
                    proposal_id,
                    proposal.project_id,
                    "stale",
                    Some("revisions changed"),
                )
                .await?;
            return Err(ArgusError::Validation("proposal is stale".into()));
        }
        self.repo
            .set_proposal_status(
                proposal_id,
                ProposalStatus::Approved,
                proposal.validation_errors.clone(),
            )
            .await?;
        self.repo
            .record_lifecycle(proposal_id, proposal.project_id, "approved", None)
            .await?;
        self.get_proposal(proposal_id).await
    }

    pub async fn reject_proposal(&self, proposal_id: Uuid) -> ArgusResult<ProposalRecord> {
        let proposal = self.get_proposal(proposal_id).await?;
        if proposal.status == ProposalStatus::Applied {
            return Err(ArgusError::Validation(
                "cannot reject an applied proposal".into(),
            ));
        }
        self.repo
            .set_proposal_status(
                proposal_id,
                ProposalStatus::Rejected,
                proposal.validation_errors.clone(),
            )
            .await?;
        self.repo
            .record_lifecycle(proposal_id, proposal.project_id, "rejected", None)
            .await?;
        self.get_proposal(proposal_id).await
    }

    /// Explicit, transactional, idempotent apply. Requires an approved
    /// proposal.
    pub async fn apply_proposal(&self, proposal_id: Uuid) -> ArgusResult<ApplyOutcome> {
        match self.repo.apply_proposal(proposal_id).await {
            Ok(outcome) => Ok(outcome),
            Err(err) => {
                if let Ok(proposal) = self.get_proposal(proposal_id).await {
                    if proposal.status == ProposalStatus::Stale {
                        let _ = self
                            .repo
                            .record_lifecycle(
                                proposal_id,
                                proposal.project_id,
                                "stale",
                                Some(&err.to_string()),
                            )
                            .await;
                    } else {
                        let _ = self
                            .repo
                            .record_lifecycle(
                                proposal_id,
                                proposal.project_id,
                                "failed",
                                Some(&err.to_string()),
                            )
                            .await;
                    }
                }
                Err(err)
            }
        }
    }

    // ============================================================
    // Manual export
    // ============================================================

    pub async fn export_packet(&self, request_id: Uuid) -> ArgusResult<String> {
        let request = self.get_request(request_id).await?;
        let role = ContextRole::parse(&request.role)
            .ok_or_else(|| ArgusError::Validation(format!("unknown role '{}'", request.role)))?;
        let purpose = ContextPurpose::parse(&request.purpose).ok_or_else(|| {
            ArgusError::Validation(format!("unknown purpose '{}'", request.purpose))
        })?;
        let mut context_request = ContextRequest::new(request.project_id, role, purpose);
        if let Some(work_item_id) = request.work_item_id {
            context_request = context_request.with_work_item(work_item_id);
        }
        let packet = ContextCompiler::new(self.state.clone())
            .compile(&context_request)
            .await?;
        let context_md = render_markdown(&packet);

        let routing_line = match (&self.intelligence, request.routing_decision_ref) {
            (Some(intel), Some(routing_id)) => intel
                .get_routing_decision(routing_id)
                .await?
                .map(|d| format!("{} / reasoning {}", d.required_tier, d.reasoning_effort)),
            _ => None,
        };

        Ok(super::export::render_planner_packet(
            &request,
            &context_md,
            routing_line.as_deref(),
        ))
    }

    // ============================================================
    // Helpers
    // ============================================================

    async fn diff_for_proposal(&self, proposal: &ProposalRecord) -> ArgusResult<PlanDiff> {
        let entities = validator::load_entities(self.state.as_ref(), proposal.project_id).await?;
        let labels = DiffLabels {
            milestones: entities
                .milestones
                .iter()
                .map(|m| (m.milestone_id, m.title.clone()))
                .collect(),
            phases: entities
                .phases
                .iter()
                .map(|p| (p.phase_id, p.title.clone()))
                .collect(),
            work_items: entities
                .work_items
                .iter()
                .map(|w| (w.work_item_id, w.title.clone()))
                .collect(),
        };
        Ok(build_plan_diff(&proposal.proposal, &labels))
    }
}
