//! V0.40 deterministic Progress Engine.
//!
//! ```text
//! Facts -> Roadmap Structure -> Verification State -> Deterministic Progress
//! ```
//!
//! The engine only **reads and computes**. It never mutates project state, and
//! it never asks a model anything: formal completion only counts work that
//! reached `VerifiedComplete` or `Accepted`. `ReportedComplete` (an Agent
//! saying "done") contributes nothing.
//!
//! The `build` method is a pure function of the structured records, so the
//! same database state always produces byte-identical output (except the
//! `updated_at` field, which is derived from the data, not the wall clock).

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::repository::ProjectStateRepository;
use super::types::*;

pub struct ProgressEngine;

impl ProgressEngine {
    /// Fetch everything needed and compute the roadmap progress.
    pub async fn calculate(
        repo: &dyn ProjectStateRepository,
        project_id: Uuid,
    ) -> crate::types::ArgusResult<RoadmapProgress> {
        let milestones = repo.list_milestones(project_id).await?;
        let phases = repo.list_phases(project_id).await?;
        let work_items = repo.list_work_items(project_id).await?;
        let dependencies = repo.list_work_item_dependencies(project_id).await?;
        let risks = repo.list_risks(project_id).await?;
        let blockers = repo.list_blockers(project_id).await?;
        let improvements = repo.list_improvements(project_id).await?;

        Ok(Self::build(
            project_id,
            &milestones,
            &phases,
            &work_items,
            &dependencies,
            &risks,
            &blockers,
            &improvements,
        ))
    }

    /// Pure computation — no I/O, no clock, no randomness.
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        project_id: Uuid,
        milestones: &[MilestoneRecord],
        phases: &[PhaseRecord],
        work_items: &[WorkItemRecord],
        dependencies: &[WorkItemDependencyRecord],
        risks: &[RiskRecord],
        blockers: &[BlockerRecord],
        improvements: &[ImprovementRecord],
    ) -> RoadmapProgress {
        let completed_ids: HashSet<Uuid> = work_items
            .iter()
            .filter(|w| w.status.is_complete())
            .map(|w| w.work_item_id)
            .collect();

        let mut deps_by_item: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
        for d in dependencies {
            deps_by_item
                .entry(d.work_item_id)
                .or_default()
                .push(d.depends_on_id);
        }
        for v in deps_by_item.values_mut() {
            v.sort();
            v.dedup();
        }

        let view = |w: &WorkItemRecord| -> WorkItemProgress {
            let depends_on = deps_by_item
                .get(&w.work_item_id)
                .cloned()
                .unwrap_or_default();
            let ready = w.status == WorkItemStatus::Planned
                && depends_on.iter().all(|id| completed_ids.contains(id));
            WorkItemProgress {
                work_item_id: w.work_item_id,
                title: w.title.clone(),
                status: w.status,
                priority: w.priority,
                weight: w.weight,
                external_key: w.external_key.clone(),
                completion: if w.status.is_complete() { 1.0 } else { 0.0 },
                included: !matches!(
                    w.status,
                    WorkItemStatus::Deferred | WorkItemStatus::Cancelled
                ),
                depends_on,
                ready,
            }
        };

        let mut ordered_milestones: Vec<&MilestoneRecord> = milestones.iter().collect();
        ordered_milestones.sort_by(|a, b| {
            a.sequence
                .cmp(&b.sequence)
                .then_with(|| a.milestone_id.cmp(&b.milestone_id))
        });

        let mut milestone_progress: Vec<MilestoneProgress> = Vec::new();
        for m in &ordered_milestones {
            let mut m_phases: Vec<&PhaseRecord> = phases
                .iter()
                .filter(|p| p.milestone_id == m.milestone_id)
                .collect();
            m_phases.sort_by(|a, b| {
                a.sequence
                    .cmp(&b.sequence)
                    .then_with(|| a.phase_id.cmp(&b.phase_id))
            });

            let direct: Vec<&WorkItemRecord> = work_items
                .iter()
                .filter(|w| w.milestone_id == Some(m.milestone_id) && w.phase_id.is_none())
                .collect();

            let mut phase_progress = Vec::new();
            let mut all_milestone_items: Vec<&WorkItemRecord> = direct.clone();
            for p in m_phases {
                let mut items: Vec<&WorkItemRecord> = work_items
                    .iter()
                    .filter(|w| w.phase_id == Some(p.phase_id))
                    .collect();
                sort_items(&mut items);
                all_milestone_items.extend(items.iter().copied());

                let summary = summarize_items(&items);
                phase_progress.push(PhaseProgress {
                    phase_id: p.phase_id,
                    title: p.title.clone(),
                    status: p.status,
                    sequence: p.sequence,
                    external_key: p.external_key.clone(),
                    summary,
                    work_items: items.iter().map(|w| view(w)).collect(),
                });
            }
            sort_items(&mut all_milestone_items);
            let direct_progress: Vec<WorkItemProgress> = {
                let mut d: Vec<&WorkItemRecord> = direct.clone();
                sort_items(&mut d);
                d.iter().map(|w| view(w)).collect()
            };

            let mut summary = summarize_items(&all_milestone_items);
            // An explicitly Completed milestone is 100%, whatever its items
            // say — but the engine still never *sets* that status.
            if m.status == MilestoneStatus::Completed {
                summary.completion_ratio = 1.0;
                summary.confidence = ProgressConfidence::Verified;
            }

            milestone_progress.push(MilestoneProgress {
                milestone_id: m.milestone_id,
                title: m.title.clone(),
                status: m.status,
                sequence: m.sequence,
                external_key: m.external_key.clone(),
                weight: m.weight,
                summary,
                work_items: direct_progress,
                phases: phase_progress,
            });
        }

        // --- Project overall: weighted average over milestones by their
        // own weight. Deferred/Cancelled milestones are excluded. ---
        let mut denominator = 0.0f64;
        let mut numerator = 0.0f64;
        let mut overall = ProgressSummary {
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
        };
        let mut any_estimated = false;
        for mp in &milestone_progress {
            if matches!(
                mp.status,
                MilestoneStatus::Deferred | MilestoneStatus::Cancelled
            ) {
                continue;
            }
            let weight = mp.summary_weight();
            if weight > 0.0 {
                denominator += weight;
                numerator += weight * mp.summary.completion_ratio;
            }
            add_summary(&mut overall, &mp.summary);
            if mp.summary.confidence == ProgressConfidence::Estimated {
                any_estimated = true;
            }
        }
        overall.completion_ratio = if denominator > 0.0 {
            numerator / denominator
        } else {
            0.0
        };
        overall.confidence = if overall.total_items == 0 && denominator == 0.0 {
            ProgressConfidence::Unknown
        } else if any_estimated {
            ProgressConfidence::Estimated
        } else {
            ProgressConfidence::Verified
        };

        // --- Current focus: Active (lowest sequence), else Planned. ---
        let current_milestone_record = ordered_milestones
            .iter()
            .find(|m| m.status == MilestoneStatus::Active)
            .or_else(|| {
                ordered_milestones
                    .iter()
                    .find(|m| m.status == MilestoneStatus::Planned)
            });
        let current_milestone = current_milestone_record.map(|m| RoadmapNodeRef {
            id: m.milestone_id,
            title: m.title.clone(),
            status: m.status,
            external_key: m.external_key.clone(),
        });

        let current_phase = current_milestone_record.and_then(|m| {
            let mut ps: Vec<&PhaseRecord> = phases
                .iter()
                .filter(|p| p.milestone_id == m.milestone_id)
                .collect();
            ps.sort_by(|a, b| {
                a.sequence
                    .cmp(&b.sequence)
                    .then_with(|| a.phase_id.cmp(&b.phase_id))
            });
            ps.iter()
                .find(|p| p.status == MilestoneStatus::Active)
                .or_else(|| ps.iter().find(|p| p.status == MilestoneStatus::Planned))
                .map(|p| RoadmapNodeRef {
                    id: p.phase_id,
                    title: p.title.clone(),
                    status: p.status,
                    external_key: p.external_key.clone(),
                })
        });

        // --- Next candidates: Planned, ready, in a live milestone/phase. ---
        let mut next_candidates: Vec<WorkItemProgress> = work_items
            .iter()
            .filter(|w| w.status == WorkItemStatus::Planned)
            .filter(|w| {
                let node_ok = |mid: Option<Uuid>, pid: Option<Uuid>| {
                    let m_ok = mid
                        .and_then(|id| milestones.iter().find(|m| m.milestone_id == id))
                        .is_some_and(|m| {
                            !matches!(
                                m.status,
                                MilestoneStatus::Deferred | MilestoneStatus::Cancelled
                            )
                        });
                    let p_ok = pid
                        .and_then(|id| phases.iter().find(|p| p.phase_id == id))
                        .is_none_or(|p| {
                            !matches!(
                                p.status,
                                MilestoneStatus::Deferred | MilestoneStatus::Cancelled
                            )
                        });
                    (mid.is_none() || m_ok) && p_ok
                };
                node_ok(w.milestone_id, w.phase_id)
            })
            .map(view)
            .filter(|w| w.ready)
            .collect();
        next_candidates.sort_by(|a, b| {
            b.priority
                .value()
                .cmp(&a.priority.value())
                .then_with(|| a.work_item_id.cmp(&b.work_item_id))
        });

        let mut open_risks: Vec<&RiskRecord> = risks
            .iter()
            .filter(|r| r.status == RiskStatus::Open)
            .collect();
        open_risks.sort_by(|a, b| {
            severity_rank(a.severity)
                .cmp(&severity_rank(b.severity))
                .then_with(|| a.risk_id.cmp(&b.risk_id))
        });
        let open_risks = open_risks.into_iter().map(|r| r.title.clone()).collect();

        let mut open_blockers: Vec<&BlockerRecord> = blockers
            .iter()
            .filter(|b| b.status == BlockerStatus::Open)
            .collect();
        open_blockers.sort_by(|a, b| a.blocker_id.cmp(&b.blocker_id));
        let open_blockers = open_blockers.into_iter().map(|b| b.title.clone()).collect();

        let mut deferred_records: Vec<&ImprovementRecord> = improvements
            .iter()
            .filter(|i| i.status == ImprovementStatus::Deferred)
            .collect();
        deferred_records.sort_by(|a, b| {
            a.external_key
                .cmp(&b.external_key)
                .then_with(|| a.improvement_id.cmp(&b.improvement_id))
        });
        let deferred = deferred_records
            .into_iter()
            .map(|i| match &i.external_key {
                Some(k) => format!("{k} {}", i.title),
                None => i.title.clone(),
            })
            .collect();

        let updated_at = latest_timestamp(milestones, phases, work_items);

        RoadmapProgress {
            project_id,
            blocked_count: overall.blocked_items,
            deferred_count: overall.deferred_items,
            verified_count: overall.verified_items + overall.accepted_items,
            active_count: overall.active_items,
            overall,
            current_milestone,
            current_phase,
            milestones: milestone_progress,
            open_risks,
            open_blockers,
            deferred,
            next_candidates,
            updated_at,
        }
    }
}

impl MilestoneProgress {
    /// Weight used when this milestone aggregates into the project overall.
    /// A Completed milestone contributes its (default 1) weight even with no
    /// work items, so finished history reads as progress rather than 0%.
    fn summary_weight(&self) -> f64 {
        self.weight.max(0) as f64
    }
}

fn sort_items(items: &mut Vec<&WorkItemRecord>) {
    items.sort_by(|a, b| {
        b.priority
            .value()
            .cmp(&a.priority.value())
            .then_with(|| a.created_at.cmp(&b.created_at))
            .then_with(|| a.work_item_id.cmp(&b.work_item_id))
    });
}

/// Deterministic, weighted summary over a flat set of work items.
fn summarize_items(items: &[&WorkItemRecord]) -> ProgressSummary {
    let mut s = ProgressSummary {
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
    };
    let mut total_weight = 0i64;
    let mut complete_weight = 0i64;
    let mut in_flight = false;

    for w in items {
        let weight = w.weight.max(0) as i64;
        match w.status {
            WorkItemStatus::Deferred => s.deferred_items += 1,
            WorkItemStatus::Cancelled => s.cancelled_items += 1,
            WorkItemStatus::Planned => {
                s.total_items += 1;
                total_weight += weight;
            }
            WorkItemStatus::Active => {
                s.total_items += 1;
                s.active_items += 1;
                total_weight += weight;
                in_flight = true;
            }
            WorkItemStatus::Blocked => {
                s.total_items += 1;
                s.blocked_items += 1;
                total_weight += weight;
                in_flight = true;
            }
            WorkItemStatus::ReportedComplete => {
                // Deliberately NOT complete.
                s.total_items += 1;
                s.reported_complete_items += 1;
                total_weight += weight;
                in_flight = true;
            }
            WorkItemStatus::VerifiedComplete => {
                s.total_items += 1;
                s.verified_items += 1;
                total_weight += weight;
                complete_weight += weight;
            }
            WorkItemStatus::Accepted => {
                s.total_items += 1;
                s.accepted_items += 1;
                total_weight += weight;
                complete_weight += weight;
            }
        }
    }

    s.completion_ratio = if total_weight > 0 {
        complete_weight as f64 / total_weight as f64
    } else {
        0.0
    };
    s.confidence = if s.total_items == 0 {
        ProgressConfidence::Unknown
    } else if in_flight {
        ProgressConfidence::Estimated
    } else {
        ProgressConfidence::Verified
    };
    s
}

fn add_summary(into: &mut ProgressSummary, from: &ProgressSummary) {
    into.total_items += from.total_items;
    into.verified_items += from.verified_items;
    into.accepted_items += from.accepted_items;
    into.reported_complete_items += from.reported_complete_items;
    into.active_items += from.active_items;
    into.blocked_items += from.blocked_items;
    into.deferred_items += from.deferred_items;
    into.cancelled_items += from.cancelled_items;
}

fn severity_rank(s: RiskSeverity) -> i32 {
    match s {
        RiskSeverity::Critical => 0,
        RiskSeverity::High => 1,
        RiskSeverity::Medium => 2,
        RiskSeverity::Low => 3,
    }
}

fn latest_timestamp(
    milestones: &[MilestoneRecord],
    phases: &[PhaseRecord],
    work_items: &[WorkItemRecord],
) -> DateTime<Utc> {
    let mut latest: Option<DateTime<Utc>> = None;
    for t in milestones
        .iter()
        .map(|m| m.updated_at)
        .chain(phases.iter().map(|p| p.updated_at))
        .chain(work_items.iter().map(|w| w.updated_at))
    {
        latest = Some(match latest {
            Some(prev) if prev > t => prev,
            _ => t,
        });
    }
    latest.unwrap_or_else(Utc::now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task_types::Priority;

    fn milestone(sequence: i32, title: &str, status: MilestoneStatus) -> MilestoneRecord {
        let now = Utc::now();
        MilestoneRecord {
            milestone_id: Uuid::new_v4(),
            project_id: Uuid::nil(),
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

    fn phase(milestone_id: Uuid, sequence: i32, title: &str) -> PhaseRecord {
        let now = Utc::now();
        PhaseRecord {
            phase_id: Uuid::new_v4(),
            project_id: Uuid::nil(),
            milestone_id,
            title: title.to_string(),
            description: None,
            status: MilestoneStatus::Active,
            sequence,
            external_key: None,
            definition_of_done: None,
            weight: 1,
            started_at: None,
            completed_at: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn item(
        milestone_id: Uuid,
        phase_id: Option<Uuid>,
        title: &str,
        status: WorkItemStatus,
        weight: i32,
    ) -> WorkItemRecord {
        let now = Utc::now();
        WorkItemRecord {
            work_item_id: Uuid::new_v4(),
            project_id: Uuid::nil(),
            milestone_id: Some(milestone_id),
            phase_id,
            title: title.to_string(),
            description: String::new(),
            status,
            priority: Priority::Normal,
            definition_of_done: None,
            source: None,
            external_key: None,
            weight,
            created_at: now,
            updated_at: now,
            completed_at: None,
        }
    }

    fn build(
        milestones: &[MilestoneRecord],
        phases: &[PhaseRecord],
        items: &[WorkItemRecord],
    ) -> RoadmapProgress {
        ProgressEngine::build(Uuid::nil(), milestones, phases, items, &[], &[], &[], &[])
    }

    #[test]
    fn reported_complete_and_verified_are_different() {
        let ms = milestone(1, "M", MilestoneStatus::Active);
        let reported = item(
            ms.milestone_id,
            None,
            "Reported",
            WorkItemStatus::ReportedComplete,
            1,
        );
        let verified = item(
            ms.milestone_id,
            None,
            "Verified",
            WorkItemStatus::VerifiedComplete,
            1,
        );

        let r = build(
            std::slice::from_ref(&ms),
            &[],
            std::slice::from_ref(&reported),
        );
        assert_eq!(r.overall.completion_ratio, 0.0);
        assert_eq!(r.overall.reported_complete_items, 1);
        assert_eq!(r.overall.verified_items, 0);

        let v = build(
            std::slice::from_ref(&ms),
            &[],
            std::slice::from_ref(&verified),
        );
        assert_eq!(v.overall.completion_ratio, 1.0);
        assert_eq!(v.overall.verified_items, 1);
    }

    #[test]
    fn deferred_is_excluded_from_denominator() {
        let ms = milestone(1, "M", MilestoneStatus::Active);
        let a = item(
            ms.milestone_id,
            None,
            "A",
            WorkItemStatus::VerifiedComplete,
            1,
        );
        let b = item(ms.milestone_id, None, "B", WorkItemStatus::Planned, 1);
        let c = item(ms.milestone_id, None, "C", WorkItemStatus::Deferred, 1);
        let r = build(&[ms], &[], &[a, b, c]);
        assert_eq!(r.overall.total_items, 2);
        assert_eq!(r.overall.deferred_items, 1);
        assert_eq!(r.overall.completion_ratio, 0.5);
    }

    #[test]
    fn weights_and_parent_aggregation() {
        let ms = milestone(1, "M", MilestoneStatus::Active);
        let ph = phase(ms.milestone_id, 1, "Phase");
        let items = vec![
            item(
                ms.milestone_id,
                Some(ph.phase_id),
                "1",
                WorkItemStatus::VerifiedComplete,
                1,
            ),
            item(
                ms.milestone_id,
                Some(ph.phase_id),
                "2",
                WorkItemStatus::VerifiedComplete,
                1,
            ),
            item(
                ms.milestone_id,
                Some(ph.phase_id),
                "3",
                WorkItemStatus::Planned,
                1,
            ),
            item(
                ms.milestone_id,
                Some(ph.phase_id),
                "Heavy",
                WorkItemStatus::Planned,
                3,
            ),
        ];
        // 2 complete of (1+1+1+3) = 2/6 = 1/3.
        let r = build(std::slice::from_ref(&ms), std::slice::from_ref(&ph), &items);
        assert!((r.overall.completion_ratio - 1.0 / 3.0).abs() < 1e-9);
        assert_eq!(r.milestones.len(), 1);
        assert_eq!(r.milestones[0].phases.len(), 1);
        assert!((r.milestones[0].summary.completion_ratio - 1.0 / 3.0).abs() < 1e-9);

        // With equal weights the same two-of-three is 2/3.
        let ms2 = milestone(1, "M2", MilestoneStatus::Active);
        let ph2 = phase(ms2.milestone_id, 1, "Phase2");
        let equal = vec![
            item(
                ms2.milestone_id,
                Some(ph2.phase_id),
                "1",
                WorkItemStatus::VerifiedComplete,
                1,
            ),
            item(
                ms2.milestone_id,
                Some(ph2.phase_id),
                "2",
                WorkItemStatus::VerifiedComplete,
                1,
            ),
            item(
                ms2.milestone_id,
                Some(ph2.phase_id),
                "3",
                WorkItemStatus::Planned,
                1,
            ),
        ];
        let r = build(
            std::slice::from_ref(&ms2),
            std::slice::from_ref(&ph2),
            &equal,
        );
        assert!((r.overall.completion_ratio - 2.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn completed_milestone_is_one_hundred_percent() {
        let done = milestone(1, "Done", MilestoneStatus::Completed);
        let active = milestone(2, "Active", MilestoneStatus::Active);
        let w = item(active.milestone_id, None, "W", WorkItemStatus::Planned, 1);
        let r = build(&[done, active], &[], &[w]);
        // (1.0 + 0.0) / 2 milestones = 0.5.
        assert_eq!(r.overall.completion_ratio, 0.5);
    }

    #[test]
    fn build_is_deterministic() {
        let ms = milestone(1, "M", MilestoneStatus::Active);
        let ph = phase(ms.milestone_id, 1, "P");
        let items = vec![
            item(
                ms.milestone_id,
                Some(ph.phase_id),
                "a",
                WorkItemStatus::VerifiedComplete,
                1,
            ),
            item(
                ms.milestone_id,
                Some(ph.phase_id),
                "b",
                WorkItemStatus::Planned,
                1,
            ),
        ];
        let first = build(std::slice::from_ref(&ms), std::slice::from_ref(&ph), &items);
        let second = build(std::slice::from_ref(&ms), std::slice::from_ref(&ph), &items);
        assert_eq!(
            serde_json::to_value(&first).unwrap(),
            serde_json::to_value(&second).unwrap()
        );
    }

    #[test]
    fn dependencies_gate_readiness() {
        let ms = milestone(1, "M", MilestoneStatus::Active);
        let a = item(ms.milestone_id, None, "A", WorkItemStatus::Planned, 1);
        let b = item(ms.milestone_id, None, "B", WorkItemStatus::Planned, 1);
        let dep = WorkItemDependencyRecord {
            work_item_id: b.work_item_id,
            depends_on_id: a.work_item_id,
            created_at: Utc::now(),
        };
        let r = ProgressEngine::build(
            Uuid::nil(),
            std::slice::from_ref(&ms),
            &[],
            &[a.clone(), b.clone()],
            std::slice::from_ref(&dep),
            &[],
            &[],
            &[],
        );
        let titles: Vec<&str> = r.next_candidates.iter().map(|w| w.title.as_str()).collect();
        assert_eq!(titles, vec!["A"]);
    }
}
