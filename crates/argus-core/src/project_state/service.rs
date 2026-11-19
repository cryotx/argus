//! V0.39 ProjectStateService — the single upper layer API handlers, CLI, and
//! any future planner call.
//!
//! It composes the structured tables into a deterministic [`ProjectSnapshot`]
//! (and a deterministic human-readable rendering of it). It never asks a
//! model to describe project state, and it never reads Markdown: the
//! database is the source of truth.

use std::sync::Arc;

use serde_json::json;
use uuid::Uuid;

use crate::task_types::Priority;
use crate::types::{ArgusError, ArgusResult};

use super::repository::ProjectStateRepository;
use super::types::*;

pub struct ProjectStateService {
    repo: Arc<dyn ProjectStateRepository>,
}

impl ProjectStateService {
    pub fn new(repo: Arc<dyn ProjectStateRepository>) -> Self {
        Self { repo }
    }

    pub fn repository(&self) -> &Arc<dyn ProjectStateRepository> {
        &self.repo
    }

    // ============================================================
    // Project
    // ============================================================

    /// Ensure a project exists (idempotent by slug). When this actually
    /// creates the project, one `ProjectCreated` activity is recorded.
    pub async fn create_project_state(&self, project: NewProject) -> ArgusResult<ProjectRecord> {
        let slug = project.slug.clone();
        let existed = self.repo.get_project_by_slug(&slug).await?.is_some();
        let record = self.repo.ensure_project(project).await?;
        if !existed {
            self.repo
                .record_activity(NewActivity {
                    project_id: record.project_id,
                    activity_type: ActivityType::ProjectCreated,
                    entity_type: "project".to_string(),
                    entity_id: Some(record.project_id),
                    summary: format!("Project '{}' created", record.name),
                    metadata: json!({ "slug": record.slug }),
                })
                .await?;
        }
        Ok(record)
    }

    pub async fn get_project(&self, project_id: Uuid) -> ArgusResult<ProjectRecord> {
        self.repo
            .get_project(project_id)
            .await?
            .ok_or_else(|| ArgusError::NotFound(format!("project {project_id} not found")))
    }

    pub async fn get_project_by_slug(&self, slug: &str) -> ArgusResult<ProjectRecord> {
        self.repo
            .get_project_by_slug(slug)
            .await?
            .ok_or_else(|| ArgusError::NotFound(format!("project '{slug}' not found")))
    }

    pub async fn list_projects(&self) -> ArgusResult<Vec<ProjectRecord>> {
        self.repo.list_projects().await
    }

    pub async fn set_project_status(
        &self,
        project_id: Uuid,
        status: ProjectStatus,
    ) -> ArgusResult<()> {
        self.repo.set_project_status(project_id, status).await?;
        let activity_type = match status {
            ProjectStatus::Paused => Some(ActivityType::ProjectPaused),
            ProjectStatus::Active => Some(ActivityType::ProjectResumed),
            _ => None,
        };
        if let Some(activity_type) = activity_type {
            self.repo
                .record_activity(NewActivity {
                    project_id,
                    activity_type,
                    entity_type: "project".to_string(),
                    entity_id: Some(project_id),
                    summary: format!("Project status set to {status}"),
                    metadata: json!({ "status": status.as_str() }),
                })
                .await?;
        }
        Ok(())
    }

    // ============================================================
    // Charter
    // ============================================================

    pub async fn save_charter(&self, charter: NewCharter) -> ArgusResult<CharterRecord> {
        self.repo.save_charter(charter).await
    }

    pub async fn get_charter(&self, project_id: Uuid) -> ArgusResult<Option<CharterRecord>> {
        self.repo.get_charter(project_id).await
    }

    // ============================================================
    // Milestone
    // ============================================================

    pub async fn create_milestone(&self, milestone: NewMilestone) -> ArgusResult<MilestoneRecord> {
        let record = self.repo.create_milestone(milestone).await?;
        self.repo
            .record_activity(NewActivity {
                project_id: record.project_id,
                activity_type: ActivityType::MilestoneCreated,
                entity_type: "milestone".to_string(),
                entity_id: Some(record.milestone_id),
                summary: format!("Milestone '{}' created", record.title),
                metadata: json!({ "sequence": record.sequence }),
            })
            .await?;
        Ok(record)
    }

    pub async fn get_milestone(&self, milestone_id: Uuid) -> ArgusResult<Option<MilestoneRecord>> {
        self.repo.get_milestone(milestone_id).await
    }

    pub async fn list_milestones(&self, project_id: Uuid) -> ArgusResult<Vec<MilestoneRecord>> {
        self.repo.list_milestones(project_id).await
    }

    pub async fn set_milestone_status(
        &self,
        milestone_id: Uuid,
        status: MilestoneStatus,
    ) -> ArgusResult<()> {
        self.repo.set_milestone_status(milestone_id, status).await?;
        if let Some(milestone) = self.repo.get_milestone(milestone_id).await? {
            let activity_type = match status {
                MilestoneStatus::Active => Some(ActivityType::MilestoneStarted),
                MilestoneStatus::Completed => Some(ActivityType::MilestoneCompleted),
                _ => None,
            };
            if let Some(activity_type) = activity_type {
                self.repo
                    .record_activity(NewActivity {
                        project_id: milestone.project_id,
                        activity_type,
                        entity_type: "milestone".to_string(),
                        entity_id: Some(milestone_id),
                        summary: format!("Milestone '{}' {}", milestone.title, status),
                        metadata: json!({ "title": milestone.title }),
                    })
                    .await?;
            }
        }
        Ok(())
    }

    // ============================================================
    // Phase (V0.40)
    // ============================================================

    pub async fn create_phase(&self, phase: NewPhase) -> ArgusResult<PhaseRecord> {
        let record = self.repo.create_phase(phase).await?;
        self.repo
            .record_activity(NewActivity {
                project_id: record.project_id,
                activity_type: ActivityType::PhaseCreated,
                entity_type: "phase".to_string(),
                entity_id: Some(record.phase_id),
                summary: format!("Phase '{}' created", record.title),
                metadata: json!({ "sequence": record.sequence }),
            })
            .await?;
        Ok(record)
    }

    pub async fn get_phase(&self, phase_id: Uuid) -> ArgusResult<Option<PhaseRecord>> {
        self.repo.get_phase(phase_id).await
    }

    pub async fn list_phases(&self, project_id: Uuid) -> ArgusResult<Vec<PhaseRecord>> {
        self.repo.list_phases(project_id).await
    }

    pub async fn set_phase_status(
        &self,
        phase_id: Uuid,
        status: MilestoneStatus,
    ) -> ArgusResult<()> {
        self.repo.set_phase_status(phase_id, status).await?;
        if let Some(phase) = self.repo.get_phase(phase_id).await? {
            let activity_type = match status {
                MilestoneStatus::Active => Some(ActivityType::PhaseStarted),
                MilestoneStatus::Completed => Some(ActivityType::PhaseCompleted),
                _ => None,
            };
            if let Some(activity_type) = activity_type {
                self.repo
                    .record_activity(NewActivity {
                        project_id: phase.project_id,
                        activity_type,
                        entity_type: "phase".to_string(),
                        entity_id: Some(phase_id),
                        summary: format!("Phase '{}' {}", phase.title, status),
                        metadata: json!({ "title": phase.title }),
                    })
                    .await?;
            }
        }
        Ok(())
    }

    // ============================================================
    // WorkItem
    // ============================================================

    pub async fn create_work_item(&self, work_item: NewWorkItem) -> ArgusResult<WorkItemRecord> {
        let record = self.repo.create_work_item(work_item).await?;
        self.repo
            .record_activity(NewActivity {
                project_id: record.project_id,
                activity_type: ActivityType::WorkItemCreated,
                entity_type: "work_item".to_string(),
                entity_id: Some(record.work_item_id),
                summary: format!("WorkItem '{}' created", record.title),
                metadata: json!({}),
            })
            .await?;
        Ok(record)
    }

    pub async fn get_work_item(&self, work_item_id: Uuid) -> ArgusResult<Option<WorkItemRecord>> {
        self.repo.get_work_item(work_item_id).await
    }

    pub async fn list_work_items(&self, project_id: Uuid) -> ArgusResult<Vec<WorkItemRecord>> {
        self.repo.list_work_items(project_id).await
    }

    pub async fn list_active_work_items(
        &self,
        project_id: Uuid,
    ) -> ArgusResult<Vec<WorkItemRecord>> {
        Ok(self
            .repo
            .list_work_items(project_id)
            .await?
            .into_iter()
            .filter(|w| w.status == WorkItemStatus::Active)
            .collect())
    }

    pub async fn list_blocked_work_items(
        &self,
        project_id: Uuid,
    ) -> ArgusResult<Vec<WorkItemRecord>> {
        Ok(self
            .repo
            .list_work_items(project_id)
            .await?
            .into_iter()
            .filter(|w| w.status == WorkItemStatus::Blocked)
            .collect())
    }

    /// Set a WorkItem's status. Marking it `VerifiedComplete`/`Accepted` is
    /// gated on evidence: a WorkItem with a `definition_of_done` cannot be
    /// declared verified until at least one evidence row attaches it to
    /// something real (an artifact, task, or execution). An Agent's report
    /// (`ReportedComplete`) is never enough.
    pub async fn set_work_item_status(
        &self,
        work_item_id: Uuid,
        status: WorkItemStatus,
    ) -> ArgusResult<()> {
        if matches!(
            status,
            WorkItemStatus::VerifiedComplete | WorkItemStatus::Accepted
        ) {
            if let Some(work_item) = self.repo.get_work_item(work_item_id).await? {
                if work_item.definition_of_done.is_some() {
                    let evidence = self.repo.list_work_item_evidence(work_item_id).await?;
                    if evidence.is_empty() {
                        return Err(ArgusError::Validation(format!(
                            "work item {work_item_id} has a definition of done but no evidence; \
                             attach artifact/task/execution evidence before verifying"
                        )));
                    }
                }
            }
        }

        self.repo.set_work_item_status(work_item_id, status).await?;
        if let Some(work_item) = self.repo.get_work_item(work_item_id).await? {
            let activity_type = if status.is_complete() {
                Some(ActivityType::WorkItemVerified)
            } else if status == WorkItemStatus::Active {
                Some(ActivityType::WorkItemStarted)
            } else {
                None
            };
            if let Some(activity_type) = activity_type {
                self.repo
                    .record_activity(NewActivity {
                        project_id: work_item.project_id,
                        activity_type,
                        entity_type: "work_item".to_string(),
                        entity_id: Some(work_item_id),
                        summary: format!("WorkItem '{}' {status}", work_item.title),
                        metadata: json!({ "title": work_item.title }),
                    })
                    .await?;
            }
        }
        Ok(())
    }

    /// A roadmap ordering edge: `work_item_id` may not start until
    /// `depends_on_id` is complete. Distinct from the runtime TaskScheduler DAG.
    pub async fn add_work_item_dependency(
        &self,
        work_item_id: Uuid,
        depends_on_id: Uuid,
    ) -> ArgusResult<()> {
        self.repo
            .add_work_item_dependency(work_item_id, depends_on_id)
            .await
    }

    pub async fn list_work_item_dependencies(
        &self,
        project_id: Uuid,
    ) -> ArgusResult<Vec<WorkItemDependencyRecord>> {
        self.repo.list_work_item_dependencies(project_id).await
    }

    /// Associate a concrete Task that contributes to this WorkItem. The
    /// relationship is intentionally 0..N — a WorkItem is not a Task.
    pub async fn link_work_item_task(&self, work_item_id: Uuid, task_id: Uuid) -> ArgusResult<()> {
        self.repo.link_work_item_task(work_item_id, task_id).await
    }

    pub async fn list_work_item_tasks(&self, work_item_id: Uuid) -> ArgusResult<Vec<Uuid>> {
        self.repo.list_work_item_tasks(work_item_id).await
    }

    /// Attach completion evidence (artifact / task / execution) to a WorkItem.
    /// This is the substrate the future Evaluation Harness reads, so a
    /// WorkItem can only become `VerifiedComplete` with something behind it.
    pub async fn add_work_item_evidence(
        &self,
        evidence: NewWorkItemEvidence,
    ) -> ArgusResult<WorkItemEvidenceRecord> {
        if evidence.evidence_type == "artifact" && evidence.artifact_id.is_none() {
            return Err(ArgusError::Validation(
                "artifact evidence requires an artifact_id".to_string(),
            ));
        }
        self.repo.add_work_item_evidence(evidence).await
    }

    pub async fn list_work_item_evidence(
        &self,
        work_item_id: Uuid,
    ) -> ArgusResult<Vec<WorkItemEvidenceRecord>> {
        self.repo.list_work_item_evidence(work_item_id).await
    }

    // ============================================================
    // Decision
    // ============================================================

    pub async fn record_decision(&self, decision: NewDecision) -> ArgusResult<DecisionRecord> {
        let record = self.repo.record_decision(decision).await?;
        if record.status == DecisionStatus::Accepted {
            self.repo
                .record_activity(NewActivity {
                    project_id: record.project_id,
                    activity_type: ActivityType::DecisionAccepted,
                    entity_type: "decision".to_string(),
                    entity_id: Some(record.decision_id),
                    summary: format!("Decision accepted: {}", record.title),
                    metadata: json!({}),
                })
                .await?;
        }
        Ok(record)
    }

    pub async fn list_decisions(&self, project_id: Uuid) -> ArgusResult<Vec<DecisionRecord>> {
        self.repo.list_decisions(project_id).await
    }

    /// A decision is never overwritten: the old one becomes `Superseded` and
    /// points at the new one.
    pub async fn supersede_decision(&self, old_id: Uuid, new_id: Uuid) -> ArgusResult<()> {
        self.repo.supersede_decision(old_id, new_id).await
    }

    // ============================================================
    // Risk
    // ============================================================

    pub async fn create_risk(&self, risk: NewRisk) -> ArgusResult<RiskRecord> {
        let record = self.repo.create_risk(risk).await?;
        self.repo
            .record_activity(NewActivity {
                project_id: record.project_id,
                activity_type: ActivityType::RiskOpened,
                entity_type: "risk".to_string(),
                entity_id: Some(record.risk_id),
                summary: format!("Risk opened: {}", record.title),
                metadata: json!({ "severity": record.severity.as_str() }),
            })
            .await?;
        Ok(record)
    }

    pub async fn list_risks(&self, project_id: Uuid) -> ArgusResult<Vec<RiskRecord>> {
        self.repo.list_risks(project_id).await
    }

    pub async fn list_open_risks(&self, project_id: Uuid) -> ArgusResult<Vec<RiskRecord>> {
        Ok(self
            .repo
            .list_risks(project_id)
            .await?
            .into_iter()
            .filter(|r| r.status == RiskStatus::Open)
            .collect())
    }

    pub async fn set_risk_status(&self, risk_id: Uuid, status: RiskStatus) -> ArgusResult<()> {
        self.repo.set_risk_status(risk_id, status).await?;
        Ok(())
    }

    // ============================================================
    // Blocker
    // ============================================================

    pub async fn create_blocker(&self, blocker: NewBlocker) -> ArgusResult<BlockerRecord> {
        let record = self.repo.create_blocker(blocker).await?;
        self.repo
            .record_activity(NewActivity {
                project_id: record.project_id,
                activity_type: ActivityType::BlockerOpened,
                entity_type: "blocker".to_string(),
                entity_id: Some(record.blocker_id),
                summary: format!("Blocker opened: {}", record.title),
                metadata: json!({}),
            })
            .await?;
        Ok(record)
    }

    pub async fn list_blockers(&self, project_id: Uuid) -> ArgusResult<Vec<BlockerRecord>> {
        self.repo.list_blockers(project_id).await
    }

    pub async fn resolve_blocker(&self, blocker_id: Uuid) -> ArgusResult<()> {
        self.repo.resolve_blocker(blocker_id).await?;
        Ok(())
    }

    // ============================================================
    // Deferred Improvement
    // ============================================================

    pub async fn create_improvement(
        &self,
        improvement: NewImprovement,
    ) -> ArgusResult<ImprovementRecord> {
        let record = self.repo.create_improvement(improvement).await?;
        self.repo
            .record_activity(NewActivity {
                project_id: record.project_id,
                activity_type: ActivityType::ImprovementRecorded,
                entity_type: "improvement".to_string(),
                entity_id: Some(record.improvement_id),
                summary: format!(
                    "Deferred improvement recorded: {}",
                    record
                        .external_key
                        .clone()
                        .unwrap_or_else(|| record.title.clone())
                ),
                metadata: json!({ "status": record.status.as_str() }),
            })
            .await?;
        Ok(record)
    }

    pub async fn list_improvements(&self, project_id: Uuid) -> ArgusResult<Vec<ImprovementRecord>> {
        self.repo.list_improvements(project_id).await
    }

    pub async fn list_deferred_improvements(
        &self,
        project_id: Uuid,
    ) -> ArgusResult<Vec<ImprovementRecord>> {
        Ok(self
            .repo
            .list_improvements(project_id)
            .await?
            .into_iter()
            .filter(|i| i.status == ImprovementStatus::Deferred)
            .collect())
    }

    pub async fn set_improvement_status(
        &self,
        improvement_id: Uuid,
        status: ImprovementStatus,
    ) -> ArgusResult<()> {
        self.repo
            .set_improvement_status(improvement_id, status)
            .await?;
        Ok(())
    }

    // ============================================================
    // Activity
    // ============================================================

    pub async fn record_activity(&self, activity: NewActivity) -> ArgusResult<ActivityRecord> {
        self.repo.record_activity(activity).await
    }

    pub async fn list_activity(
        &self,
        project_id: Uuid,
        limit: i64,
    ) -> ArgusResult<Vec<ActivityRecord>> {
        self.repo.list_activity(project_id, limit).await
    }

    /// Minimal V0.38 → V0.39 hook: turn a persisted Git-checkpoint artifact
    /// into a durable `ArtifactProduced` project activity. Deliberately an
    /// explicit service call rather than a change to the frozen V0.38
    /// `finalize` pipeline.
    pub async fn record_artifact_produced(
        &self,
        project_id: Uuid,
        artifact_id: Uuid,
        task_id: Uuid,
        commit_sha: &str,
        files_changed: &[String],
    ) -> ArgusResult<ActivityRecord> {
        self.repo
            .record_activity(NewActivity {
                project_id,
                activity_type: ActivityType::ArtifactProduced,
                entity_type: "artifact".to_string(),
                entity_id: Some(artifact_id),
                summary: format!(
                    "Artifact {commit_sha} produced ({} file(s))",
                    files_changed.len()
                ),
                metadata: json!({
                    "artifact_id": artifact_id,
                    "task_id": task_id,
                    "commit_sha": commit_sha,
                    "files_changed": files_changed,
                }),
            })
            .await
    }

    // ============================================================
    // Context Compiler (V0.41)
    // ============================================================

    /// Compile a deterministic, role/purpose-aware [`crate::context::ContextPacket`].
    /// Read-only: it never mutates Project State and never calls a model.
    pub async fn compile_context(
        &self,
        request: &crate::context::ContextRequest,
    ) -> ArgusResult<crate::context::ContextPacket> {
        crate::context::ContextCompiler::new(self.repo.clone())
            .compile(request)
            .await
    }

    // ============================================================
    // Roadmap Progress (V0.40)
    // ============================================================

    /// The full deterministic roadmap with verified progress. Reads only.
    pub async fn get_roadmap_progress(&self, project_id: Uuid) -> ArgusResult<RoadmapProgress> {
        // 404 rather than an empty roadmap for an unknown project.
        self.get_project(project_id).await?;
        super::progress::ProgressEngine::calculate(self.repo.as_ref(), project_id).await
    }

    /// Compact deterministic progress report for the `/progress` view.
    pub async fn get_progress_report(&self, project_id: Uuid) -> ArgusResult<ProgressReport> {
        let roadmap = self.get_roadmap_progress(project_id).await?;
        let active_work_items = collect_items(&roadmap, |w| w.status == WorkItemStatus::Active);
        let completion_percent = (roadmap.overall.completion_ratio * 1000.0).round() / 10.0;
        Ok(ProgressReport {
            project_id,
            overall: roadmap.overall,
            completion_percent,
            current_milestone: roadmap.current_milestone,
            current_phase: roadmap.current_phase,
            active_work_items,
            next_candidates: roadmap.next_candidates,
            updated_at: roadmap.updated_at,
        })
    }

    /// Record the current progress as a `ProgressRecorded` activity, so a
    /// later `ProgressDiff` has a baseline. Not called every tick — only on
    /// meaningful project-state mutations.
    pub async fn record_progress_snapshot(&self, project_id: Uuid) -> ArgusResult<RoadmapProgress> {
        let progress = self.get_roadmap_progress(project_id).await?;
        let metadata = serde_json::to_value(&progress)?;
        self.record_activity(NewActivity {
            project_id,
            activity_type: ActivityType::ProgressRecorded,
            entity_type: "project".to_string(),
            entity_id: Some(project_id),
            summary: format!(
                "Progress recorded: {:.0}% verified",
                progress.overall.completion_ratio * 100.0
            ),
            metadata,
        })
        .await?;
        Ok(progress)
    }

    /// The most recently recorded progress snapshot, if any.
    pub async fn latest_recorded_progress(
        &self,
        project_id: Uuid,
    ) -> ArgusResult<Option<RoadmapProgress>> {
        let activities = self.repo.list_activity(project_id, 500).await?;
        for activity in activities {
            if activity.activity_type == ActivityType::ProgressRecorded {
                if let Ok(progress) = serde_json::from_value::<RoadmapProgress>(activity.metadata) {
                    return Ok(Some(progress));
                }
            }
        }
        Ok(None)
    }

    /// Diff the current progress against the most recently recorded snapshot.
    /// If nothing was recorded, the baseline is empty (everything reads as
    /// newly added). Computation never mutates state.
    pub async fn get_progress_diff(&self, project_id: Uuid) -> ArgusResult<ProgressDiff> {
        let current = self.get_roadmap_progress(project_id).await?;
        let before = match self.latest_recorded_progress(project_id).await? {
            Some(before) => before,
            None => empty_progress_baseline(project_id),
        };
        Ok(ProgressDiff::between(&before, &current))
    }

    // ============================================================
    // Snapshot
    // ============================================================

    pub async fn get_current_milestone(
        &self,
        project_id: Uuid,
    ) -> ArgusResult<Option<MilestoneRecord>> {
        let milestones = self.repo.list_milestones(project_id).await?;
        Ok(pick_current_milestone(&milestones))
    }

    /// The deterministic, machine-readable summary of a project's state.
    pub async fn get_project_snapshot(&self, project_id: Uuid) -> ArgusResult<ProjectSnapshot> {
        let project = self.get_project(project_id).await?;
        let charter = self.repo.get_charter(project_id).await?;
        let milestones = self.repo.list_milestones(project_id).await?;
        let work_items = self.repo.list_work_items(project_id).await?;
        let risks = self.repo.list_risks(project_id).await?;
        let blockers = self.repo.list_blockers(project_id).await?;
        let improvements = self.repo.list_improvements(project_id).await?;
        let recent_artifacts = self.repo.recent_artifacts(project_id, 5).await?;
        let recent_activity = self.repo.list_activity(project_id, 10).await?;

        let current_milestone = pick_current_milestone(&milestones);

        let active_work_items: Vec<WorkItemRecord> = work_items
            .iter()
            .filter(|w| w.status == WorkItemStatus::Active)
            .cloned()
            .collect();
        let blocked_work_items: Vec<WorkItemRecord> = work_items
            .iter()
            .filter(|w| w.status == WorkItemStatus::Blocked)
            .cloned()
            .collect();
        let open_risks: Vec<RiskRecord> = risks
            .iter()
            .filter(|r| r.status == RiskStatus::Open)
            .cloned()
            .collect();
        let open_blockers: Vec<BlockerRecord> = blockers
            .iter()
            .filter(|b| b.status == BlockerStatus::Open)
            .cloned()
            .collect();
        let deferred_improvements: Vec<ImprovementRecord> = improvements
            .iter()
            .filter(|i| i.status == ImprovementStatus::Deferred)
            .cloned()
            .collect();

        let mut next_candidates: Vec<WorkItemRecord> = work_items
            .iter()
            .filter(|w| w.status == WorkItemStatus::Planned)
            .cloned()
            .collect();
        next_candidates.sort_by(|a, b| {
            b.priority
                .value()
                .cmp(&a.priority.value())
                .then_with(|| a.created_at.cmp(&b.created_at))
        });

        Ok(ProjectSnapshot {
            project,
            charter,
            current_milestone,
            milestones,
            active_work_items,
            blocked_work_items,
            open_risks,
            open_blockers,
            deferred_count: deferred_improvements.len(),
            deferred_improvements,
            recent_artifacts,
            recent_activity,
            next_candidates,
            updated_at: chrono::Utc::now(),
        })
    }

    /// Deterministic human-readable rendering of project state *and* verified
    /// progress. Derived from the same structured data as
    /// [`Self::get_project_snapshot`]/[`Self::get_roadmap_progress`] — never a
    /// prose summary and never LLM-generated.
    pub async fn render_status(&self, project_id: Uuid) -> ArgusResult<String> {
        let snapshot = self.get_project_snapshot(project_id).await?;
        let progress = self.get_roadmap_progress(project_id).await?;
        Ok(render_roadmap_status(&snapshot, &progress))
    }

    // ============================================================
    // ARGUS self-bootstrap
    // ============================================================

    /// Bootstrap ARGUS's own Project State and Roadmap: charter, the
    /// V0.38–V0.42 milestones, the V0.40 phase and work items, and the
    /// currently known deferred improvements. Idempotent — safe on every boot.
    ///
    /// This is an explicit one-time import: the database becomes the runtime
    /// source of truth, and `docs/MASTER_ROADMAP.md` stays human documentation
    /// that is never parsed at runtime.
    pub async fn bootstrap_argus(&self) -> ArgusResult<ProjectRecord> {
        let project = self
            .create_project_state(NewProject {
                name: "ARGUS".to_string(),
                slug: "argus".to_string(),
                description: "ARGUS — Agent / Project Operating System".to_string(),
                priority: Priority::High,
            })
            .await?;
        let project_id = project.project_id;

        self.repo
            .save_charter(NewCharter {
                project_id,
                vision: "Agent / Project Operating System".to_string(),
                goal: "Reliably plan, execute, verify, and manage multiple long-lived AI projects."
                    .to_string(),
                success_criteria: vec![
                    "Project truth lives in durable structured state, not chat history."
                        .to_string(),
                    "Agent output is verified before a project records it as complete.".to_string(),
                    "Progress is computed deterministically from verified evidence, with no model."
                        .to_string(),
                ],
                constraints: vec![
                    "No LLM generates project state, roadmap, or progress.".to_string(),
                    "V0.38 and V0.39 behavior must not regress.".to_string(),
                ],
                non_goals: vec![
                    "Training a foundation model.".to_string(),
                    "Vector / semantic project memory before the Context Compiler.".to_string(),
                ],
            })
            .await?;

        // --- Milestones (external_key = version handle) ---
        let v038 = self
            .ensure_milestone(
                project_id,
                "V0.38 Reliable Agent Delivery Runtime",
                "Durable scheduler, execution loop, recovery, spend/budget, \
                 workspace isolation, git checkpoints, task artifacts.",
                1,
                "Crash-safe, budget-safe, isolated Agent delivery ending in a verified \
                 Git checkpoint and a persisted task artifact.",
                Some("V0.38"),
            )
            .await?;
        let v039 = self
            .ensure_milestone(
                project_id,
                "V0.39 Project State / Memory",
                "Structured, durable project state — the source of truth for planning.",
                2,
                "Project state is persisted, snapshotted, served, and ARGUS bootstraps \
                 itself into it without any LLM.",
                Some("V0.39"),
            )
            .await?;
        let v040 = self
            .ensure_milestone(
                project_id,
                "V0.40 Roadmap / Progress Engine",
                "Deterministic, evidence-driven roadmap and progress computation.",
                3,
                "Roadmap hierarchy, dependency-aware progress, progress diff, and API all \
                 derive from verified state with no LLM.",
                Some("V0.40"),
            )
            .await?;
        let v041 = self
            .ensure_milestone(
                project_id,
                "V0.41 Context Compiler",
                "Compile structured project state into planner/Agent context.",
                4,
                "A deterministic context compiler over Project State exists.",
                Some("V0.41"),
            )
            .await?;
        let v042 = self
            .ensure_milestone(
                project_id,
                "V0.42 Task Intelligence / Routing",
                "Task difficulty, model tier, and provider routing.",
                5,
                "Routing decisions are driven by task characteristics and provider state.",
                Some("V0.42"),
            )
            .await?;
        let v043 = self
            .ensure_milestone(
                project_id,
                "V0.43 Planner Harness / Protocol",
                "Provider-neutral planning protocol: request, result, proposal, diff, apply.",
                6,
                "A Planner can propose structured changes; ARGUS validates, diffs, approves, \
                 and transactionally applies them without any provider coupling.",
                Some("V0.43"),
            )
            .await?;

        if v038.status != MilestoneStatus::Completed {
            self.set_milestone_status(v038.milestone_id, MilestoneStatus::Completed)
                .await?;
        }
        if v039.status != MilestoneStatus::Completed {
            self.set_milestone_status(v039.milestone_id, MilestoneStatus::Completed)
                .await?;
        }
        if v040.status != MilestoneStatus::Completed {
            self.set_milestone_status(v040.milestone_id, MilestoneStatus::Completed)
                .await?;
        }
        if v041.status != MilestoneStatus::Completed {
            self.set_milestone_status(v041.milestone_id, MilestoneStatus::Completed)
                .await?;
        }
        if v042.status != MilestoneStatus::Completed {
            self.set_milestone_status(v042.milestone_id, MilestoneStatus::Completed)
                .await?;
        }
        if v043.status != MilestoneStatus::Active {
            self.set_milestone_status(v043.milestone_id, MilestoneStatus::Active)
                .await?;
        }

        // --- V0.40 phase + work items ---
        let phase = self
            .ensure_phase(
                project_id,
                v040.milestone_id,
                "Progress Engine",
                "Deterministic progress calculation over the structured roadmap.",
                1,
                Some("P40-Phase-1"),
            )
            .await?;
        if phase.status != MilestoneStatus::Active {
            self.set_phase_status(phase.phase_id, MilestoneStatus::Active)
                .await?;
        }

        let _progress_engine = self
            .ensure_work_item(
                project_id,
                Some(v040.milestone_id),
                Some(phase.phase_id),
                "Deterministic Progress Engine",
                "Compute weighted, evidence-driven progress for the roadmap.",
                Some("P40-001"),
                Priority::High,
            )
            .await?;
        let _progress_diff = self
            .ensure_work_item(
                project_id,
                Some(v040.milestone_id),
                Some(phase.phase_id),
                "Progress Diff",
                "Compare two progress snapshots to report what changed.",
                Some("P40-002"),
                Priority::Normal,
            )
            .await?;
        for item in [&_progress_engine, &_progress_diff] {
            if !item.status.is_complete() {
                self.verify_work_item(
                    item.work_item_id,
                    "V0.40 closed: roadmap_progress_e2e (evidence-driven completion, \
                     deferred denominator, parent aggregation, weights, dependencies, \
                     diff, isolation, determinism/restart) passes against real PostgreSQL.",
                )
                .await?;
            }
        }

        // --- V0.41: completed; its work item is verified ---
        let context_compiler = self
            .ensure_work_item(
                project_id,
                Some(v041.milestone_id),
                None,
                "Context Compiler",
                "Compile Project State into bounded, structured, provenance-tracked \
                 context per role and purpose.",
                Some("P41-001"),
                Priority::High,
            )
            .await?;
        self.verify_work_item(
            context_compiler.work_item_id,
            "V0.41 closed: context_compiler_e2e (ARGUS self-context, developer \
             isolation, PM breadth, reviewer evidence, superseded decisions, budget \
             truncation, determinism/restart, isolation, provenance) passes against \
             real PostgreSQL.",
        )
        .await?;

        // --- V0.42: active milestone, phase, and work item ---
        let task_phase = self
            .ensure_phase(
                project_id,
                v042.milestone_id,
                "Task Classification & Routing",
                "Deterministic task classification, capability tiers, and model routing policy.",
                1,
                Some("P42-Phase-1"),
            )
            .await?;
        if task_phase.status != MilestoneStatus::Active {
            self.set_phase_status(task_phase.phase_id, MilestoneStatus::Active)
                .await?;
        }
        let task_intelligence = self
            .ensure_work_item(
                project_id,
                Some(v042.milestone_id),
                Some(task_phase.phase_id),
                "Task Intelligence / Model Routing",
                "Implement deterministic task classification: build an explainable \
                 TaskProfile (complexity, risk, ambiguity, reasoning, scope, verification, \
                 context, reversibility) for each work item and route it to a minimum \
                 capability tier T0..T3 with retry and escalation policy.",
                Some("P42-001"),
                Priority::High,
            )
            .await?;
        self.verify_work_item(
            task_intelligence.work_item_id,
            "V0.42 closed: task_intelligence_e2e (ARGUS self-classification, four task \
             shapes, prompt-length independence, failure escalation matrix, outcomes + \
             statistics, determinism/restart, project isolation) passes against real \
             PostgreSQL.",
        )
        .await?;

        // --- V0.43: active milestone, phase, and work item ---
        let planner_phase = self
            .ensure_phase(
                project_id,
                v043.milestone_id,
                "Planner Protocol",
                "PlanningRequest/Result protocol, validation, diff, and transactional apply.",
                1,
                Some("P43-Phase-1"),
            )
            .await?;
        if planner_phase.status != MilestoneStatus::Active {
            self.set_phase_status(planner_phase.phase_id, MilestoneStatus::Active)
                .await?;
        }
        let planner = self
            .ensure_work_item(
                project_id,
                Some(v043.milestone_id),
                Some(planner_phase.phase_id),
                "Planner Harness / Protocol",
                "Implement a provider-neutral planning protocol: PlanningRequest bound to a \
                 context hash, a structured PlanProposal, a validator (no fabricated \
                 verification or artifacts, project-scoped), a deterministic PlanDiff, \
                 explicit approve/reject, and a transactional idempotent apply.",
                Some("P43-001"),
                Priority::High,
            )
            .await?;
        if planner.status == WorkItemStatus::Planned {
            self.set_work_item_status(planner.work_item_id, WorkItemStatus::Active)
                .await?;
        }

        // --- V0.39 foundation: verified with explicit evidence ---
        let foundation = self
            .ensure_work_item(
                project_id,
                Some(v039.milestone_id),
                None,
                "Project State Foundation",
                "Persist and serve structured project state: charter, milestones, work \
                 items, decisions, risks, blockers, deferred improvements, activities, \
                 and deterministic snapshots.",
                Some("P39-001"),
                Priority::High,
            )
            .await?;
        self.verify_work_item(
            foundation.work_item_id,
            "V0.39 closed: project_state_e2e (CRUD/snapshot, restart, isolation, \
             source-of-truth, self-bootstrap) passes against real PostgreSQL.",
        )
        .await?;

        // --- Deferred improvements currently tracked in Markdown ---
        let existing_improvements = self.repo.list_improvements(project_id).await?;
        let deferred: [(&str, &str, &str); 10] = [
            (
                "DEF-010",
                "create_task idempotency integration tests use an in-memory-only scheduler",
                "Both API idempotency integration tests build a TaskScheduler with no \
                 Postgres repository, so the handler's persisted-task read finds nothing.",
            ),
            (
                "DEF-011",
                "Unknown Agent Cost Semantics",
                "Spend rows with no reported cost are persisted as 0.0 (the NOT NULL \
                 sentinel). True unknown-cost accounting/enforcement is not yet modeled.",
            ),
            (
                "DEF-012",
                "Embedded migration rebuild detection",
                "sqlx::migrate! embeds migration files at compile time and cargo does not \
                 notice a newly added migration file, so a stale binary can silently skip it.",
            ),
            (
                "DEF-013",
                "phases.weight is stored but unused",
                "Progress aggregates WorkItems by their own weight; phase weight is persisted \
                 but never read. Changing it would change progress math.",
            ),
            (
                "DEF-014",
                "ProgressDiff matches deferred/risk/blocker transitions by title",
                "A rename reads as an add plus a resolve. Keying diffs by id would change \
                 the output shape.",
            ),
            (
                "DEF-015",
                "Empty Planned milestone contributes weight 1 at ratio 0",
                "A Planned milestone with no WorkItems dilutes the project overall. The rule \
                 is deterministic but may need revisiting.",
            ),
            (
                "DEF-016",
                "Classifier uncommon-wording limitations",
                "Task kinds inferred by keyword can miss unusual wording and fall back to \
                 unknown, leaving the tier driven by complexity signals alone.",
            ),
            (
                "DEF-017",
                "WorkItems lack richer execution metadata",
                "WorkItems have title/description/DoD but no tags, acceptance criteria, or \
                 test commands, so task intelligence sees coarser signals than for Tasks.",
            ),
            (
                "DEF-018",
                "Per-attempt tier history not wired into the scheduler",
                "Escalation uses the routing decision's tier; the ExecutionLoop does not yet \
                 feed per-attempt tier/failure history back into routing.",
            ),
            (
                "DEF-019",
                "Exact Planner Context Snapshot Replay",
                "A PlanningRequest binds a context_hash but does not persist the compiled \
                 packet; manual export regenerates it, which can differ if state changed.",
            ),
        ];
        for (key, title, why) in deferred {
            if !existing_improvements
                .iter()
                .any(|i| i.external_key.as_deref() == Some(key))
            {
                self.repo
                    .create_improvement(NewImprovement {
                        project_id,
                        external_key: Some(key.to_string()),
                        title: title.to_string(),
                        description: None,
                        priority: ImprovementPriority::Medium,
                        status: ImprovementStatus::Deferred,
                        source: Some("docs/DEFERRED_IMPROVEMENTS.md".to_string()),
                        why_deferred: Some(why.to_string()),
                        required_before: Some("Next touch of the relevant subsystem.".to_string()),
                    })
                    .await?;
            }
        }

        self.get_project(project_id).await
    }

    async fn ensure_milestone(
        &self,
        project_id: Uuid,
        title: &str,
        description: &str,
        sequence: i32,
        definition_of_done: &str,
        external_key: Option<&str>,
    ) -> ArgusResult<MilestoneRecord> {
        let existing = self.repo.list_milestones(project_id).await?;
        if let Some(found) = existing.iter().find(|m| m.title == title) {
            return Ok(found.clone());
        }
        self.repo
            .create_milestone(NewMilestone {
                project_id,
                title: title.to_string(),
                description: Some(description.to_string()),
                sequence: Some(sequence),
                parent_id: None,
                definition_of_done: Some(definition_of_done.to_string()),
                external_key: external_key.map(str::to_string),
                weight: 1,
            })
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn ensure_phase(
        &self,
        project_id: Uuid,
        milestone_id: Uuid,
        title: &str,
        description: &str,
        sequence: i32,
        external_key: Option<&str>,
    ) -> ArgusResult<PhaseRecord> {
        let existing = self.repo.list_phases(project_id).await?;
        if let Some(found) = existing
            .iter()
            .find(|p| p.milestone_id == milestone_id && p.title == title)
        {
            return Ok(found.clone());
        }
        self.repo
            .create_phase(NewPhase {
                project_id,
                milestone_id,
                title: title.to_string(),
                description: Some(description.to_string()),
                sequence: Some(sequence),
                external_key: external_key.map(str::to_string),
                definition_of_done: Some(description.to_string()),
                weight: 1,
            })
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn ensure_work_item(
        &self,
        project_id: Uuid,
        milestone_id: Option<Uuid>,
        phase_id: Option<Uuid>,
        title: &str,
        description: &str,
        external_key: Option<&str>,
        priority: Priority,
    ) -> ArgusResult<WorkItemRecord> {
        let existing = self.repo.list_work_items(project_id).await?;
        if let Some(found) = existing.iter().find(|w| w.title == title) {
            return Ok(found.clone());
        }
        self.repo
            .create_work_item(NewWorkItem {
                project_id,
                milestone_id,
                phase_id,
                title: title.to_string(),
                description: description.to_string(),
                priority,
                definition_of_done: Some(description.to_string()),
                source: Some("bootstrap".to_string()),
                external_key: external_key.map(str::to_string),
                weight: 1,
            })
            .await
    }

    /// Idempotently mark a WorkItem `VerifiedComplete` with an explicit note
    /// evidence row (required by the V0.40 evidence gate). A no-op when the
    /// item is already complete, so repeated bootstraps do not duplicate
    /// evidence or activities.
    async fn verify_work_item(&self, work_item_id: Uuid, note: &str) -> ArgusResult<()> {
        if let Some(w) = self.repo.get_work_item(work_item_id).await? {
            if w.status.is_complete() {
                return Ok(());
            }
        }
        let evidence = self.repo.list_work_item_evidence(work_item_id).await?;
        if evidence.is_empty() {
            self.repo
                .add_work_item_evidence(NewWorkItemEvidence {
                    work_item_id,
                    evidence_type: "note".to_string(),
                    artifact_id: None,
                    task_id: None,
                    execution_id: None,
                    note: Some(note.to_string()),
                })
                .await?;
        }
        self.set_work_item_status(work_item_id, WorkItemStatus::VerifiedComplete)
            .await
    }
}

/// Current milestone = the Active milestone with the lowest sequence; if none
/// is Active, the first Planned one; otherwise `None`.
fn pick_current_milestone(milestones: &[MilestoneRecord]) -> Option<MilestoneRecord> {
    milestones
        .iter()
        .filter(|m| m.status == MilestoneStatus::Active)
        .min_by_key(|m| m.sequence)
        .or_else(|| {
            milestones
                .iter()
                .filter(|m| m.status == MilestoneStatus::Planned)
                .min_by_key(|m| m.sequence)
        })
        .cloned()
}

#[cfg(test)]
fn render_snapshot(s: &ProjectSnapshot) -> String {
    let mut out = String::new();
    out.push_str(&format!("{}\n", s.project.name));

    out.push_str("\nCurrent Milestone:\n");
    match &s.current_milestone {
        Some(m) => out.push_str(&format!("{}\n", m.title)),
        None => out.push_str("None\n"),
    }

    out.push_str("\nCompleted:\n");
    let completed: Vec<&MilestoneRecord> = s
        .milestones
        .iter()
        .filter(|m| m.status == MilestoneStatus::Completed)
        .collect();
    if completed.is_empty() {
        out.push_str("None\n");
    } else {
        for m in completed {
            out.push_str(&format!("- {}\n", m.title));
        }
    }

    out.push_str("\nActive:\n");
    if s.active_work_items.is_empty() {
        out.push_str("None\n");
    } else {
        for w in &s.active_work_items {
            out.push_str(&format!("- {}\n", w.title));
        }
    }

    out.push_str("\nBlocked:\n");
    if s.blocked_work_items.is_empty() && s.open_blockers.is_empty() {
        out.push_str("None\n");
    } else {
        for w in &s.blocked_work_items {
            out.push_str(&format!("- {}\n", w.title));
        }
        for b in &s.open_blockers {
            out.push_str(&format!("- {}\n", b.title));
        }
    }

    out.push_str("\nRisks:\n");
    if s.open_risks.is_empty() {
        out.push_str("None\n");
    } else {
        for r in &s.open_risks {
            out.push_str(&format!("- [{}] {}\n", r.severity.as_str(), r.title));
        }
    }

    out.push_str("\nDeferred:\n");
    if s.deferred_improvements.is_empty() {
        out.push_str("None\n");
    } else {
        for i in &s.deferred_improvements {
            match &i.external_key {
                Some(key) => out.push_str(&format!("- {key} {}\n", i.title)),
                None => out.push_str(&format!("- {}\n", i.title)),
            }
        }
    }

    out.push_str("\nRecent Artifacts:\n");
    if s.recent_artifacts.is_empty() {
        out.push_str("None\n");
    } else {
        for a in &s.recent_artifacts {
            let short = &a.commit_sha[..8.min(a.commit_sha.len())];
            out.push_str(&format!("- {short} ({} file(s))\n", a.files_changed.len()));
        }
    }

    out.push_str("\nNext:\n");
    if s.next_candidates.is_empty() {
        out.push_str("None\n");
    } else {
        for w in &s.next_candidates {
            out.push_str(&format!("- {}\n", w.title));
        }
    }

    out
}

/// Deterministic project status rendering including verified progress. Same
/// structured inputs always render identically.
fn render_roadmap_status(s: &ProjectSnapshot, p: &RoadmapProgress) -> String {
    let mut out = String::new();
    out.push_str(&format!("{}\n", s.project.name));
    out.push_str(&format!(
        "\nOverall Verified Progress:\n{:.0}%\n",
        p.overall.completion_ratio * 100.0
    ));

    out.push_str("\nCurrent:\n");
    match &p.current_milestone {
        Some(m) => out.push_str(&format!("{}\n", m.title)),
        None => out.push_str("None\n"),
    }
    if let Some(phase) = &p.current_phase {
        out.push_str(&format!("Phase: {}\n", phase.title));
    }

    out.push_str("\nMilestones:\n");
    if p.milestones.is_empty() {
        out.push_str("None\n");
    } else {
        for m in &p.milestones {
            let mark = match m.status {
                MilestoneStatus::Completed => "✓",
                MilestoneStatus::Active => "→",
                MilestoneStatus::Blocked => "!",
                MilestoneStatus::Deferred => "-",
                MilestoneStatus::Cancelled => "x",
                MilestoneStatus::Planned => " ",
            };
            out.push_str(&format!(
                "{mark} {} {:.0}%\n",
                m.title,
                m.summary.completion_ratio * 100.0
            ));
        }
    }

    out.push_str(&format!("\nBlocked:\n{}\n", p.blocked_count));

    out.push_str("\nDeferred:\n");
    if s.deferred_improvements.is_empty() {
        out.push_str("None\n");
    } else {
        for i in &s.deferred_improvements {
            match &i.external_key {
                Some(key) => out.push_str(&format!("- {key} {}\n", i.title)),
                None => out.push_str(&format!("- {}\n", i.title)),
            }
        }
    }

    out.push_str("\nActive:\n");
    let active = collect_items(p, |w| w.status == WorkItemStatus::Active);
    if active.is_empty() {
        out.push_str("None\n");
    } else {
        for w in &active {
            out.push_str(&format!("- {}\n", w.title));
        }
    }

    out.push_str("\nRisks:\n");
    if p.open_risks.is_empty() {
        out.push_str("None\n");
    } else {
        for r in &p.open_risks {
            out.push_str(&format!("- {r}\n"));
        }
    }

    out.push_str("\nNext:\n");
    if p.next_candidates.is_empty() {
        out.push_str("None\n");
    } else {
        for w in &p.next_candidates {
            out.push_str(&format!("- {}\n", w.title));
        }
    }

    out
}

/// Every work item in the roadmap (direct + phased), in deterministic order.
fn collect_items<F>(p: &RoadmapProgress, keep: F) -> Vec<WorkItemProgress>
where
    F: Fn(&WorkItemProgress) -> bool,
{
    let mut items = Vec::new();
    for m in &p.milestones {
        for w in &m.work_items {
            if keep(w) {
                items.push(w.clone());
            }
        }
        for phase in &m.phases {
            for w in &phase.work_items {
                if keep(w) {
                    items.push(w.clone());
                }
            }
        }
    }
    items.sort_by(|a, b| {
        b.priority
            .value()
            .cmp(&a.priority.value())
            .then_with(|| a.work_item_id.cmp(&b.work_item_id))
    });
    items
}

fn empty_progress_baseline(project_id: Uuid) -> RoadmapProgress {
    RoadmapProgress {
        project_id,
        overall: ProgressSummary {
            total_items: 0,
            verified_items: 0,
            accepted_items: 0,
            reported_complete_items: 0,
            active_items: 0,
            blocked_items: 0,
            deferred_items: 0,
            cancelled_items: 0,
            completion_ratio: 0.0,
            confidence: ProgressConfidence::Unknown,
        },
        current_milestone: None,
        current_phase: None,
        milestones: Vec::new(),
        blocked_count: 0,
        deferred_count: 0,
        verified_count: 0,
        active_count: 0,
        open_risks: Vec::new(),
        open_blockers: Vec::new(),
        deferred: Vec::new(),
        next_candidates: Vec::new(),
        updated_at: chrono::Utc::now(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn milestone(sequence: i32, title: &str, status: MilestoneStatus) -> MilestoneRecord {
        let now = Utc::now();
        MilestoneRecord {
            milestone_id: Uuid::new_v4(),
            project_id: Uuid::new_v4(),
            title: title.to_string(),
            description: None,
            status,
            sequence,
            parent_id: None,
            definition_of_done: None,
            external_key: None,
            weight: 1,
            started_at: None,
            completed_at: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn work_item(title: &str, status: WorkItemStatus) -> WorkItemRecord {
        let now = Utc::now();
        WorkItemRecord {
            work_item_id: Uuid::new_v4(),
            project_id: Uuid::new_v4(),
            milestone_id: None,
            phase_id: None,
            title: title.to_string(),
            description: String::new(),
            status,
            priority: Priority::Normal,
            definition_of_done: None,
            source: None,
            external_key: None,
            weight: 1,
            created_at: now,
            updated_at: now,
            completed_at: None,
        }
    }

    fn base_snapshot() -> ProjectSnapshot {
        let now = Utc::now();
        ProjectSnapshot {
            project: ProjectRecord {
                project_id: Uuid::new_v4(),
                name: "ARGUS".to_string(),
                slug: "argus".to_string(),
                description: String::new(),
                status: ProjectStatus::Active,
                priority: Priority::High,
                git_repo_url: None,
                created_at: now,
                updated_at: now,
            },
            charter: None,
            current_milestone: None,
            milestones: Vec::new(),
            active_work_items: Vec::new(),
            blocked_work_items: Vec::new(),
            open_risks: Vec::new(),
            open_blockers: Vec::new(),
            deferred_improvements: Vec::new(),
            deferred_count: 0,
            recent_artifacts: Vec::new(),
            recent_activity: Vec::new(),
            next_candidates: Vec::new(),
            updated_at: now,
        }
    }

    #[test]
    fn test_pick_current_milestone_prefers_active_over_planned() {
        let ms = vec![
            milestone(2, "V0.39", MilestoneStatus::Planned),
            milestone(1, "V0.38", MilestoneStatus::Completed),
            milestone(3, "V0.40", MilestoneStatus::Active),
        ];
        let current = pick_current_milestone(&ms).expect("must pick one");
        assert_eq!(current.title, "V0.40");

        let only_planned = vec![milestone(2, "V0.39", MilestoneStatus::Planned)];
        assert_eq!(
            pick_current_milestone(&only_planned).unwrap().title,
            "V0.39"
        );

        assert!(pick_current_milestone(&[]).is_none());
    }

    #[test]
    fn test_render_status_is_deterministic_and_structured() {
        let mut s = base_snapshot();
        s.milestones = vec![
            milestone(
                1,
                "V0.38 Reliable Agent Delivery Runtime",
                MilestoneStatus::Completed,
            ),
            milestone(2, "V0.39 Project State / Memory", MilestoneStatus::Active),
        ];
        s.current_milestone = pick_current_milestone(&s.milestones);
        s.active_work_items = vec![work_item(
            "Project State Foundation",
            WorkItemStatus::Active,
        )];
        s.next_candidates = vec![work_item("Project State API", WorkItemStatus::Planned)];

        let rendered = render_snapshot(&s);
        assert!(rendered.starts_with("ARGUS\n"));
        assert!(rendered.contains("Current Milestone:\nV0.39 Project State / Memory"));
        assert!(rendered.contains("Completed:\n- V0.38 Reliable Agent Delivery Runtime"));
        assert!(rendered.contains("Active:\n- Project State Foundation"));
        assert!(rendered.contains("Blocked:\nNone"));
        assert!(rendered.contains("Risks:\nNone"));
        assert!(rendered.contains("Next:\n- Project State API"));

        // Deterministic: rendering the same snapshot twice is identical.
        assert_eq!(rendered, render_snapshot(&s));
    }
}
