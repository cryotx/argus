//! V0.41 [`ContextCompiler`] — deterministic role/purpose-aware selection.
//!
//! The compiler reads the structured Project State, selects and ranks facts
//! by structural relevance, applies a byte/item budget with deterministic
//! truncation, records provenance for every fact, and returns an immutable,
//! hashed [`ContextPacket`]. No model, no embeddings, no Markdown.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use chrono::Utc;
use uuid::Uuid;

use crate::project_state::{
    ActivityRecord, ArtifactSummary, BlockerRecord, BlockerStatus, CharterRecord, DecisionRecord,
    DecisionStatus, ImprovementRecord, ImprovementStatus, MilestoneRecord, PhaseRecord,
    PostgresProjectStateRepository, ProgressEngine, ProjectRecord, ProjectStateRepository,
    RiskRecord, RiskSeverity, RiskStatus, RoadmapProgress, WorkItemRecord, WorkItemStatus,
};
use crate::types::{ArgusError, ArgusResult};

use super::types::*;

pub struct ContextCompiler {
    repo: Arc<dyn ProjectStateRepository>,
}

impl ContextCompiler {
    pub fn new(repo: Arc<dyn ProjectStateRepository>) -> Self {
        Self { repo }
    }

    /// Convenience constructor over a Postgres pool.
    pub fn from_pool(pool: sqlx::PgPool) -> Self {
        Self::new(Arc::new(PostgresProjectStateRepository::new(pool)))
    }

    pub async fn compile(&self, req: &ContextRequest) -> ArgusResult<ContextPacket> {
        let policy = policy_for(req.role, req.purpose);
        let project = self
            .repo
            .get_project(req.project_id)
            .await?
            .ok_or_else(|| ArgusError::NotFound(format!("project {} not found", req.project_id)))?;

        let charter = self.repo.get_charter(req.project_id).await?;
        let milestones = self.repo.list_milestones(req.project_id).await?;
        let phases = self.repo.list_phases(req.project_id).await?;
        let work_items = self.repo.list_work_items(req.project_id).await?;
        let dependencies = self
            .repo
            .list_work_item_dependencies(req.project_id)
            .await?;
        let decisions = self.repo.list_decisions(req.project_id).await?;
        let risks = self.repo.list_risks(req.project_id).await?;
        let blockers = self.repo.list_blockers(req.project_id).await?;
        let improvements = self.repo.list_improvements(req.project_id).await?;

        let progress = ProgressEngine::build(
            req.project_id,
            &milestones,
            &phases,
            &work_items,
            &dependencies,
            &risks,
            &blockers,
            &improvements,
        );

        let activities = if req.include_recent_activity && policy.activity_limit > 0 {
            self.repo
                .list_activity(req.project_id, policy.activity_limit as i64)
                .await?
        } else {
            Vec::new()
        };

        // --- Focus resolution ---
        let current_work_id = self.resolve_current_work(req, &work_items).await?;
        let current_work = current_work_id
            .and_then(|id| work_items.iter().find(|w| w.work_item_id == id).cloned());

        let deps_by_item = index_dependencies(&dependencies);

        // --- Build sections ---
        let project_identity = ProjectIdentityContext {
            project_id: project.project_id,
            name: project.name.clone(),
            slug: project.slug.clone(),
            description: project.description.clone(),
            status: project.status.to_string(),
            priority: project.priority,
            created_at: project.created_at,
            updated_at: project.updated_at,
        };

        let charter_ctx = if policy.charter {
            charter.as_ref().map(charter_ctx)
        } else {
            None
        };

        let (current_milestone, current_phase) = if policy.milestone_context {
            current_focus(&progress)
        } else {
            (None, None)
        };

        let work_item_ctx = |w: &WorkItemRecord| -> WorkItemContext {
            WorkItemContext {
                work_item_id: w.work_item_id,
                title: w.title.clone(),
                description: w.description.clone(),
                status: w.status.to_string(),
                priority: w.priority,
                definition_of_done: w.definition_of_done.clone(),
                external_key: w.external_key.clone(),
                weight: w.weight,
                milestone_id: w.milestone_id,
                phase_id: w.phase_id,
                depends_on: deps_by_item
                    .get(&w.work_item_id)
                    .cloned()
                    .unwrap_or_default(),
                evidence: Vec::new(),
                completed_at: w.completed_at,
            }
        };

        let mut current_work_item = None;
        let mut evidence_artifact_ids: Vec<Uuid> = Vec::new();
        if policy.current_work_item {
            if let Some(w) = &current_work {
                let mut ctx = work_item_ctx(w);
                let evidence = self.repo.list_work_item_evidence(w.work_item_id).await?;
                for e in &evidence {
                    if let Some(aid) = e.artifact_id {
                        evidence_artifact_ids.push(aid);
                    }
                }
                ctx.evidence = evidence
                    .iter()
                    .take(policy.evidence_limit)
                    .map(|e| WorkItemEvidenceRef {
                        evidence_id: e.evidence_id,
                        evidence_type: e.evidence_type.clone(),
                        artifact_id: e.artifact_id,
                        task_id: e.task_id,
                        execution_id: e.execution_id,
                        note: e.note.clone(),
                    })
                    .collect();
                current_work_item = Some(ctx);
            }
        }

        let mut active_work_items = Vec::new();
        let mut blocked_work_items = Vec::new();
        if policy.work_item_lists {
            let mut active: Vec<&WorkItemRecord> = work_items
                .iter()
                .filter(|w| w.status == WorkItemStatus::Active)
                .collect();
            sort_work_items(&mut active);
            active_work_items = active
                .into_iter()
                .take(policy.work_item_list_limit)
                .map(&work_item_ctx)
                .collect();

            let mut blocked: Vec<&WorkItemRecord> = work_items
                .iter()
                .filter(|w| w.status == WorkItemStatus::Blocked)
                .collect();
            sort_work_items(&mut blocked);
            blocked_work_items = blocked
                .into_iter()
                .take(policy.work_item_list_limit)
                .map(&work_item_ctx)
                .collect();
        }

        let roadmap_context = if policy.roadmap {
            Some(RoadmapContext {
                overall: progress.overall.clone(),
                milestones: progress
                    .milestones
                    .iter()
                    .take(policy.roadmap_limit)
                    .map(milestone_ctx)
                    .collect(),
            })
        } else {
            None
        };

        // Decisions: Accepted only (superseded/rejected never active).
        let relevant_decisions: Vec<DecisionContext> = if policy.decisions {
            let mut ds: Vec<&DecisionRecord> = decisions
                .iter()
                .filter(|d| d.status == DecisionStatus::Accepted)
                .collect();
            ds.sort_by(|a, b| {
                b.created_at
                    .cmp(&a.created_at)
                    .then_with(|| a.decision_id.cmp(&b.decision_id))
            });
            ds.into_iter()
                .take(policy.decision_limit)
                .map(|d| DecisionContext {
                    decision_id: d.decision_id,
                    title: d.title.clone(),
                    decision: d.decision.clone(),
                    reason: d.reason.clone(),
                    status: d.status.to_string(),
                    source: d.source.clone(),
                    created_at: d.created_at,
                })
                .collect()
        } else {
            Vec::new()
        };

        let relevant_risks: Vec<RiskContext> = {
            let mut rs: Vec<&RiskRecord> = risks
                .iter()
                .filter(|r| r.status == RiskStatus::Open)
                .filter(|r| match policy.risk_scope {
                    RiskScope::HighCritical => {
                        matches!(r.severity, RiskSeverity::High | RiskSeverity::Critical)
                    }
                    RiskScope::AllOpen => true,
                })
                .collect();
            rs.sort_by(|a, b| {
                severity_rank(a.severity)
                    .cmp(&severity_rank(b.severity))
                    .then_with(|| a.risk_id.cmp(&b.risk_id))
            });
            rs.into_iter()
                .take(policy.risk_limit)
                .map(|r| RiskContext {
                    risk_id: r.risk_id,
                    title: r.title.clone(),
                    severity: r.severity.as_str().to_string(),
                    likelihood: r.likelihood.as_str().to_string(),
                    status: r.status.as_str().to_string(),
                    mitigation: r.mitigation.clone(),
                    milestone_id: r.milestone_id,
                    created_at: r.created_at,
                })
                .collect()
        };

        let relevant_blockers: Vec<BlockerContext> = {
            let mut bs: Vec<&BlockerRecord> = blockers
                .iter()
                .filter(|b| b.status == BlockerStatus::Open)
                .filter(|b| match policy.blocker_scope {
                    BlockerScope::None => false,
                    BlockerScope::CurrentItem => b.work_item_id == current_work_id,
                    BlockerScope::AllOpen => true,
                })
                .collect();
            bs.sort_by(|a, b| {
                a.created_at
                    .cmp(&b.created_at)
                    .then_with(|| a.blocker_id.cmp(&b.blocker_id))
            });
            bs.into_iter()
                .take(policy.blocker_limit)
                .map(|b| BlockerContext {
                    blocker_id: b.blocker_id,
                    title: b.title.clone(),
                    status: b.status.as_str().to_string(),
                    work_item_id: b.work_item_id,
                    created_at: b.created_at,
                })
                .collect()
        };

        // Artifacts: evidence-linked first, else recent.
        let relevant_artifacts: Vec<ArtifactContext> = if !req.include_artifacts {
            Vec::new()
        } else {
            match policy.artifact_scope {
                ArtifactScope::None => Vec::new(),
                ArtifactScope::Evidence => {
                    let mut out = Vec::new();
                    let mut seen = BTreeSet::new();
                    for aid in &evidence_artifact_ids {
                        if out.len() >= policy.artifact_limit {
                            break;
                        }
                        if !seen.insert(*aid) {
                            continue;
                        }
                        if let Some(a) = self.repo.get_artifact(*aid).await? {
                            out.push(artifact_ctx(&a));
                        }
                    }
                    out
                }
                ArtifactScope::Recent => self
                    .repo
                    .recent_artifacts(req.project_id, policy.artifact_limit as i64)
                    .await?
                    .iter()
                    .map(artifact_ctx)
                    .collect(),
            }
        };

        let recent_activity: Vec<ActivityContext> = if req.include_recent_activity {
            activities
                .iter()
                .take(policy.activity_limit)
                .map(activity_ctx)
                .collect()
        } else {
            Vec::new()
        };

        let dependencies_ctx: Vec<DependencyContext> = if policy.dependencies {
            let mut out: Vec<DependencyContext> = match current_work_id {
                // Focused: what the current WorkItem depends on.
                Some(focus) => deps_by_item
                    .get(&focus)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|dep_id| dependency_ctx(focus, dep_id, &work_items))
                    .collect(),
                // Broad: every roadmap dependency edge in the project.
                None => dependencies
                    .iter()
                    .map(|d| dependency_ctx(d.work_item_id, d.depends_on_id, &work_items))
                    .collect(),
            };
            out.sort_by(|a, b| {
                a.work_item_id
                    .cmp(&b.work_item_id)
                    .then_with(|| a.depends_on_id.cmp(&b.depends_on_id))
            });
            out.truncate(policy.dependency_limit);
            out
        } else {
            Vec::new()
        };

        let deferred_improvements: Vec<ImprovementContext> = if policy.deferred {
            let mut ds: Vec<&ImprovementRecord> = improvements
                .iter()
                .filter(|i| i.status == ImprovementStatus::Deferred)
                .collect();
            ds.sort_by(|a, b| {
                a.external_key
                    .cmp(&b.external_key)
                    .then_with(|| a.improvement_id.cmp(&b.improvement_id))
            });
            ds.into_iter()
                .take(policy.deferred_limit)
                .map(|i| ImprovementContext {
                    improvement_id: i.improvement_id,
                    external_key: i.external_key.clone(),
                    title: i.title.clone(),
                    status: i.status.as_str().to_string(),
                    priority: i.priority.as_str().to_string(),
                    why_deferred: i.why_deferred.clone(),
                    required_before: i.required_before.clone(),
                })
                .collect()
        } else {
            Vec::new()
        };

        let constraints: Vec<ConstraintContext> = if policy.constraints {
            match &charter {
                Some(c) => {
                    let source = ContextSourceRef::new(
                        ContextSourceType::Charter,
                        c.project_id,
                        Some(c.revision),
                        c.updated_at,
                    );
                    c.constraints
                        .iter()
                        .map(|text| ConstraintContext {
                            text: text.clone(),
                            source: source.clone(),
                        })
                        .collect()
                }
                None => Vec::new(),
            }
        } else {
            Vec::new()
        };

        // --- requested_sections hard filter ---
        let mut omitted_sections: Vec<ContextSection> = Vec::new();
        let mut draft = Draft {
            project_identity,
            charter: charter_ctx,
            current_milestone,
            current_phase,
            current_work_item,
            roadmap: roadmap_context,
            active_work_items,
            blocked_work_items,
            decisions: relevant_decisions,
            risks: relevant_risks,
            blockers: relevant_blockers,
            artifacts: relevant_artifacts,
            activity: recent_activity,
            dependencies: dependencies_ctx,
            deferred: deferred_improvements,
            constraints,
            omitted_counts: BTreeMap::new(),
        };
        if let Some(requested) = &req.requested_sections {
            draft.apply_section_filter(requested, &mut omitted_sections);
        }

        // --- Budget + deterministic truncation ---
        let max_items = req
            .max_items
            .or_else(|| req.estimated_token_budget.map(|t| (t / 8).max(8)))
            .unwrap_or(policy.default_max_items);
        let max_bytes = req
            .max_bytes
            .or_else(|| req.estimated_token_budget.map(|t| t * 4))
            .unwrap_or(policy.default_max_bytes);
        let mut omitted_from_budget = draft.truncate_to(max_items, max_bytes);

        let mut all_omitted = omitted_sections;
        all_omitted.append(&mut omitted_from_budget);
        all_omitted.sort();
        all_omitted.dedup();

        // --- Provenance (from what actually remains) ---
        let source_refs = build_source_refs(
            &draft,
            &project,
            charter.as_ref(),
            &milestones,
            &phases,
            &improvements,
            &work_items,
        );

        let item_count = draft.item_count();
        let estimated_bytes = draft.estimate_bytes();

        let mut packet = ContextPacket {
            packet_id: Uuid::new_v4(),
            project_id: req.project_id,
            role: req.role,
            purpose: req.purpose,
            compiler_version: CONTEXT_COMPILER_VERSION.to_string(),
            context_hash: String::new(),
            project_identity: draft.project_identity,
            charter: draft.charter,
            current_milestone: draft.current_milestone,
            current_phase: draft.current_phase,
            current_work_item: draft.current_work_item,
            roadmap_context: draft.roadmap,
            active_work_items: draft.active_work_items,
            blocked_work_items: draft.blocked_work_items,
            relevant_decisions: draft.decisions,
            relevant_risks: draft.risks,
            relevant_blockers: draft.blockers,
            relevant_artifacts: draft.artifacts,
            recent_activity: draft.activity,
            dependencies: draft.dependencies,
            deferred_improvements: draft.deferred,
            constraints: draft.constraints,
            source_refs,
            estimated_size: ContextStats {
                item_count,
                estimated_bytes,
                estimated_tokens: estimated_bytes / 4,
            },
            truncation: TruncationInfo {
                truncated: !all_omitted.is_empty() || !draft.omitted_counts.is_empty(),
                omitted_sections: all_omitted,
                omitted_item_counts: draft.omitted_counts,
            },
            compiled_at: Utc::now(),
        };
        packet.context_hash = packet.compute_context_hash();
        Ok(packet)
    }

    async fn resolve_current_work(
        &self,
        req: &ContextRequest,
        work_items: &[WorkItemRecord],
    ) -> ArgusResult<Option<Uuid>> {
        if let Some(id) = req.work_item_id {
            if work_items.iter().any(|w| w.work_item_id == id) {
                return Ok(Some(id));
            }
            return Ok(None);
        }
        if let Some(task_id) = req.task_id {
            return Ok(self
                .repo
                .get_work_item_by_task_id(task_id)
                .await?
                .map(|w| w.work_item_id));
        }
        // Otherwise: the current active WorkItem (lowest created_at, stable id).
        let mut active: Vec<&WorkItemRecord> = work_items
            .iter()
            .filter(|w| w.status == WorkItemStatus::Active)
            .collect();
        sort_work_items(&mut active);
        Ok(active.first().map(|w| w.work_item_id))
    }
}

// ============================================================
// Role policy
// ============================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RiskScope {
    HighCritical,
    AllOpen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BlockerScope {
    None,
    CurrentItem,
    AllOpen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArtifactScope {
    None,
    Evidence,
    Recent,
}

struct RolePolicy {
    charter: bool,
    roadmap: bool,
    roadmap_limit: usize,
    milestone_context: bool,
    current_work_item: bool,
    work_item_lists: bool,
    work_item_list_limit: usize,
    decisions: bool,
    decision_limit: usize,
    risk_scope: RiskScope,
    risk_limit: usize,
    blocker_scope: BlockerScope,
    blocker_limit: usize,
    artifact_scope: ArtifactScope,
    artifact_limit: usize,
    evidence_limit: usize,
    activity_limit: usize,
    dependencies: bool,
    dependency_limit: usize,
    deferred: bool,
    deferred_limit: usize,
    constraints: bool,
    default_max_items: usize,
    default_max_bytes: usize,
}

fn base_policy(role: ContextRole) -> RolePolicy {
    match role {
        ContextRole::Chief => RolePolicy {
            charter: true,
            roadmap: true,
            roadmap_limit: 12,
            milestone_context: true,
            current_work_item: false,
            work_item_lists: false,
            work_item_list_limit: 0,
            decisions: true,
            decision_limit: 5,
            risk_scope: RiskScope::HighCritical,
            risk_limit: 5,
            blocker_scope: BlockerScope::None,
            blocker_limit: 0,
            artifact_scope: ArtifactScope::None,
            artifact_limit: 0,
            evidence_limit: 0,
            activity_limit: 0,
            dependencies: false,
            dependency_limit: 0,
            deferred: true,
            deferred_limit: 5,
            constraints: true,
            default_max_items: 40,
            default_max_bytes: 16_000,
        },
        ContextRole::ProjectManager => RolePolicy {
            charter: true,
            roadmap: true,
            roadmap_limit: 50,
            milestone_context: true,
            current_work_item: true,
            work_item_lists: true,
            work_item_list_limit: 50,
            decisions: true,
            decision_limit: 10,
            risk_scope: RiskScope::AllOpen,
            risk_limit: 20,
            blocker_scope: BlockerScope::AllOpen,
            blocker_limit: 20,
            artifact_scope: ArtifactScope::Recent,
            artifact_limit: 10,
            evidence_limit: 20,
            activity_limit: 15,
            dependencies: true,
            dependency_limit: 20,
            deferred: true,
            deferred_limit: 20,
            constraints: true,
            default_max_items: 150,
            default_max_bytes: 64_000,
        },
        ContextRole::Developer => RolePolicy {
            charter: false,
            roadmap: false,
            roadmap_limit: 0,
            milestone_context: true,
            current_work_item: true,
            work_item_lists: false,
            work_item_list_limit: 0,
            decisions: true,
            decision_limit: 5,
            risk_scope: RiskScope::HighCritical,
            risk_limit: 5,
            blocker_scope: BlockerScope::CurrentItem,
            blocker_limit: 10,
            artifact_scope: ArtifactScope::Evidence,
            artifact_limit: 10,
            evidence_limit: 20,
            activity_limit: 5,
            dependencies: true,
            dependency_limit: 10,
            deferred: false,
            deferred_limit: 0,
            constraints: true,
            default_max_items: 60,
            default_max_bytes: 32_000,
        },
        ContextRole::Tester => RolePolicy {
            charter: false,
            roadmap: false,
            roadmap_limit: 0,
            milestone_context: false,
            current_work_item: true,
            work_item_lists: false,
            work_item_list_limit: 0,
            decisions: true,
            decision_limit: 5,
            risk_scope: RiskScope::HighCritical,
            risk_limit: 5,
            blocker_scope: BlockerScope::CurrentItem,
            blocker_limit: 5,
            artifact_scope: ArtifactScope::Evidence,
            artifact_limit: 10,
            evidence_limit: 20,
            activity_limit: 5,
            dependencies: true,
            dependency_limit: 10,
            deferred: false,
            deferred_limit: 0,
            constraints: true,
            default_max_items: 60,
            default_max_bytes: 32_000,
        },
        ContextRole::Reviewer => RolePolicy {
            charter: false,
            roadmap: false,
            roadmap_limit: 0,
            milestone_context: true,
            current_work_item: true,
            work_item_lists: false,
            work_item_list_limit: 0,
            decisions: true,
            decision_limit: 8,
            risk_scope: RiskScope::HighCritical,
            risk_limit: 8,
            blocker_scope: BlockerScope::CurrentItem,
            blocker_limit: 5,
            artifact_scope: ArtifactScope::Evidence,
            artifact_limit: 10,
            evidence_limit: 20,
            activity_limit: 5,
            dependencies: true,
            dependency_limit: 10,
            deferred: false,
            deferred_limit: 0,
            constraints: true,
            default_max_items: 80,
            default_max_bytes: 48_000,
        },
        ContextRole::Researcher => RolePolicy {
            charter: true,
            roadmap: true,
            roadmap_limit: 50,
            milestone_context: true,
            current_work_item: false,
            work_item_lists: true,
            work_item_list_limit: 30,
            decisions: true,
            decision_limit: 15,
            risk_scope: RiskScope::AllOpen,
            risk_limit: 15,
            blocker_scope: BlockerScope::AllOpen,
            blocker_limit: 10,
            artifact_scope: ArtifactScope::Recent,
            artifact_limit: 10,
            evidence_limit: 0,
            activity_limit: 20,
            dependencies: true,
            dependency_limit: 20,
            deferred: true,
            deferred_limit: 15,
            constraints: true,
            default_max_items: 150,
            default_max_bytes: 80_000,
        },
        ContextRole::Integrator => RolePolicy {
            charter: false,
            roadmap: true,
            roadmap_limit: 50,
            milestone_context: true,
            current_work_item: false,
            work_item_lists: true,
            work_item_list_limit: 30,
            decisions: true,
            decision_limit: 10,
            risk_scope: RiskScope::AllOpen,
            risk_limit: 15,
            blocker_scope: BlockerScope::AllOpen,
            blocker_limit: 15,
            artifact_scope: ArtifactScope::Recent,
            artifact_limit: 15,
            evidence_limit: 0,
            activity_limit: 10,
            dependencies: true,
            dependency_limit: 20,
            deferred: true,
            deferred_limit: 10,
            constraints: true,
            default_max_items: 120,
            default_max_bytes: 64_000,
        },
    }
}

/// Role policy, then deterministic purpose adjustments. Purpose never changes
/// *what role* sees in a permission sense; it biases the relevant sections.
fn policy_for(role: ContextRole, purpose: ContextPurpose) -> RolePolicy {
    let mut p = base_policy(role);
    match purpose {
        ContextPurpose::Planning | ContextPurpose::StatusReview => {
            p.roadmap = true;
            p.roadmap_limit = p.roadmap_limit.max(12);
            p.current_work_item = p.current_work_item || role == ContextRole::ProjectManager;
        }
        ContextPurpose::Execution => {
            p.current_work_item = true;
            p.dependencies = true;
            p.constraints = true;
            p.activity_limit = p.activity_limit.min(5);
        }
        ContextPurpose::Review => {
            p.current_work_item = true;
            p.artifact_scope = if p.artifact_scope == ArtifactScope::None {
                ArtifactScope::Evidence
            } else {
                p.artifact_scope
            };
        }
        ContextPurpose::Debugging => {
            p.activity_limit = p.activity_limit.max(10);
            if p.blocker_scope == BlockerScope::None {
                p.blocker_scope = BlockerScope::AllOpen;
                p.blocker_limit = 10;
            }
        }
        ContextPurpose::Research => {
            p.roadmap = true;
            p.activity_limit = p.activity_limit.max(20);
        }
        ContextPurpose::Integration => {
            p.dependencies = true;
            p.artifact_scope = ArtifactScope::Recent;
        }
    }
    p
}

// ============================================================
// Draft + truncation
// ============================================================

struct Draft {
    project_identity: ProjectIdentityContext,
    charter: Option<CharterContext>,
    current_milestone: Option<MilestoneContext>,
    current_phase: Option<PhaseContext>,
    current_work_item: Option<WorkItemContext>,
    roadmap: Option<RoadmapContext>,
    active_work_items: Vec<WorkItemContext>,
    blocked_work_items: Vec<WorkItemContext>,
    decisions: Vec<DecisionContext>,
    risks: Vec<RiskContext>,
    blockers: Vec<BlockerContext>,
    artifacts: Vec<ArtifactContext>,
    activity: Vec<ActivityContext>,
    dependencies: Vec<DependencyContext>,
    deferred: Vec<ImprovementContext>,
    constraints: Vec<ConstraintContext>,
    omitted_counts: BTreeMap<String, usize>,
}

impl Draft {
    fn estimate_bytes(&self) -> usize {
        let mut n = 256; // fixed header overhead
        if let Ok(v) = serde_json::to_vec(&self.project_identity) {
            n += v.len();
        }
        n += opt_len(&self.charter);
        n += opt_len(&self.current_milestone);
        n += opt_len(&self.current_phase);
        n += opt_len(&self.current_work_item);
        n += opt_len(&self.roadmap);
        n += vec_len(&self.active_work_items);
        n += vec_len(&self.blocked_work_items);
        n += vec_len(&self.decisions);
        n += vec_len(&self.risks);
        n += vec_len(&self.blockers);
        n += vec_len(&self.artifacts);
        n += vec_len(&self.activity);
        n += vec_len(&self.dependencies);
        n += vec_len(&self.deferred);
        n += vec_len(&self.constraints);
        n
    }

    fn item_count(&self) -> usize {
        let mut n = 1;
        n += self.charter.iter().count();
        n += self.current_milestone.iter().count();
        n += self.current_phase.iter().count();
        n += self.current_work_item.iter().count();
        n += self
            .roadmap
            .as_ref()
            .map(|r| r.milestones.len())
            .unwrap_or(0);
        n += self.active_work_items.len();
        n += self.blocked_work_items.len();
        n += self.decisions.len();
        n += self.risks.len();
        n += self.blockers.len();
        n += self.artifacts.len();
        n += self.activity.len();
        n += self.dependencies.len();
        n += self.deferred.len();
        n += self.constraints.len();
        n
    }

    fn record_omit(&mut self, section: ContextSection) {
        *self
            .omitted_counts
            .entry(section.as_str().to_string())
            .or_insert(0) += 1;
    }

    /// Drop one item from the lowest-priority non-empty section. Returns the
    /// section it dropped from, or `None` if only protected content remains.
    ///
    /// Drop order (lowest priority first) follows the context priority list:
    /// deferred -> activity -> milestone/phase -> artifacts -> dependencies ->
    /// decisions/risks -> blockers -> constraints. Current WorkItem, charter,
    /// and project identity are protected.
    fn drop_lowest_priority(&mut self) -> Option<ContextSection> {
        fn drop_from<T>(
            items: &mut Vec<T>,
            section: ContextSection,
            omitted: &mut BTreeMap<String, usize>,
        ) -> Option<ContextSection> {
            if items.pop().is_some() {
                *omitted.entry(section.as_str().to_string()).or_insert(0) += 1;
                Some(section)
            } else {
                None
            }
        }

        if let Some(s) = drop_from(
            &mut self.deferred,
            ContextSection::DeferredImprovements,
            &mut self.omitted_counts,
        ) {
            return Some(s);
        }
        if let Some(s) = drop_from(
            &mut self.activity,
            ContextSection::RecentActivity,
            &mut self.omitted_counts,
        ) {
            return Some(s);
        }
        if let Some(roadmap) = self.roadmap.as_mut() {
            if let Some(s) = drop_from(
                &mut roadmap.milestones,
                ContextSection::Roadmap,
                &mut self.omitted_counts,
            ) {
                return Some(s);
            }
        }
        if let Some(s) = drop_from(
            &mut self.artifacts,
            ContextSection::Artifacts,
            &mut self.omitted_counts,
        ) {
            return Some(s);
        }
        if let Some(s) = drop_from(
            &mut self.dependencies,
            ContextSection::Dependencies,
            &mut self.omitted_counts,
        ) {
            return Some(s);
        }
        if let Some(s) = drop_from(
            &mut self.decisions,
            ContextSection::Decisions,
            &mut self.omitted_counts,
        ) {
            return Some(s);
        }
        if let Some(s) = drop_from(
            &mut self.risks,
            ContextSection::Risks,
            &mut self.omitted_counts,
        ) {
            return Some(s);
        }
        if let Some(s) = drop_from(
            &mut self.blockers,
            ContextSection::Blockers,
            &mut self.omitted_counts,
        ) {
            return Some(s);
        }
        if let Some(s) = drop_from(
            &mut self.constraints,
            ContextSection::Constraints,
            &mut self.omitted_counts,
        ) {
            return Some(s);
        }
        if let Some(s) = drop_from(
            &mut self.blocked_work_items,
            ContextSection::Blockers,
            &mut self.omitted_counts,
        ) {
            return Some(s);
        }
        if self.current_phase.take().is_some() {
            self.record_omit(ContextSection::CurrentPhase);
            return Some(ContextSection::CurrentPhase);
        }
        if self.current_milestone.take().is_some() {
            self.record_omit(ContextSection::CurrentMilestone);
            return Some(ContextSection::CurrentMilestone);
        }
        // Protected: project identity, charter, current work item, active list.
        None
    }

    fn truncate_to(&mut self, max_items: usize, max_bytes: usize) -> Vec<ContextSection> {
        let mut omitted = Vec::new();
        loop {
            if self.item_count() <= max_items && self.estimate_bytes() <= max_bytes {
                break;
            }
            match self.drop_lowest_priority() {
                Some(section) => omitted.push(section),
                None => break,
            }
        }
        omitted.sort();
        omitted.dedup();
        omitted
    }

    fn apply_section_filter(
        &mut self,
        requested: &[ContextSection],
        omitted: &mut Vec<ContextSection>,
    ) {
        let keep = |s: ContextSection| requested.contains(&s);
        if !keep(ContextSection::Charter) && self.charter.take().is_some() {
            omitted.push(ContextSection::Charter);
        }
        if !keep(ContextSection::CurrentMilestone) {
            self.current_milestone = None;
        }
        if !keep(ContextSection::CurrentPhase) {
            self.current_phase = None;
        }
        if !keep(ContextSection::CurrentWorkItem) {
            self.current_work_item = None;
        }
        if !keep(ContextSection::Roadmap) {
            self.roadmap = None;
        }
        if !keep(ContextSection::Decisions) {
            self.decisions.clear();
        }
        if !keep(ContextSection::Risks) {
            self.risks.clear();
        }
        if !keep(ContextSection::Blockers) {
            self.blockers.clear();
            self.blocked_work_items.clear();
        }
        if !keep(ContextSection::Artifacts) {
            self.artifacts.clear();
        }
        if !keep(ContextSection::RecentActivity) {
            self.activity.clear();
        }
        if !keep(ContextSection::Dependencies) {
            self.dependencies.clear();
        }
        if !keep(ContextSection::DeferredImprovements) {
            self.deferred.clear();
        }
        if !keep(ContextSection::Constraints) {
            self.constraints.clear();
        }
        // Project identity is always present: it is the packet's subject.
    }
}

// ============================================================
// Helpers
// ============================================================

fn index_dependencies(
    deps: &[crate::project_state::WorkItemDependencyRecord],
) -> HashMap<Uuid, Vec<Uuid>> {
    let mut map: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for d in deps {
        map.entry(d.work_item_id).or_default().push(d.depends_on_id);
    }
    for v in map.values_mut() {
        v.sort();
        v.dedup();
    }
    map
}

fn sort_work_items(items: &mut Vec<&WorkItemRecord>) {
    items.sort_by(|a, b| {
        b.priority
            .value()
            .cmp(&a.priority.value())
            .then_with(|| a.created_at.cmp(&b.created_at))
            .then_with(|| a.work_item_id.cmp(&b.work_item_id))
    });
}

fn dependency_ctx(
    work_item_id: Uuid,
    depends_on_id: Uuid,
    work_items: &[WorkItemRecord],
) -> DependencyContext {
    let target = work_items.iter().find(|w| w.work_item_id == depends_on_id);
    DependencyContext {
        work_item_id,
        depends_on_id,
        depends_on_title: target.map(|w| w.title.clone()),
        depends_on_status: target.map(|w| w.status.to_string()),
        satisfied: target.is_some_and(|w| w.status.is_complete()),
    }
}

fn current_focus(progress: &RoadmapProgress) -> (Option<MilestoneContext>, Option<PhaseContext>) {
    let current_id = progress.current_milestone.as_ref().map(|m| m.id);
    let milestone = current_id.and_then(|id| {
        progress
            .milestones
            .iter()
            .find(|m| m.milestone_id == id)
            .map(milestone_ctx)
    });
    let phase = progress.current_phase.as_ref().and_then(|p| {
        progress
            .milestones
            .iter()
            .flat_map(|m| m.phases.iter())
            .find(|ph| ph.phase_id == p.id)
            .map(|ph| PhaseContext {
                phase_id: ph.phase_id,
                title: ph.title.clone(),
                status: ph.status.as_str().to_string(),
                sequence: ph.sequence,
                external_key: ph.external_key.clone(),
                completion_ratio: ph.summary.completion_ratio,
            })
    });
    (milestone, phase)
}

fn milestone_ctx(m: &crate::project_state::MilestoneProgress) -> MilestoneContext {
    MilestoneContext {
        milestone_id: m.milestone_id,
        title: m.title.clone(),
        status: m.status.as_str().to_string(),
        sequence: m.sequence,
        external_key: m.external_key.clone(),
        completion_ratio: m.summary.completion_ratio,
        verified_items: m.summary.verified_items + m.summary.accepted_items,
        total_items: m.summary.total_items,
    }
}

fn charter_ctx(c: &CharterRecord) -> CharterContext {
    CharterContext {
        vision: c.vision.clone(),
        goal: c.goal.clone(),
        success_criteria: c.success_criteria.clone(),
        constraints: c.constraints.clone(),
        non_goals: c.non_goals.clone(),
        revision: c.revision,
        updated_at: c.updated_at,
    }
}

fn artifact_ctx(a: &ArtifactSummary) -> ArtifactContext {
    ArtifactContext {
        artifact_id: a.artifact_id,
        task_id: a.task_id,
        workspace_id: a.workspace_id,
        commit_sha: a.commit_sha.clone(),
        branch_name: a.branch_name.clone(),
        files_changed: a.files_changed.clone(),
        quality_status: a.quality_status.clone(),
        created_at: a.created_at,
    }
}

fn activity_ctx(a: &ActivityRecord) -> ActivityContext {
    ActivityContext {
        activity_id: a.activity_id,
        activity_type: a.activity_type.as_str().to_string(),
        entity_type: a.entity_type.clone(),
        entity_id: a.entity_id,
        summary: a.summary.clone(),
        occurred_at: a.occurred_at,
    }
}

fn severity_rank(s: RiskSeverity) -> i32 {
    match s {
        RiskSeverity::Critical => 0,
        RiskSeverity::High => 1,
        RiskSeverity::Medium => 2,
        RiskSeverity::Low => 3,
    }
}

fn opt_len<T: serde::Serialize>(v: &Option<T>) -> usize {
    v.as_ref()
        .and_then(|x| serde_json::to_vec(x).ok())
        .map(|b| b.len())
        .unwrap_or(0)
}

fn vec_len<T: serde::Serialize>(v: &[T]) -> usize {
    v.iter()
        .map(|x| serde_json::to_vec(x).map(|b| b.len()).unwrap_or(0))
        .sum()
}

#[allow(clippy::too_many_arguments)]
fn build_source_refs(
    draft: &Draft,
    project: &ProjectRecord,
    charter: Option<&CharterRecord>,
    milestones: &[MilestoneRecord],
    phases: &[PhaseRecord],
    improvements: &[ImprovementRecord],
    work_items: &[WorkItemRecord],
) -> Vec<ContextSourceRef> {
    let mut refs: BTreeMap<(ContextSourceType, Uuid), ContextSourceRef> = BTreeMap::new();
    let milestone_by_id: HashMap<Uuid, &MilestoneRecord> =
        milestones.iter().map(|m| (m.milestone_id, m)).collect();
    let phase_by_id: HashMap<Uuid, &PhaseRecord> = phases.iter().map(|p| (p.phase_id, p)).collect();
    let improvement_by_id: HashMap<Uuid, &ImprovementRecord> =
        improvements.iter().map(|i| (i.improvement_id, i)).collect();
    let work_item_by_id: HashMap<Uuid, &WorkItemRecord> =
        work_items.iter().map(|w| (w.work_item_id, w)).collect();

    refs.insert(
        (ContextSourceType::Project, project.project_id),
        ContextSourceRef::new(
            ContextSourceType::Project,
            project.project_id,
            None,
            project.updated_at,
        ),
    );
    if let Some(c) = charter {
        refs.insert(
            (ContextSourceType::Charter, c.project_id),
            ContextSourceRef::new(
                ContextSourceType::Charter,
                c.project_id,
                Some(c.revision),
                c.updated_at,
            ),
        );
    }
    if let Some(m) = &draft.current_milestone {
        if let Some(rec) = milestone_by_id.get(&m.milestone_id) {
            refs.insert(
                (ContextSourceType::Milestone, m.milestone_id),
                ContextSourceRef::new(
                    ContextSourceType::Milestone,
                    m.milestone_id,
                    None,
                    rec.updated_at,
                ),
            );
        }
    }
    if let Some(r) = &draft.roadmap {
        for m in &r.milestones {
            if let Some(rec) = milestone_by_id.get(&m.milestone_id) {
                refs.insert(
                    (ContextSourceType::Milestone, m.milestone_id),
                    ContextSourceRef::new(
                        ContextSourceType::Milestone,
                        m.milestone_id,
                        None,
                        rec.updated_at,
                    ),
                );
            }
        }
    }
    if let Some(p) = &draft.current_phase {
        if let Some(rec) = phase_by_id.get(&p.phase_id) {
            refs.insert(
                (ContextSourceType::Phase, p.phase_id),
                ContextSourceRef::new(ContextSourceType::Phase, p.phase_id, None, rec.updated_at),
            );
        }
    }
    let add_work_item = |refs: &mut BTreeMap<(ContextSourceType, Uuid), ContextSourceRef>,
                         id: Uuid| {
        if let Some(rec) = work_item_by_id.get(&id) {
            refs.insert(
                (ContextSourceType::WorkItem, id),
                ContextSourceRef::new(ContextSourceType::WorkItem, id, None, rec.updated_at),
            );
        }
    };
    if let Some(w) = &draft.current_work_item {
        add_work_item(&mut refs, w.work_item_id);
        for dep in &w.depends_on {
            add_work_item(&mut refs, *dep);
        }
    }
    for w in draft
        .active_work_items
        .iter()
        .chain(draft.blocked_work_items.iter())
    {
        add_work_item(&mut refs, w.work_item_id);
    }
    for d in &draft.decisions {
        refs.insert(
            (ContextSourceType::Decision, d.decision_id),
            ContextSourceRef::new(
                ContextSourceType::Decision,
                d.decision_id,
                None,
                d.created_at,
            ),
        );
    }
    for r in &draft.risks {
        refs.insert(
            (ContextSourceType::Risk, r.risk_id),
            ContextSourceRef::new(ContextSourceType::Risk, r.risk_id, None, r.created_at),
        );
    }
    for b in &draft.blockers {
        refs.insert(
            (ContextSourceType::Blocker, b.blocker_id),
            ContextSourceRef::new(ContextSourceType::Blocker, b.blocker_id, None, b.created_at),
        );
    }
    for a in &draft.artifacts {
        refs.insert(
            (ContextSourceType::Artifact, a.artifact_id),
            ContextSourceRef::new(
                ContextSourceType::Artifact,
                a.artifact_id,
                None,
                a.created_at,
            ),
        );
    }
    for a in &draft.activity {
        refs.insert(
            (ContextSourceType::Activity, a.activity_id),
            ContextSourceRef::new(
                ContextSourceType::Activity,
                a.activity_id,
                None,
                a.occurred_at,
            ),
        );
    }
    for i in &draft.deferred {
        let created_at = improvement_by_id
            .get(&i.improvement_id)
            .map(|rec| rec.created_at)
            .unwrap_or(project.updated_at);
        refs.insert(
            (ContextSourceType::Improvement, i.improvement_id),
            ContextSourceRef::new(
                ContextSourceType::Improvement,
                i.improvement_id,
                None,
                created_at,
            ),
        );
    }
    let mut out: Vec<ContextSourceRef> = refs.into_values().collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn developer_policy_is_narrower_than_project_manager() {
        let dev = policy_for(ContextRole::Developer, ContextPurpose::Execution);
        assert!(!dev.roadmap, "Developer must not get the whole roadmap");
        assert!(!dev.deferred, "Developer must not get the deferred backlog");
        assert_eq!(dev.artifact_scope, ArtifactScope::Evidence);
        assert_eq!(dev.blocker_scope, BlockerScope::CurrentItem);

        let pm = policy_for(ContextRole::ProjectManager, ContextPurpose::Planning);
        assert!(pm.roadmap);
        assert!(pm.deferred);
        assert!(pm.work_item_lists);
        assert_eq!(pm.risk_scope, RiskScope::AllOpen);
    }

    #[test]
    fn chief_only_gets_high_level() {
        let chief = policy_for(ContextRole::Chief, ContextPurpose::StatusReview);
        assert!(chief.roadmap);
        assert!(!chief.current_work_item);
        assert_eq!(chief.risk_scope, RiskScope::HighCritical);
        assert_eq!(chief.artifact_scope, ArtifactScope::None);
        assert_eq!(chief.activity_limit, 0);
    }

    #[test]
    fn purpose_changes_selection() {
        let exec = policy_for(ContextRole::Developer, ContextPurpose::Execution);
        let debug = policy_for(ContextRole::Developer, ContextPurpose::Debugging);
        assert!(debug.activity_limit > exec.activity_limit);
        // A role with no blocker scope gains one when debugging.
        let chief_debug = policy_for(ContextRole::Chief, ContextPurpose::Debugging);
        assert_eq!(chief_debug.blocker_scope, BlockerScope::AllOpen);
    }
}
