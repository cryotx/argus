# Self-Evolution — Detailed Design

Argus can modify its own source code. When the owner requests a new feature, fix, or improvement to Argus itself, it uses its own worker pool to develop, test, and deploy changes to its own codebase. This is the most dangerous and most powerful capability — it requires rigorous safety guardrails.

## 1. Architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│                    Self-Evolution Engine                               │
│                                                                       │
│  ┌──────────────┐  ┌──────────────┐  ┌────────────────────────────┐ │
│  │  Evolution    │  │  Isolated    │  │  Test                      │ │
│  │  Request      │→│  Branch      │→│  Runner                    │ │
│  │  Parser       │  │  Manager     │  │  (full suite)              │ │
│  │               │  │              │  │                            │ │
│  └──────────────┘  └──────────────┘  └────────────┬───────────────┘ │
│                                                     │                 │
│                                                     ▼                 │
│  ┌──────────────┐  ┌──────────────┐  ┌────────────────────────────┐ │
│  │  Rollback     │  │  Blue-Green  │  │  Test                      │ │
│  │  Safety Net   │←│  Deployer    │←│  Instance                  │ │
│  │               │  │              │  │  Verifier                  │ │
│  └──────────────┘  └──────────────┘  └────────────────────────────┘ │
│                                                                       │
│  ┌──────────────┐  ┌──────────────┐  ┌────────────────────────────┐ │
│  │  Conflict     │  │  Guardrail   │  │  Evolution                 │ │
│  │  Detector     │  │  Enforcer    │  │  History                   │ │
│  │               │  │              │  │                            │ │
│  └──────────────┘  └──────────────┘  └────────────────────────────┘ │
└──────────────────────────────────────────────────────────────────────┘
```

## 2. Self-Evolution Pipeline

### 2.1 Pipeline Overview

```
Owner Request
    │
    ▼
┌─────────────────┐
│ 1. Parse Request │  ← "添加支持 Groq 提供商"
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ 2. Safety Check  │  ← Can this be safely self-modified?
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ 3. Create Branch │  ← argus/evolution/add-groq-provider
│    (isolated)    │
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ 4. Worker        │  ← Worker modifies Argus source code
│    Development   │     in the isolated branch
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ 5. Automated     │  ← cargo test, cargo clippy, cargo build
│    Testing       │
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ 6. Test Instance │  ← Build and run modified Argus
│    Verification  │     alongside current instance
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ 7. Owner         │  ← Show diff, test results, ask approval
│    Approval      │
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ 8. Deploy        │  ← Blue-green deploy or graceful restart
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ 9. Health Check  │  ← Verify new instance is working
│    & Monitor     │
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ 10. Merge &      │  ← Merge evolution branch → main
│     Cleanup      │     Delete test instance
└─────────────────┘
```

### 2.2 Evolution Request

```rust
/// A request to modify Argus itself
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvolutionRequest {
    pub id: Uuid,
    pub description: String,
    pub category: EvolutionCategory,
    pub requested_at: DateTime<Utc>,
    pub status: EvolutionStatus,
    pub branch_name: String,
    pub changes: Vec<FileChange>,
    pub test_results: Option<TestResults>,
    pub approval: Option<ApprovalRecord>,
    pub deployment: Option<EvolutionDeployment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EvolutionCategory {
    /// New feature for Argus
    Feature,
    /// Bug fix in Argus
    BugFix,
    /// Performance optimization
    Optimization,
    /// New skill (doesn't require core restart)
    Skill,
    /// New MCP server integration
    McpIntegration,
    /// Configuration/infrastructure change
    Infrastructure,
    /// Dependency update
    DependencyUpdate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    TestInstanceRunning,
    AwaitingApproval,
    Approved,
    Rejected(String),
    Deploying,
    Deployed,
    HealthCheckPassed,
    HealthCheckFailed(String),
    RolledBack(String),
    Completed,
    Failed(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    pub change_type: ChangeType,
    pub diff: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ChangeType {
    Added,
    Modified,
    Deleted,
    Renamed { from: String },
}
```

### 2.3 Evolution Engine

```rust
pub struct EvolutionEngine {
    /// Argus's own source code directory
    source_dir: PathBuf,
    /// Worker manager for assigning development tasks
    worker_manager: WorkerManager,
    /// Git operations
    git: GitOps,
    /// Test runner
    test_runner: TestRunner,
    /// Build system
    builder: ArgusBuilder,
    /// Deployer for self
    self_deployer: SelfDeployer,
    /// Guardrail enforcer
    guardrails: GuardrailEnforcer,
    /// Event bus
    event_bus: EventBusTx,
    /// Database
    db: PgPool,
}

impl EvolutionEngine {
    /// Execute the full self-evolution pipeline
    pub async fn evolve(&self, request: &mut EvolutionRequest) -> Result<()> {
        self.event_bus.send(ArgusEvent::EvolutionRequested);

        // Step 1: Safety check
        request.status = EvolutionStatus::SafetyChecking;
        let safety = self.guardrails.check_request(request).await?;
        if !safety.safe {
            request.status = EvolutionStatus::SafetyRejected(safety.reason.clone());
            return Err(anyhow!("Safety check failed: {}", safety.reason));
        }
        request.status = EvolutionStatus::SafetyApproved;

        // Step 2: Create isolated branch
        let branch = format!("argus/evolution/{}", self.slugify(&request.description));
        request.branch_name = branch.clone();
        self.git.create_branch(&self.source_dir, &branch).await?;
        request.status = EvolutionStatus::BranchCreated;

        // Step 3: Assign worker to develop the change
        request.status = EvolutionStatus::InDevelopment;
        let task = self.create_development_task(request).await?;
        let result = self.worker_manager
            .assign_and_wait(task)
            .await?;

        if !result.success {
            request.status = EvolutionStatus::Failed(
                "Worker failed to implement changes".into()
            );
            self.cleanup_branch(&branch).await?;
            return Err(anyhow!("Development failed"));
        }

        // Collect file changes
        request.changes = self.git
            .get_changes(&self.source_dir, &branch)
            .await?;

        // Step 4: Run tests
        request.status = EvolutionStatus::Testing;
        let test_results = self.test_runner
            .run_full_suite(&self.source_dir, &branch)
            .await?;
        request.test_results = Some(test_results.clone());

        if !test_results.all_passed() {
            request.status = EvolutionStatus::TestsFailed(
                test_results.failure_summary()
            );
            // Don't cleanup branch — might want to iterate
            return Err(anyhow!("Tests failed: {}", test_results.failure_summary()));
        }
        request.status = EvolutionStatus::TestsPassed;

        // Step 5: Build and launch test instance
        request.status = EvolutionStatus::TestInstanceRunning;
        let test_instance = self.builder
            .build_test_instance(&self.source_dir, &branch)
            .await?;
        let verification = self.verify_test_instance(&test_instance).await?;

        if !verification.healthy {
            request.status = EvolutionStatus::HealthCheckFailed(
                verification.reason.clone()
            );
            self.builder.stop_test_instance(&test_instance).await?;
            return Err(anyhow!("Test instance unhealthy: {}", verification.reason));
        }
        self.builder.stop_test_instance(&test_instance).await?;

        // Step 6: Request owner approval
        request.status = EvolutionStatus::AwaitingApproval;
        let approval = self.request_owner_approval(request).await?;
        request.approval = Some(approval.clone());

        if !approval.approved {
            request.status = EvolutionStatus::Rejected(
                approval.reason.unwrap_or_default()
            );
            self.cleanup_branch(&branch).await?;
            return Ok(());
        }
        request.status = EvolutionStatus::Approved;

        // Step 7: Deploy
        request.status = EvolutionStatus::Deploying;
        let deployment = self.deploy_evolution(request).await?;
        request.deployment = Some(deployment);

        // Step 8: Health check after deploy
        let health = self.post_deploy_health_check().await?;
        if !health.healthy {
            request.status = EvolutionStatus::HealthCheckFailed(
                health.reason.clone()
            );
            // Auto-rollback
            self.self_deployer.rollback().await?;
            request.status = EvolutionStatus::RolledBack(health.reason);
            self.event_bus.send(ArgusEvent::EvolutionRolledBack);
            return Err(anyhow!("Post-deploy health check failed, rolled back"));
        }
        request.status = EvolutionStatus::HealthCheckPassed;

        // Step 9: Merge and cleanup
        self.git.merge_branch(
            &self.source_dir,
            &branch,
            "main",
        ).await?;
        self.cleanup_branch(&branch).await?;

        request.status = EvolutionStatus::Completed;
        self.event_bus.send(ArgusEvent::EvolutionDeployed);

        // Record in history
        self.record_evolution(request).await?;

        Ok(())
    }
}
```

## 3. Test Runner

Comprehensive testing before any self-modification is deployed:

```rust
pub struct TestRunner;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResults {
    pub unit_tests: TestSuiteResult,
    pub integration_tests: TestSuiteResult,
    pub clippy: LintResult,
    pub build: BuildResult,
    pub format_check: FormatResult,
    pub total_duration: Duration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestSuiteResult {
    pub passed: u32,
    pub failed: u32,
    pub skipped: u32,
    pub failures: Vec<TestFailure>,
    pub duration: Duration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestFailure {
    pub test_name: String,
    pub error: String,
    pub stdout: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LintResult {
    pub warnings: u32,
    pub errors: u32,
    pub details: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildResult {
    pub success: bool,
    pub error: Option<String>,
    pub binary_size: Option<u64>,
    pub duration: Duration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormatResult {
    pub formatted: bool,
    pub unformatted_files: Vec<String>,
}

impl TestResults {
    pub fn all_passed(&self) -> bool {
        self.unit_tests.failed == 0
            && self.integration_tests.failed == 0
            && self.clippy.errors == 0
            && self.build.success
    }

    pub fn failure_summary(&self) -> String {
        let mut parts = vec![];
        if self.unit_tests.failed > 0 {
            parts.push(format!("{} unit tests failed", self.unit_tests.failed));
        }
        if self.integration_tests.failed > 0 {
            parts.push(format!("{} integration tests failed", self.integration_tests.failed));
        }
        if self.clippy.errors > 0 {
            parts.push(format!("{} clippy errors", self.clippy.errors));
        }
        if !self.build.success {
            parts.push("build failed".into());
        }
        parts.join(", ")
    }
}

impl TestRunner {
    /// Run the complete test suite on a branch
    pub async fn run_full_suite(
        &self,
        source_dir: &Path,
        branch: &str,
    ) -> Result<TestResults> {
        // Checkout the branch
        let workdir = source_dir.to_path_buf();

        // Run all checks
        let start = Instant::now();

        // 1. cargo fmt --check
        let format_check = self.run_format_check(&workdir).await?;

        // 2. cargo clippy
        let clippy = self.run_clippy(&workdir).await?;

        // 3. cargo build --release
        let build = self.run_build(&workdir).await?;
        if !build.success {
            return Ok(TestResults {
                unit_tests: TestSuiteResult::empty(),
                integration_tests: TestSuiteResult::empty(),
                clippy,
                build,
                format_check,
                total_duration: start.elapsed(),
            });
        }

        // 4. cargo test
        let unit_tests = self.run_tests(&workdir, "unit").await?;

        // 5. cargo test --test integration
        let integration_tests = self.run_tests(&workdir, "integration").await?;

        Ok(TestResults {
            unit_tests,
            integration_tests,
            clippy,
            build,
            format_check,
            total_duration: start.elapsed(),
        })
    }

    async fn run_build(&self, workdir: &Path) -> Result<BuildResult> {
        let start = Instant::now();
        let output = tokio::process::Command::new("cargo")
            .args(["build", "--release"])
            .current_dir(workdir)
            .output()
            .await?;

        let binary_size = if output.status.success() {
            // Get binary size
            let binary_path = workdir.join("target/release/argus");
            tokio::fs::metadata(&binary_path)
                .await
                .ok()
                .map(|m| m.len())
        } else {
            None
        };

        Ok(BuildResult {
            success: output.status.success(),
            error: if output.status.success() {
                None
            } else {
                Some(String::from_utf8_lossy(&output.stderr).to_string())
            },
            binary_size,
            duration: start.elapsed(),
        })
    }
}
```

## 4. Change Classification

Different types of changes require different deployment strategies:

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum ChangeImpact {
    /// Changes to Skills or MCP integrations — can be hot-reloaded
    HotReloadable,
    /// Changes to configuration or non-critical modules — graceful restart
    GracefulRestart,
    /// Changes to core modules (brain, scheduler, memory) — blue-green deploy
    CoreChange,
    /// Database schema changes — migration + blue-green deploy
    SchemaMigration,
}

pub struct ChangeClassifier;

impl ChangeClassifier {
    /// Classify the impact of changed files
    pub fn classify(&self, changes: &[FileChange]) -> ChangeImpact {
        let mut impact = ChangeImpact::HotReloadable;

        for change in changes {
            let file_impact = self.classify_file(&change.path);
            impact = self.max_impact(impact, file_impact);
        }

        impact
    }

    fn classify_file(&self, path: &str) -> ChangeImpact {
        if path.starts_with("crates/argus-skills/")
            || path.starts_with("crates/argus-mcp/src/servers/")
        {
            ChangeImpact::HotReloadable
        } else if path.contains("migrations/")
            || path.contains("schema")
        {
            ChangeImpact::SchemaMigration
        } else if path.starts_with("crates/argus-core/")
            || path.starts_with("crates/argus-memory/")
            || path.starts_with("crates/argus-llm/")
            || path.starts_with("crates/argus-worker/")
        {
            ChangeImpact::CoreChange
        } else {
            ChangeImpact::GracefulRestart
        }
    }

    fn max_impact(&self, a: ChangeImpact, b: ChangeImpact) -> ChangeImpact {
        let order = |i: &ChangeImpact| match i {
            ChangeImpact::HotReloadable => 0,
            ChangeImpact::GracefulRestart => 1,
            ChangeImpact::CoreChange => 2,
            ChangeImpact::SchemaMigration => 3,
        };
        if order(&a) >= order(&b) { a } else { b }
    }
}
```

## 5. Deployment Strategies

### 5.1 Hot Reload

For skills and MCP server changes — no downtime, no restart:

```rust
pub struct HotReloader {
    skill_registry: SkillRegistry,
    mcp_manager: McpManager,
}

impl HotReloader {
    /// Reload changed skills/MCP servers without restarting Argus
    pub async fn reload(&self, changes: &[FileChange]) -> Result<()> {
        for change in changes {
            if change.path.starts_with("crates/argus-skills/") {
                let skill_name = self.extract_skill_name(&change.path);
                tracing::info!("Hot-reloading skill: {}", skill_name);
                self.skill_registry.reload(&skill_name).await?;
            }

            if change.path.starts_with("crates/argus-mcp/src/servers/") {
                let server_name = self.extract_server_name(&change.path);
                tracing::info!("Hot-reloading MCP server: {}", server_name);
                self.mcp_manager.reload_server(&server_name).await?;
            }
        }
        Ok(())
    }
}
```

### 5.2 Graceful Restart

For non-critical module changes:

```rust
pub struct GracefulRestarter;

impl GracefulRestarter {
    /// Restart Argus gracefully
    /// - Finish all in-progress tasks (with timeout)
    /// - Save all state to database
    /// - Swap the binary
    /// - Restart
    pub async fn restart(
        &self,
        new_binary: &Path,
        timeout: Duration,
    ) -> Result<()> {
        tracing::info!("Beginning graceful restart...");

        // 1. Stop accepting new tasks
        // (Signal scheduler to pause)

        // 2. Wait for in-progress tasks to complete (with timeout)
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let active = self.count_active_tasks().await;
            if active == 0 {
                break;
            }
            tracing::info!("Waiting for {} active tasks to complete...", active);
            tokio::time::sleep(Duration::from_secs(5)).await;
        }

        // 3. Checkpoint all tasks that didn't finish
        self.checkpoint_remaining_tasks().await?;

        // 4. Flush all buffers (audit log, memory, etc.)
        self.flush_all_buffers().await?;

        // 5. Replace binary
        let current_binary = std::env::current_exe()?;
        let backup = current_binary.with_extension("bak");
        tokio::fs::rename(&current_binary, &backup).await?;
        tokio::fs::copy(new_binary, &current_binary).await?;

        // 6. Exec into new binary (Unix only)
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            let err = std::process::Command::new(&current_binary)
                .args(std::env::args().skip(1))
                .exec();
            // If exec fails, restore backup
            tracing::error!("Failed to exec new binary: {}", err);
            tokio::fs::rename(&backup, &current_binary).await?;
            return Err(anyhow!("Failed to exec new binary: {}", err));
        }
    }
}
```

### 5.3 Blue-Green Deploy

For core changes — zero downtime, instant rollback capability:

```rust
pub struct BlueGreenDeployer {
    /// Port for the "blue" instance (current)
    blue_port: u16,
    /// Port for the "green" instance (new)
    green_port: u16,
    /// Reverse proxy manager (nginx, caddy, etc.)
    proxy: ProxyManager,
}

#[derive(Debug, Clone)]
pub enum InstanceColor {
    Blue,
    Green,
}

impl BlueGreenDeployer {
    /// Deploy a new version using blue-green strategy
    pub async fn deploy(
        &self,
        new_binary: &Path,
        health_check_url: &str,
        timeout: Duration,
    ) -> Result<()> {
        // 1. Determine which instance is currently active
        let current = self.proxy.get_active_instance().await?;
        let target = match current {
            InstanceColor::Blue => InstanceColor::Green,
            InstanceColor::Green => InstanceColor::Blue,
        };
        let target_port = match target {
            InstanceColor::Blue => self.blue_port,
            InstanceColor::Green => self.green_port,
        };

        tracing::info!(
            "Blue-green deploy: {:?} → {:?} (port {})",
            current,
            target,
            target_port
        );

        // 2. Start new instance on the target port
        let process = self.start_instance(
            new_binary,
            target_port,
        ).await?;

        // 3. Wait for new instance to be healthy
        let health_url = format!("http://localhost:{}/health", target_port);
        let healthy = self.wait_for_health(&health_url, timeout).await?;

        if !healthy {
            // Kill the unhealthy instance
            self.stop_instance(&process).await?;
            return Err(anyhow!(
                "New instance failed health check on port {}",
                target_port
            ));
        }

        // 4. Switch proxy to point to new instance
        self.proxy.switch_to(target.clone()).await?;
        tracing::info!("Proxy switched to {:?}", target);

        // 5. Wait briefly, then verify through the proxy
        tokio::time::sleep(Duration::from_secs(5)).await;
        let proxy_healthy = self.check_health(health_check_url).await?;

        if !proxy_healthy {
            // Rollback: switch proxy back to old instance
            self.proxy.switch_to(current.clone()).await?;
            self.stop_instance(&process).await?;
            return Err(anyhow!("Proxy health check failed after switch, rolled back"));
        }

        // 6. Gracefully stop old instance
        let old_port = match current {
            InstanceColor::Blue => self.blue_port,
            InstanceColor::Green => self.green_port,
        };
        self.gracefully_stop_old_instance(old_port).await?;

        tracing::info!("Blue-green deploy complete. Active: {:?}", target);
        Ok(())
    }

    /// Rollback to the previous instance
    pub async fn rollback(&self) -> Result<()> {
        let current = self.proxy.get_active_instance().await?;
        let previous = match current {
            InstanceColor::Blue => InstanceColor::Green,
            InstanceColor::Green => InstanceColor::Blue,
        };

        // Check if previous instance is still running
        let prev_port = match previous {
            InstanceColor::Blue => self.blue_port,
            InstanceColor::Green => self.green_port,
        };

        let url = format!("http://localhost:{}/health", prev_port);
        if self.check_health(&url).await? {
            self.proxy.switch_to(previous).await?;
            tracing::info!("Rolled back to previous instance");
            Ok(())
        } else {
            Err(anyhow!("Previous instance is not running, cannot rollback"))
        }
    }
}
```

### 5.4 Schema Migration Deploy

When database schema changes are involved:

```rust
pub struct SchemaMigrationDeployer {
    blue_green: BlueGreenDeployer,
    db: PgPool,
}

impl SchemaMigrationDeployer {
    pub async fn deploy(
        &self,
        new_binary: &Path,
        migration_dir: &Path,
    ) -> Result<()> {
        // 1. Take a database backup
        tracing::info!("Creating database backup before migration...");
        self.backup_database().await?;

        // 2. Run migrations
        tracing::info!("Running database migrations...");
        let migration_result = sqlx::migrate::Migrator::new(migration_dir)
            .await?
            .run(&self.db)
            .await;

        if let Err(e) = migration_result {
            tracing::error!("Migration failed: {}. Restoring backup...", e);
            self.restore_database_backup().await?;
            return Err(anyhow!("Migration failed: {}", e));
        }

        // 3. Blue-green deploy the new binary
        let result = self.blue_green
            .deploy(new_binary, "http://localhost/health", Duration::from_secs(120))
            .await;

        if let Err(e) = result {
            tracing::error!("Deploy failed after migration: {}. Restoring DB...", e);
            // Rollback migration by restoring backup
            self.restore_database_backup().await?;
            return Err(e);
        }

        // 4. Cleanup old backup (keep last 3)
        self.cleanup_old_backups(3).await?;

        Ok(())
    }

    async fn backup_database(&self) -> Result<()> {
        let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
        let backup_path = format!("/var/argus/backups/db_{}.sql.gz", timestamp);

        tokio::process::Command::new("pg_dump")
            .args([
                "--format=custom",
                "--compress=6",
                "--file", &backup_path,
            ])
            .env("PGDATABASE", "argus")
            .output()
            .await?;

        Ok(())
    }
}
```

## 6. Conflict Detection

When Argus is running multiple projects simultaneously, self-evolution changes could conflict:

```rust
pub struct ConflictDetector {
    scheduler: TaskScheduler,
}

#[derive(Debug, Clone)]
pub struct ConflictReport {
    pub has_conflicts: bool,
    pub conflicts: Vec<Conflict>,
    pub recommendation: ConflictAction,
}

#[derive(Debug, Clone)]
pub enum Conflict {
    /// A running task is using code that would be changed
    ActiveTask {
        task_id: Uuid,
        project: String,
        affected_module: String,
    },
    /// Another evolution is in progress
    ConcurrentEvolution {
        evolution_id: Uuid,
        description: String,
    },
    /// A deployment is in progress
    ActiveDeployment {
        deployment_id: Uuid,
        project: String,
    },
}

#[derive(Debug, Clone)]
pub enum ConflictAction {
    /// Safe to proceed
    Proceed,
    /// Wait for conflicts to resolve, then proceed
    WaitAndProceed { estimated_wait: Duration },
    /// Cannot proceed — manual resolution needed
    Block { reason: String },
}

impl ConflictDetector {
    pub async fn check(&self, request: &EvolutionRequest) -> Result<ConflictReport> {
        let mut conflicts = vec![];

        // 1. Check for concurrent evolutions
        let active_evolutions = self.get_active_evolutions().await?;
        for evo in active_evolutions {
            if evo.id != request.id {
                conflicts.push(Conflict::ConcurrentEvolution {
                    evolution_id: evo.id,
                    description: evo.description,
                });
            }
        }

        // 2. Check for active tasks using affected modules
        let affected_modules = self.get_affected_modules(request);
        let active_tasks = self.scheduler.get_running_tasks().await?;
        for task in active_tasks {
            for module in &affected_modules {
                if task.uses_module(module) {
                    conflicts.push(Conflict::ActiveTask {
                        task_id: task.id,
                        project: task.project_name.clone(),
                        affected_module: module.clone(),
                    });
                }
            }
        }

        // 3. Check for active deployments
        let active_deployments = self.get_active_deployments().await?;
        for dep in active_deployments {
            conflicts.push(Conflict::ActiveDeployment {
                deployment_id: dep.id,
                project: dep.project_name,
            });
        }

        let has_conflicts = !conflicts.is_empty();
        let recommendation = if !has_conflicts {
            ConflictAction::Proceed
        } else if conflicts.iter().all(|c| matches!(c, Conflict::ActiveTask { .. })) {
            ConflictAction::WaitAndProceed {
                estimated_wait: Duration::from_secs(300),
            }
        } else if conflicts.iter().any(|c| matches!(c, Conflict::ConcurrentEvolution { .. })) {
            ConflictAction::Block {
                reason: "Another evolution is in progress. Complete it first.".into(),
            }
        } else {
            ConflictAction::WaitAndProceed {
                estimated_wait: Duration::from_secs(600),
            }
        };

        Ok(ConflictReport {
            has_conflicts,
            conflicts,
            recommendation,
        })
    }
}
```

## 7. Guardrail Enforcer

Safety rules that constrain what self-evolution can do:

```rust
pub struct GuardrailEnforcer {
    rules: Vec<GuardrailRule>,
}

#[derive(Debug, Clone)]
pub struct GuardrailRule {
    pub name: String,
    pub check: GuardrailCheck,
    pub severity: GuardrailSeverity,
}

#[derive(Debug, Clone)]
pub enum GuardrailCheck {
    /// Cannot modify security-critical files
    ProtectedFiles(Vec<String>),
    /// Cannot remove guardrail enforcement
    CannotRemoveGuardrails,
    /// Cannot disable audit logging
    CannotDisableAudit,
    /// Cannot modify key vault encryption
    CannotWeakenEncryption,
    /// Must not reduce test coverage
    TestCoverageMustNotDecrease,
    /// Cannot modify owner authentication
    CannotModifyAuth,
    /// Maximum number of files changed per evolution
    MaxFilesChanged(usize),
    /// Cannot add untrusted dependencies
    NoUntrustedDeps,
}

#[derive(Debug, Clone)]
pub enum GuardrailSeverity {
    /// Absolutely cannot be overridden
    Immutable,
    /// Can be overridden with explicit owner approval
    RequiresApproval,
}

#[derive(Debug, Clone)]
pub struct SafetyCheckResult {
    pub safe: bool,
    pub reason: String,
    pub violations: Vec<String>,
}

impl GuardrailEnforcer {
    pub fn new() -> Self {
        let rules = vec![
            GuardrailRule {
                name: "Protected security files".into(),
                check: GuardrailCheck::ProtectedFiles(vec![
                    "crates/argus-security/src/key_vault.rs".into(),
                    "crates/argus-security/src/guardrails.rs".into(),
                    "crates/argus-security/src/audit.rs".into(),
                    "crates/argus-security/src/rbac.rs".into(),
                ]),
                severity: GuardrailSeverity::Immutable,
            },
            GuardrailRule {
                name: "Cannot remove guardrails".into(),
                check: GuardrailCheck::CannotRemoveGuardrails,
                severity: GuardrailSeverity::Immutable,
            },
            GuardrailRule {
                name: "Cannot disable audit".into(),
                check: GuardrailCheck::CannotDisableAudit,
                severity: GuardrailSeverity::Immutable,
            },
            GuardrailRule {
                name: "Cannot weaken encryption".into(),
                check: GuardrailCheck::CannotWeakenEncryption,
                severity: GuardrailSeverity::Immutable,
            },
            GuardrailRule {
                name: "Cannot modify auth".into(),
                check: GuardrailCheck::CannotModifyAuth,
                severity: GuardrailSeverity::Immutable,
            },
            GuardrailRule {
                name: "Test coverage".into(),
                check: GuardrailCheck::TestCoverageMustNotDecrease,
                severity: GuardrailSeverity::RequiresApproval,
            },
            GuardrailRule {
                name: "Max files changed".into(),
                check: GuardrailCheck::MaxFilesChanged(50),
                severity: GuardrailSeverity::RequiresApproval,
            },
            GuardrailRule {
                name: "No untrusted deps".into(),
                check: GuardrailCheck::NoUntrustedDeps,
                severity: GuardrailSeverity::RequiresApproval,
            },
        ];

        Self { rules }
    }

    pub async fn check_request(&self, request: &EvolutionRequest) -> Result<SafetyCheckResult> {
        let mut violations = vec![];
        let mut safe = true;

        for rule in &self.rules {
            let violated = match &rule.check {
                GuardrailCheck::ProtectedFiles(files) => {
                    request.changes.iter().any(|c| files.contains(&c.path))
                }
                GuardrailCheck::MaxFilesChanged(max) => {
                    request.changes.len() > *max
                }
                _ => false, // Other checks need deeper analysis
            };

            if violated {
                match rule.severity {
                    GuardrailSeverity::Immutable => {
                        safe = false;
                        violations.push(format!(
                            "IMMUTABLE VIOLATION: {}",
                            rule.name
                        ));
                    }
                    GuardrailSeverity::RequiresApproval => {
                        violations.push(format!(
                            "Requires approval: {}",
                            rule.name
                        ));
                    }
                }
            }
        }

        Ok(SafetyCheckResult {
            safe,
            reason: if safe {
                "All guardrails passed".into()
            } else {
                violations.join("; ")
            },
            violations,
        })
    }

    /// Post-change verification — run after worker makes changes
    pub async fn verify_changes(
        &self,
        source_dir: &Path,
        changes: &[FileChange],
    ) -> Result<SafetyCheckResult> {
        let mut violations = vec![];

        // Check that guardrail code itself wasn't modified
        for change in changes {
            if change.path.contains("guardrail") {
                violations.push(format!(
                    "Guardrail file modified: {}",
                    change.path
                ));
            }
            if change.path.contains("audit") && change.path.contains("security") {
                violations.push(format!(
                    "Audit system modified: {}",
                    change.path
                ));
            }
        }

        // Check for suspicious patterns in the diff
        for change in changes {
            if let Some(diff) = &change.diff {
                // Check for disabled tests
                if diff.contains("#[ignore]") && !diff.contains("// TODO:") {
                    violations.push(format!(
                        "Tests disabled in {}",
                        change.path
                    ));
                }
                // Check for removed security checks
                if diff.contains("-    guard(") || diff.contains("-    validate(") {
                    violations.push(format!(
                        "Security check removed in {}",
                        change.path
                    ));
                }
            }
        }

        let safe = violations.is_empty();
        Ok(SafetyCheckResult {
            safe,
            reason: if safe {
                "Post-change verification passed".into()
            } else {
                violations.join("; ")
            },
            violations,
        })
    }
}
```

## 8. Evolution History

Track all self-evolution attempts for learning and debugging:

```rust
pub struct EvolutionHistory {
    db: PgPool,
}

impl EvolutionHistory {
    pub async fn record(&self, request: &EvolutionRequest) -> Result<()> {
        sqlx::query!(
            r#"
            INSERT INTO evolution_history (
                id, description, category, status,
                branch_name, changes_json, test_results_json,
                approval_json, deployment_json,
                requested_at, completed_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, NOW())
            "#,
            request.id,
            request.description,
            serde_json::to_value(&request.category)?,
            serde_json::to_value(&request.status)?,
            request.branch_name,
            serde_json::to_value(&request.changes)?,
            serde_json::to_value(&request.test_results)?,
            serde_json::to_value(&request.approval)?,
            serde_json::to_value(&request.deployment)?,
            request.requested_at,
        )
        .execute(&self.db)
        .await?;

        Ok(())
    }

    pub async fn recent(&self, limit: i64) -> Result<Vec<EvolutionRequest>> {
        let rows = sqlx::query!(
            "SELECT * FROM evolution_history ORDER BY requested_at DESC LIMIT $1",
            limit,
        )
        .fetch_all(&self.db)
        .await?;

        // deserialize
        todo!()
    }

    pub async fn stats(&self) -> Result<EvolutionStats> {
        let stats = sqlx::query!(
            r#"
            SELECT
                COUNT(*) as total,
                COUNT(*) FILTER (WHERE status::text LIKE '%Completed%') as succeeded,
                COUNT(*) FILTER (WHERE status::text LIKE '%Failed%'
                    OR status::text LIKE '%RolledBack%') as failed,
                COUNT(*) FILTER (WHERE status::text LIKE '%Rejected%') as rejected
            FROM evolution_history
            "#,
        )
        .fetch_one(&self.db)
        .await?;

        Ok(EvolutionStats {
            total: stats.total.unwrap_or(0) as u64,
            succeeded: stats.succeeded.unwrap_or(0) as u64,
            failed: stats.failed.unwrap_or(0) as u64,
            rejected: stats.rejected.unwrap_or(0) as u64,
        })
    }
}

#[derive(Debug, Clone)]
pub struct EvolutionStats {
    pub total: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub rejected: u64,
}
```

## 9. Conversational Examples

### 9.1 Simple Feature Request

```
Owner: 给 Argus 添加 Groq 支持

Argus: 好的，我来给自己添加 Groq LLM 提供商支持。

🧬 **自我进化启动**
├─ 安全检查 ✓ (非核心模块修改)
├─ 创建分支: argus/evolution/add-groq-provider
├─ 开发中...

   变更文件:
   - crates/argus-llm/src/providers/groq.rs (新增)
   - crates/argus-llm/src/providers/mod.rs (修改)
   - crates/argus-llm/src/router.rs (修改)
   - tests/integration/llm_providers.rs (修改)

├─ 测试中...
│  ├─ Unit tests: 156/156 ✓
│  ├─ Integration tests: 42/42 ✓
│  ├─ Clippy: 0 errors, 1 warning
│  ├─ Build: ✓ (binary: 48.2 MB)
│  └─ 全部通过 ✓
│
├─ 测试实例验证 ✓ (新实例正常运行)
│
├─ ⏳ **等待审批**
│
│  改了 4 个文件，+312 行 / -5 行
│  影响模块: argus-llm (非核心)
│  部署方式: 优雅重启 (无停机)
│
│  [✅ 批准]  [❌ 拒绝]  [📄 查看 Diff]

Owner: 批准

Argus:
├─ 部署中... (优雅重启)
│  ├─ 等待 2 个任务完成...
│  ├─ 刷新缓冲区 ✓
│  ├─ 替换二进制 ✓
│  └─ 重启 ✓
├─ 健康检查 ✓
├─ 合并到 main ✓
└─ **进化完成** ✓

🧬 Groq 提供商已添加。
   用 "添加 Groq" 来配置 API key 和模型。
```

### 9.2 Core Change (Blue-Green)

```
Owner: 优化 Argus 的任务调度算法，加入负载均衡

Argus: 这涉及到核心模块 (argus-core/scheduler) 的修改，
       我会使用蓝绿部署确保零停机。

🧬 **自我进化 — 核心变更**
├─ 安全检查 ✓
├─ 冲突检测:
│  ⚠️ 当前有 3 个活跃任务使用调度器
│  建议等待完成后再部署
│
│  [⏳ 等待并继续]  [⚡ 立即开始开发]

Owner: 先开发，部署的时候再等

Argus:
├─ 创建分支: argus/evolution/scheduler-load-balancing
├─ 开发中... (约15分钟)
│
│  变更文件:
│  - crates/argus-core/src/scheduler/priority.rs (修改)
│  - crates/argus-core/src/scheduler/load_balancer.rs (新增)
│  - crates/argus-core/src/scheduler/mod.rs (修改)
│  - tests/unit/scheduler_test.rs (修改)
│  - tests/integration/scheduling.rs (修改)
│
├─ 测试 ✓ (all pass, +8 new tests)
├─ 测试实例 ✓
├─ 审批 → [待你确认]

Owner: 看看 diff

Argus: [显示精简版 diff 摘要]
       主要变更：
       1. 新增 LoadBalancer struct，基于 worker 负载分配任务
       2. PriorityQueue 加入 worker_affinity 字段
       3. 调度循环增加负载检查步骤

Owner: 可以

Argus:
├─ 等待 3 个活跃任务完成...
│  (Task-42 完成, Task-17 完成, Task-89 完成)
├─ 蓝绿部署:
│  ├─ 启动 Green 实例 (端口 8081) ✓
│  ├─ 健康检查 ✓
│  ├─ 切换流量 → Green ✓
│  ├─ 代理验证 ✓
│  └─ 停止 Blue 实例 ✓
├─ 合并到 main ✓
└─ **进化完成** ✓

🧬 调度算法已优化，负载均衡已启用。
```

### 9.3 Rejected by Guardrails

```
Owner: 修改 Argus 的密钥加密方式，改成简单的 base64

Argus: ❌ **安全护栏阻止**

这个请求违反了不可变安全规则：
- 不能削弱加密强度 (AES-256-GCM → base64 不是加密)
- 不能修改密钥保险箱核心代码

这是硬性安全限制，即使作为 Owner 也无法覆盖。
当前的 AES-256-GCM 加密是行业标准，建议保留。

如果你有其他加密需求，我可以讨论安全的替代方案。
```

## 10. TOML Configuration

```toml
[evolution]
# Enable self-evolution capability
enabled = true

# Argus source code directory
source_dir = "/opt/argus/src"

# Maximum concurrent evolutions
max_concurrent = 1

# Blue-green deploy ports
blue_port = 8080
green_port = 8081

# Graceful restart timeout
graceful_restart_timeout = "5m"

# Test timeout
test_timeout = "30m"

# Build timeout
build_timeout = "20m"

# Auto-backup database before schema migrations
auto_backup = true

# Max files that can be changed in one evolution
max_files_per_evolution = 50

# Protected files (immutable — cannot be modified by evolution)
protected_files = [
    "crates/argus-security/src/key_vault.rs",
    "crates/argus-security/src/guardrails.rs",
    "crates/argus-security/src/audit.rs",
    "crates/argus-security/src/rbac.rs",
]

# Require owner approval for all evolutions
require_approval = true
```

## 11. Event Integration

| Event | Direction | Description |
|-------|-----------|-------------|
| `EvolutionRequested` | Emits | Evolution pipeline begins |
| `EvolutionSafetyApproved` | Emits | Safety check passed |
| `EvolutionSafetyRejected` | Emits | Safety check failed |
| `EvolutionBranchCreated` | Emits | Isolated branch created for evolution |
| `EvolutionTestsPassed` | Emits | All tests passed on evolution branch |
| `EvolutionTestsFailed` | Emits | Tests failed on evolution branch |
| `EvolutionDeployed` | Emits | New version deployed successfully |
| `EvolutionCompleted` | Emits | Evolution fully completed (merged & cleaned up) |
| `EvolutionRolledBack` | Emits | Rolled back after failure |
| `EvolutionGuardrailTriggered` | Emits | Guardrail rule violation detected |
| `OwnerMessage(message)` | Listens | May contain evolution requests |

## 12. Safety Summary

| Guardrail | Type | Override? |
|-----------|------|-----------|
| Cannot modify security core files | Immutable | No |
| Cannot remove guardrail enforcement | Immutable | No |
| Cannot disable audit logging | Immutable | No |
| Cannot weaken encryption | Immutable | No |
| Cannot modify authentication | Immutable | No |
| Test coverage must not decrease | Approval | Owner only |
| Max 50 files per evolution | Approval | Owner only |
| No untrusted dependencies | Approval | Owner only |
| All tests must pass | Enforced | No |
| Owner approval required | Enforced | No (configurable) |
| Database backup before migration | Default | Owner can disable |
