//! Multi-project orchestration.
//!
//! Provides project registry, quota/policy, conflict detection, and
//! cross-project dispatch planning.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::task_types::Priority;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectState {
    Active,
    Paused,
    Archived,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictPolicy {
    PreferHigherPriority,
    PreferEarlierCreated,
    Manual,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectQuota {
    pub max_workers: usize,
    pub max_concurrent_tasks: usize,
    pub max_daily_tokens: u64,
}

impl Default for ProjectQuota {
    fn default() -> Self {
        Self {
            max_workers: 2,
            max_concurrent_tasks: 4,
            max_daily_tokens: 1_000_000,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectProfile {
    pub id: Uuid,
    pub name: String,
    pub state: ProjectState,
    pub priority: Priority,
    pub quota: ProjectQuota,
    pub workspace_root: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ProjectProfile {
    pub fn new(
        name: impl Into<String>,
        priority: Priority,
        workspace_root: impl Into<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            state: ProjectState::Active,
            priority,
            quota: ProjectQuota::default(),
            workspace_root: workspace_root.into(),
            created_at: now,
            updated_at: now,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ProjectRegistry {
    projects: HashMap<Uuid, ProjectProfile>,
}

impl ProjectRegistry {
    pub fn new() -> Self {
        Self {
            projects: HashMap::new(),
        }
    }

    pub fn add(&mut self, project: ProjectProfile) {
        self.projects.insert(project.id, project);
    }

    pub fn get(&self, project_id: Uuid) -> Option<&ProjectProfile> {
        self.projects.get(&project_id)
    }

    pub fn get_mut(&mut self, project_id: Uuid) -> Option<&mut ProjectProfile> {
        self.projects.get_mut(&project_id)
    }

    pub fn pause(&mut self, project_id: Uuid) -> bool {
        if let Some(p) = self.projects.get_mut(&project_id) {
            p.state = ProjectState::Paused;
            p.updated_at = Utc::now();
            true
        } else {
            false
        }
    }

    pub fn resume(&mut self, project_id: Uuid) -> bool {
        if let Some(p) = self.projects.get_mut(&project_id) {
            p.state = ProjectState::Active;
            p.updated_at = Utc::now();
            true
        } else {
            false
        }
    }

    pub fn active_projects(&self) -> Vec<&ProjectProfile> {
        self.projects
            .values()
            .filter(|p| p.state == ProjectState::Active)
            .collect()
    }

    pub fn all(&self) -> Vec<&ProjectProfile> {
        self.projects.values().collect()
    }
}

impl Default for ProjectRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug)]
pub struct ProjectLoad {
    pub project_id: Uuid,
    pub queued_tasks: usize,
    pub running_tasks: usize,
    pub allocated_workers: usize,
}

#[derive(Clone, Debug)]
pub struct DispatchDecision {
    pub project_id: Uuid,
    pub grant_workers: usize,
}

pub struct CrossProjectArbiter {
    pub policy: ConflictPolicy,
}

impl CrossProjectArbiter {
    pub fn new(policy: ConflictPolicy) -> Self {
        Self { policy }
    }

    pub fn plan_dispatch(
        &self,
        registry: &ProjectRegistry,
        loads: &[ProjectLoad],
        total_available_workers: usize,
    ) -> Vec<DispatchDecision> {
        let mut active: Vec<(&ProjectProfile, &ProjectLoad)> = loads
            .iter()
            .filter_map(|l| registry.get(l.project_id).map(|p| (p, l)))
            .filter(|(p, _)| p.state == ProjectState::Active)
            .collect();

        active.sort_by(|(a, _), (b, _)| {
            // higher priority first; fallback by created_at (older first)
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.created_at.cmp(&b.created_at))
        });

        let mut remaining = total_available_workers;
        let mut out = Vec::new();

        for (project, load) in active {
            if remaining == 0 {
                break;
            }

            let demand = load
                .queued_tasks
                .saturating_sub(load.running_tasks)
                .max(0usize);
            let quota_room = project
                .quota
                .max_workers
                .saturating_sub(load.allocated_workers);
            let grant = demand.min(quota_room).min(remaining);

            if grant > 0 {
                out.push(DispatchDecision {
                    project_id: project.id,
                    grant_workers: grant,
                });
                remaining = remaining.saturating_sub(grant);
            }
        }

        out
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictKind {
    WorkspaceOverlap,
    SharedPath,
    EnvKeyCollision,
}

#[derive(Clone, Debug)]
pub struct ConflictRecord {
    pub kind: ConflictKind,
    pub left_project: Uuid,
    pub right_project: Uuid,
    pub detail: String,
}

pub struct ConflictDetector;

impl ConflictDetector {
    pub fn detect_workspace_overlap(projects: &[ProjectProfile]) -> Vec<ConflictRecord> {
        let mut out = Vec::new();
        for i in 0..projects.len() {
            for j in (i + 1)..projects.len() {
                let a = &projects[i];
                let b = &projects[j];
                if a.workspace_root == b.workspace_root
                    || a.workspace_root.starts_with(&b.workspace_root)
                    || b.workspace_root.starts_with(&a.workspace_root)
                {
                    out.push(ConflictRecord {
                        kind: ConflictKind::WorkspaceOverlap,
                        left_project: a.id,
                        right_project: b.id,
                        detail: format!("{} <-> {}", a.workspace_root, b.workspace_root),
                    });
                }
            }
        }
        out
    }

    pub fn detect_shared_paths(
        project_id: Uuid,
        touched_paths: &[String],
        others: &[(Uuid, Vec<String>)],
    ) -> Vec<ConflictRecord> {
        let lhs: HashSet<&str> = touched_paths.iter().map(|s| s.as_str()).collect();
        let mut out = Vec::new();

        for (other_id, paths) in others {
            for p in paths {
                if lhs.contains(p.as_str()) {
                    out.push(ConflictRecord {
                        kind: ConflictKind::SharedPath,
                        left_project: project_id,
                        right_project: *other_id,
                        detail: p.clone(),
                    });
                }
            }
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_lifecycle() {
        let mut reg = ProjectRegistry::new();
        let p = ProjectProfile::new("p1", Priority::High, "/tmp/p1");
        let id = p.id;
        reg.add(p);
        assert_eq!(reg.active_projects().len(), 1);
        assert!(reg.pause(id));
        assert_eq!(reg.active_projects().len(), 0);
        assert!(reg.resume(id));
        assert_eq!(reg.active_projects().len(), 1);
    }

    #[test]
    fn test_dispatch_respects_priority_and_quota() {
        let mut reg = ProjectRegistry::new();
        let mut a = ProjectProfile::new("a", Priority::High, "/tmp/a");
        a.quota.max_workers = 1;
        let b = ProjectProfile::new("b", Priority::Normal, "/tmp/b");
        let a_id = a.id;
        let b_id = b.id;
        reg.add(a);
        reg.add(b);

        let arbiter = CrossProjectArbiter::new(ConflictPolicy::PreferHigherPriority);
        let plan = arbiter.plan_dispatch(
            &reg,
            &[
                ProjectLoad {
                    project_id: a_id,
                    queued_tasks: 5,
                    running_tasks: 0,
                    allocated_workers: 0,
                },
                ProjectLoad {
                    project_id: b_id,
                    queued_tasks: 5,
                    running_tasks: 0,
                    allocated_workers: 0,
                },
            ],
            2,
        );

        assert!(!plan.is_empty());
        let a_grant = plan
            .iter()
            .find(|d| d.project_id == a_id)
            .map(|d| d.grant_workers)
            .unwrap_or(0);
        assert_eq!(a_grant, 1);
    }

    #[test]
    fn test_workspace_conflict_detection() {
        let a = ProjectProfile::new("a", Priority::Normal, "/repo");
        let b = ProjectProfile::new("b", Priority::Normal, "/repo/sub");
        let conflicts = ConflictDetector::detect_workspace_overlap(&[a, b]);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].kind, ConflictKind::WorkspaceOverlap);
    }

    #[test]
    fn test_shared_path_conflict_detection() {
        let pid = Uuid::new_v4();
        let other = Uuid::new_v4();
        let conflicts = ConflictDetector::detect_shared_paths(
            pid,
            &["src/main.rs".to_string(), "Cargo.toml".to_string()],
            &[(
                other,
                vec!["README.md".to_string(), "src/main.rs".to_string()],
            )],
        );
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].kind, ConflictKind::SharedPath);
    }
}
