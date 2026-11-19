//! Self-evolution engine (P12 skeleton).
//!
//! Safe pipeline for Argus modifying its own codebase.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::types::{ArgusError, ArgusResult};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvolutionCategory {
    Feature,
    BugFix,
    Optimization,
    Skill,
    McpIntegration,
    Infrastructure,
    DependencyUpdate,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvolutionStatus {
    Requested,
    SafetyChecking,
    SafetyApproved,
    SafetyRejected(String),
    BranchCreated,
    InDevelopment,
    Testing,
    TestsPassed,
    TestsFailed(String),
    AwaitingApproval,
    Approved,
    Rejected(String),
    Deploying,
    Deployed,
    Completed,
    Failed(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChangeType {
    Added,
    Modified,
    Deleted,
    Renamed { from: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    pub change_type: ChangeType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvolutionRequest {
    pub id: Uuid,
    pub description: String,
    pub category: EvolutionCategory,
    pub requested_at: DateTime<Utc>,
    pub status: EvolutionStatus,
    pub branch_name: String,
    pub changes: Vec<FileChange>,
}

impl EvolutionRequest {
    pub fn new(description: impl Into<String>, category: EvolutionCategory) -> Self {
        Self {
            id: Uuid::new_v4(),
            description: description.into(),
            category,
            requested_at: Utc::now(),
            status: EvolutionStatus::Requested,
            branch_name: String::new(),
            changes: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SafetyCheckResult {
    pub safe: bool,
    pub reason: String,
    pub violations: Vec<String>,
}

pub struct GuardrailEnforcer {
    protected_files: Vec<String>,
    max_files_per_evolution: usize,
}

pub trait EvolutionSecurityHooks: Send + Sync {
    fn authorize_request(&self, _request: &EvolutionRequest) -> ArgusResult<()> {
        Ok(())
    }

    fn validate_changes(&self, _request: &EvolutionRequest) -> ArgusResult<()> {
        Ok(())
    }

    fn audit_transition(
        &self,
        _request: &EvolutionRequest,
        _status: &EvolutionStatus,
    ) -> ArgusResult<()> {
        Ok(())
    }
}

#[derive(Default)]
pub struct NoopEvolutionSecurityHooks;

impl EvolutionSecurityHooks for NoopEvolutionSecurityHooks {}

impl GuardrailEnforcer {
    pub fn new() -> Self {
        Self {
            protected_files: vec![
                "crates/argus-security/src/key_vault.rs".to_string(),
                "crates/argus-security/src/guardrails.rs".to_string(),
                "crates/argus-security/src/audit.rs".to_string(),
                "crates/argus-security/src/rbac.rs".to_string(),
            ],
            max_files_per_evolution: 50,
        }
    }

    pub fn check_request(&self, request: &EvolutionRequest) -> SafetyCheckResult {
        let mut violations = Vec::new();

        if request.changes.len() > self.max_files_per_evolution {
            violations.push(format!(
                "too many files changed: {} > {}",
                request.changes.len(),
                self.max_files_per_evolution
            ));
        }

        for c in &request.changes {
            if self.protected_files.contains(&c.path) {
                violations.push(format!("protected file modified: {}", c.path));
            }
        }

        let safe = violations.is_empty();
        SafetyCheckResult {
            safe,
            reason: if safe {
                "all guardrails passed".to_string()
            } else {
                violations.join("; ")
            },
            violations,
        }
    }
}

impl Default for GuardrailEnforcer {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
#[allow(clippy::ptr_arg)]
pub trait EvolutionGit: Send + Sync {
    async fn create_branch(&self, source_dir: &PathBuf, branch: &str) -> ArgusResult<()>;
    async fn get_changes(&self, source_dir: &PathBuf, branch: &str)
    -> ArgusResult<Vec<FileChange>>;
    async fn merge_branch(
        &self,
        source_dir: &PathBuf,
        branch: &str,
        target: &str,
    ) -> ArgusResult<()>;
}

#[async_trait]
#[allow(clippy::ptr_arg)]
pub trait EvolutionTester: Send + Sync {
    async fn run_full_suite(&self, source_dir: &PathBuf, branch: &str) -> ArgusResult<TestSummary>;
}

#[async_trait]
#[allow(clippy::ptr_arg)]
pub trait EvolutionDeployer: Send + Sync {
    async fn deploy(&self, source_dir: &PathBuf, branch: &str) -> ArgusResult<()>;
}

#[derive(Debug, Clone)]
pub struct TestSummary {
    pub all_passed: bool,
    pub failure_summary: String,
}

#[derive(Default)]
pub struct EvolutionHistory {
    entries: Arc<RwLock<Vec<EvolutionRequest>>>,
}

impl EvolutionHistory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&self, req: EvolutionRequest) -> ArgusResult<()> {
        let mut guard = self
            .entries
            .write()
            .map_err(|e| ArgusError::Evolution(format!("history lock poisoned: {e}")))?;
        guard.push(req);
        Ok(())
    }

    pub fn recent(&self, limit: usize) -> ArgusResult<Vec<EvolutionRequest>> {
        let guard = self
            .entries
            .read()
            .map_err(|e| ArgusError::Evolution(format!("history lock poisoned: {e}")))?;
        Ok(guard.iter().rev().take(limit).cloned().collect())
    }
}

pub struct EvolutionEngine<G, T, D>
where
    G: EvolutionGit,
    T: EvolutionTester,
    D: EvolutionDeployer,
{
    pub source_dir: PathBuf,
    pub git: G,
    pub tester: T,
    pub deployer: D,
    pub guardrails: GuardrailEnforcer,
    pub security_hooks: Arc<dyn EvolutionSecurityHooks>,
    pub history: EvolutionHistory,
}

impl<G, T, D> EvolutionEngine<G, T, D>
where
    G: EvolutionGit,
    T: EvolutionTester,
    D: EvolutionDeployer,
{
    pub fn new(source_dir: impl Into<PathBuf>, git: G, tester: T, deployer: D) -> Self {
        Self {
            source_dir: source_dir.into(),
            git,
            tester,
            deployer,
            guardrails: GuardrailEnforcer::new(),
            security_hooks: Arc::new(NoopEvolutionSecurityHooks),
            history: EvolutionHistory::new(),
        }
    }

    pub fn with_security_hooks(mut self, hooks: Arc<dyn EvolutionSecurityHooks>) -> Self {
        self.security_hooks = hooks;
        self
    }

    fn transition(
        &self,
        request: &mut EvolutionRequest,
        status: EvolutionStatus,
    ) -> ArgusResult<()> {
        request.status = status;
        self.security_hooks
            .audit_transition(request, &request.status)
    }

    pub async fn evolve(&self, request: &mut EvolutionRequest) -> ArgusResult<()> {
        self.transition(request, EvolutionStatus::SafetyChecking)?;

        if let Err(err) = self.security_hooks.authorize_request(request) {
            request.status = EvolutionStatus::SafetyRejected(format!("{err}"));
            self.security_hooks
                .audit_transition(request, &request.status)?;
            self.history.record(request.clone())?;
            return Err(err);
        }

        let safety = self.guardrails.check_request(request);
        if !safety.safe {
            request.status = EvolutionStatus::SafetyRejected(safety.reason.clone());
            self.security_hooks
                .audit_transition(request, &request.status)?;
            self.history.record(request.clone())?;
            return Err(ArgusError::Evolution(safety.reason));
        }
        self.transition(request, EvolutionStatus::SafetyApproved)?;

        // branch
        let branch = format!("argus/evolution/{}", slugify(&request.description));
        request.branch_name = branch.clone();
        self.git.create_branch(&self.source_dir, &branch).await?;
        self.transition(request, EvolutionStatus::BranchCreated)?;

        // develop (placeholder: get changes from branch)
        self.transition(request, EvolutionStatus::InDevelopment)?;
        request.changes = self.git.get_changes(&self.source_dir, &branch).await?;

        if let Err(err) = self.security_hooks.validate_changes(request) {
            request.status = EvolutionStatus::SafetyRejected(format!("{err}"));
            self.security_hooks
                .audit_transition(request, &request.status)?;
            self.history.record(request.clone())?;
            return Err(err);
        }

        // test
        self.transition(request, EvolutionStatus::Testing)?;
        let tests = self
            .tester
            .run_full_suite(&self.source_dir, &branch)
            .await?;
        if !tests.all_passed {
            request.status = EvolutionStatus::TestsFailed(tests.failure_summary.clone());
            self.security_hooks
                .audit_transition(request, &request.status)?;
            self.history.record(request.clone())?;
            return Err(ArgusError::Evolution(tests.failure_summary));
        }
        self.transition(request, EvolutionStatus::TestsPassed)?;

        // simplified approval path (auto-approved in skeleton)
        self.transition(request, EvolutionStatus::Approved)?;

        self.transition(request, EvolutionStatus::Deploying)?;
        self.deployer.deploy(&self.source_dir, &branch).await?;
        self.transition(request, EvolutionStatus::Deployed)?;

        self.git
            .merge_branch(&self.source_dir, &branch, "main")
            .await?;
        self.transition(request, EvolutionStatus::Completed)?;

        self.history.record(request.clone())?;
        Ok(())
    }
}

fn slugify(input: &str) -> String {
    input
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    struct MockGit;
    #[async_trait]
    impl EvolutionGit for MockGit {
        async fn create_branch(&self, _source_dir: &PathBuf, _branch: &str) -> ArgusResult<()> {
            Ok(())
        }

        async fn get_changes(
            &self,
            _source_dir: &PathBuf,
            _branch: &str,
        ) -> ArgusResult<Vec<FileChange>> {
            Ok(vec![FileChange {
                path: "crates/argus-llm/src/providers/groq.rs".to_string(),
                change_type: ChangeType::Added,
            }])
        }

        async fn merge_branch(
            &self,
            _source_dir: &PathBuf,
            _branch: &str,
            _target: &str,
        ) -> ArgusResult<()> {
            Ok(())
        }
    }

    struct MockTester {
        ok: bool,
    }
    #[async_trait]
    impl EvolutionTester for MockTester {
        async fn run_full_suite(
            &self,
            _source_dir: &PathBuf,
            _branch: &str,
        ) -> ArgusResult<TestSummary> {
            Ok(TestSummary {
                all_passed: self.ok,
                failure_summary: if self.ok {
                    "".to_string()
                } else {
                    "tests failed".to_string()
                },
            })
        }
    }

    struct MockDeployer;
    #[async_trait]
    impl EvolutionDeployer for MockDeployer {
        async fn deploy(&self, _source_dir: &PathBuf, _branch: &str) -> ArgusResult<()> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct TrackingHooks {
        transitions: Mutex<Vec<String>>,
        deny_on_authorize: bool,
        deny_on_changes: bool,
    }

    impl EvolutionSecurityHooks for TrackingHooks {
        fn authorize_request(&self, _request: &EvolutionRequest) -> ArgusResult<()> {
            if self.deny_on_authorize {
                return Err(ArgusError::PermissionDenied(
                    "authorization denied".to_string(),
                ));
            }
            Ok(())
        }

        fn validate_changes(&self, _request: &EvolutionRequest) -> ArgusResult<()> {
            if self.deny_on_changes {
                return Err(ArgusError::Security("change validation denied".to_string()));
            }
            Ok(())
        }

        fn audit_transition(
            &self,
            _request: &EvolutionRequest,
            status: &EvolutionStatus,
        ) -> ArgusResult<()> {
            let mut guard = self
                .transitions
                .lock()
                .map_err(|e| ArgusError::Evolution(format!("test lock poisoned: {e}")))?;
            guard.push(format!("{status:?}"));
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_evolution_success() {
        let hooks = Arc::new(TrackingHooks::default());
        let engine =
            EvolutionEngine::new("/tmp/argus", MockGit, MockTester { ok: true }, MockDeployer)
                .with_security_hooks(hooks.clone());
        let mut req = EvolutionRequest::new("Add groq provider", EvolutionCategory::Feature);

        engine.evolve(&mut req).await.unwrap();
        assert!(matches!(req.status, EvolutionStatus::Completed));

        let transitions = hooks.transitions.lock().unwrap();
        assert!(transitions.iter().any(|s| s == "SafetyChecking"));
        assert!(transitions.iter().any(|s| s == "Completed"));
    }

    #[tokio::test]
    async fn test_evolution_guardrail_reject() {
        let engine =
            EvolutionEngine::new("/tmp/argus", MockGit, MockTester { ok: true }, MockDeployer);
        let mut req = EvolutionRequest::new("Weaken encryption", EvolutionCategory::Infrastructure);
        req.changes.push(FileChange {
            path: "crates/argus-security/src/key_vault.rs".to_string(),
            change_type: ChangeType::Modified,
        });

        let err = engine.evolve(&mut req).await.unwrap_err();
        assert!(matches!(req.status, EvolutionStatus::SafetyRejected(_)));
        assert!(format!("{err}").contains("protected file"));
    }

    #[tokio::test]
    async fn test_evolution_test_failure() {
        let engine = EvolutionEngine::new(
            "/tmp/argus",
            MockGit,
            MockTester { ok: false },
            MockDeployer,
        );
        let mut req = EvolutionRequest::new("Refactor scheduler", EvolutionCategory::Optimization);

        let err = engine.evolve(&mut req).await.unwrap_err();
        assert!(matches!(req.status, EvolutionStatus::TestsFailed(_)));
        assert!(format!("{err}").contains("tests failed"));
    }

    #[tokio::test]
    async fn test_evolution_authorization_denied_by_hook() {
        let hooks = Arc::new(TrackingHooks {
            deny_on_authorize: true,
            ..TrackingHooks::default()
        });
        let engine =
            EvolutionEngine::new("/tmp/argus", MockGit, MockTester { ok: true }, MockDeployer)
                .with_security_hooks(hooks.clone());
        let mut req = EvolutionRequest::new("Sensitive change", EvolutionCategory::Infrastructure);

        let err = engine.evolve(&mut req).await.unwrap_err();
        assert!(matches!(req.status, EvolutionStatus::SafetyRejected(_)));
        assert!(format!("{err}").contains("authorization denied"));
    }

    #[tokio::test]
    async fn test_evolution_change_validation_denied_by_hook() {
        let hooks = Arc::new(TrackingHooks {
            deny_on_changes: true,
            ..TrackingHooks::default()
        });
        let engine =
            EvolutionEngine::new("/tmp/argus", MockGit, MockTester { ok: true }, MockDeployer)
                .with_security_hooks(hooks);
        let mut req = EvolutionRequest::new("Sensitive change", EvolutionCategory::Infrastructure);

        let err = engine.evolve(&mut req).await.unwrap_err();
        assert!(matches!(req.status, EvolutionStatus::SafetyRejected(_)));
        assert!(format!("{err}").contains("change validation denied"));
    }

    #[test]
    fn test_slugify() {
        assert_eq!(slugify("Add Groq Provider"), "add-groq-provider");
    }

    #[test]
    fn test_guardrail_max_files() {
        let g = GuardrailEnforcer::new();
        let mut req = EvolutionRequest::new("bulk", EvolutionCategory::Feature);
        for i in 0..51 {
            req.changes.push(FileChange {
                path: format!("f{i}.rs"),
                change_type: ChangeType::Modified,
            });
        }
        let r = g.check_request(&req);
        assert!(!r.safe);
        assert!(!r.violations.is_empty());
    }
}
