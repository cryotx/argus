# Deploy Engine — Detailed Design

The Deploy Engine (`argus-deploy`) manages multi-stage deployment pipelines for all projects Argus oversees. It supports multiple deployment targets (Docker, VPS/SSH, Vercel, Kubernetes, WeChat Mini Program), provides snapshot-based rollback, health checking, environment management, and integrates with the approval system for production deploys.

## 1. Architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│                        Deploy Engine                                  │
│                                                                       │
│  ┌────────────────────────────────────────────────────────────────┐  │
│  │                   Pipeline Executor                             │  │
│  │  ┌────────┐ ┌────────┐ ┌────────┐ ┌──────┐ ┌──────┐ ┌──────┐│  │
│  │  │ Build  │→│  Test  │→│Package │→│Pre-  │→│Deploy│→│Health││  │
│  │  │        │ │        │ │        │ │Check │ │      │ │Check ││  │
│  │  └────────┘ └────────┘ └────────┘ └──────┘ └──────┘ └──────┘│  │
│  │                              ↑ Approval Gate (prod only)       │  │
│  └────────────────────────────────────────────────────────────────┘  │
│                                                                       │
│  ┌──────────────────┐  ┌──────────────────┐  ┌────────────────────┐ │
│  │  Deployer         │  │  Rollback        │  │  Environment       │ │
│  │  Registry         │  │  Manager         │  │  Manager           │ │
│  │  (target drivers) │  │  (snapshots)     │  │  (dev/stg/prod)    │ │
│  └──────────────────┘  └──────────────────┘  └────────────────────┘ │
│                                                                       │
│  ┌──────────────────┐  ┌──────────────────┐  ┌────────────────────┐ │
│  │  Health Checker   │  │  Artifact        │  │  Deployment        │ │
│  │  (probes/custom)  │  │  Store           │  │  History           │ │
│  └──────────────────┘  └──────────────────┘  └────────────────────┘ │
└──────────────────────────────────────────────────────────────────────┘
```

## 2. Deployment Pipeline

### 2.1 Pipeline Model

Each deployment is a sequence of stages. If any stage fails, the pipeline halts and either retries or rolls back.

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Unique deployment identifier
pub type DeploymentId = Uuid;
pub type ProjectId = Uuid;

/// The six stages of a deployment pipeline
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PipelineStage {
    Build,
    Test,
    Package,
    PreCheck,
    Deploy,
    HealthCheck,
}

impl PipelineStage {
    pub fn all_ordered() -> Vec<PipelineStage> {
        vec![
            PipelineStage::Build,
            PipelineStage::Test,
            PipelineStage::Package,
            PipelineStage::PreCheck,
            PipelineStage::Deploy,
            PipelineStage::HealthCheck,
        ]
    }

    pub fn next(&self) -> Option<PipelineStage> {
        match self {
            PipelineStage::Build => Some(PipelineStage::Test),
            PipelineStage::Test => Some(PipelineStage::Package),
            PipelineStage::Package => Some(PipelineStage::PreCheck),
            PipelineStage::PreCheck => Some(PipelineStage::Deploy),
            PipelineStage::Deploy => Some(PipelineStage::HealthCheck),
            PipelineStage::HealthCheck => None,
        }
    }
}

/// The result of an individual pipeline stage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageResult {
    pub stage: PipelineStage,
    pub status: StageStatus,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub logs: Vec<String>,
    pub artifacts: Vec<ArtifactRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum StageStatus {
    Pending,
    Running,
    Passed,
    Failed(String),
    Skipped,
}

/// Reference to a build artifact
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
    pub checksum_sha256: String,
}
```

### 2.2 Deployment Record

A single deployment attempt, from trigger through completion or failure:

```rust
/// A complete deployment record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Deployment {
    pub id: DeploymentId,
    pub project_id: ProjectId,
    pub environment: Environment,
    pub target: DeployTarget,
    pub trigger: DeployTrigger,
    pub git_ref: String,           // commit SHA or tag
    pub git_branch: String,
    pub status: DeploymentStatus,
    pub stages: Vec<StageResult>,
    pub snapshot_id: Option<SnapshotId>,  // pre-deploy snapshot for rollback
    pub config: DeployConfig,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub rolled_back: bool,
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DeploymentStatus {
    Queued,
    Running,
    WaitingApproval,      // blocked at approval gate
    Approved,
    Succeeded,
    Failed(String),
    RolledBack,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DeployTrigger {
    /// Automatic — triggered by task completion
    Automatic { task_id: Uuid },
    /// Owner requested via conversation
    OwnerRequest { message: String },
    /// Scheduled (cron-like)
    Scheduled { schedule_name: String },
    /// Retry after failure
    Retry { previous_deployment_id: DeploymentId },
}
```

### 2.3 Pipeline Executor

The executor runs stages sequentially, emitting events and handling failures:

```rust
pub struct PipelineExecutor {
    deployer_registry: DeployerRegistry,
    artifact_store: ArtifactStore,
    health_checker: HealthChecker,
    rollback_manager: RollbackManager,
    event_bus: EventBusTx,
}

impl PipelineExecutor {
    /// Execute the full deployment pipeline
    pub async fn execute(&self, deployment: &mut Deployment) -> Result<()> {
        self.event_bus.send(ArgusEvent::DeployStarted(
            deployment.project_id,
            deployment.environment.clone(),
        ));

        for stage in PipelineStage::all_ordered() {
            // Check for approval gate before Deploy stage on production
            if stage == PipelineStage::Deploy
                && deployment.environment == Environment::Production
                && deployment.config.require_approval
            {
                deployment.status = DeploymentStatus::WaitingApproval;
                self.event_bus.send(ArgusEvent::DeployAwaitingApproval(
                    deployment.id,
                    deployment.project_id,
                ));

                // Block until owner approves or rejects
                let approved = self.wait_for_approval(deployment).await?;
                if !approved {
                    deployment.status = DeploymentStatus::Cancelled;
                    return Ok(());
                }
                deployment.status = DeploymentStatus::Approved;
            }

            let result = self.execute_stage(deployment, &stage).await;

            match &result.status {
                StageStatus::Passed => {
                    deployment.stages.push(result);
                    self.event_bus.send(ArgusEvent::DeployStageCompleted(
                        deployment.project_id,
                        format!("{:?}", stage),
                    ));
                }
                StageStatus::Failed(reason) => {
                    deployment.stages.push(result);
                    deployment.status = DeploymentStatus::Failed(reason.clone());

                    // Auto-rollback if deploy or health check failed
                    if matches!(stage, PipelineStage::Deploy | PipelineStage::HealthCheck) {
                        if let Some(snapshot_id) = &deployment.snapshot_id {
                            self.rollback_manager
                                .rollback(deployment, snapshot_id)
                                .await?;
                            deployment.rolled_back = true;
                            deployment.status = DeploymentStatus::RolledBack;
                            self.event_bus.send(ArgusEvent::DeployRolledBack(
                                deployment.project_id,
                            ));
                        }
                    }

                    self.event_bus.send(ArgusEvent::DeployFailed(
                        deployment.project_id,
                        reason.clone(),
                    ));
                    return Ok(());
                }
                _ => {}
            }
        }

        deployment.status = DeploymentStatus::Succeeded;
        deployment.finished_at = Some(Utc::now());
        self.event_bus.send(ArgusEvent::DeploySucceeded(
            deployment.project_id,
            DeploymentInfo::from(deployment),
        ));
        Ok(())
    }

    /// Execute a single stage
    async fn execute_stage(
        &self,
        deployment: &mut Deployment,
        stage: &PipelineStage,
    ) -> StageResult {
        let started_at = Utc::now();
        let timeout = deployment.config.stage_timeouts
            .get(stage)
            .copied()
            .unwrap_or(Duration::from_secs(600)); // 10 min default

        let result = tokio::time::timeout(
            timeout,
            self.run_stage(deployment, stage),
        ).await;

        match result {
            Ok(Ok((logs, artifacts))) => StageResult {
                stage: stage.clone(),
                status: StageStatus::Passed,
                started_at,
                finished_at: Some(Utc::now()),
                logs,
                artifacts,
            },
            Ok(Err(e)) => StageResult {
                stage: stage.clone(),
                status: StageStatus::Failed(e.to_string()),
                started_at,
                finished_at: Some(Utc::now()),
                logs: vec![e.to_string()],
                artifacts: vec![],
            },
            Err(_) => StageResult {
                stage: stage.clone(),
                status: StageStatus::Failed("Stage timed out".into()),
                started_at,
                finished_at: Some(Utc::now()),
                logs: vec![format!("Timed out after {:?}", timeout)],
                artifacts: vec![],
            },
        }
    }

    /// Route stage execution to the appropriate handler
    async fn run_stage(
        &self,
        deployment: &mut Deployment,
        stage: &PipelineStage,
    ) -> Result<(Vec<String>, Vec<ArtifactRef>)> {
        let deployer = self.deployer_registry.get(&deployment.target)?;

        match stage {
            PipelineStage::Build => deployer.build(deployment).await,
            PipelineStage::Test => deployer.test(deployment).await,
            PipelineStage::Package => deployer.package(deployment).await,
            PipelineStage::PreCheck => {
                // Take snapshot before deploying
                let snapshot_id = self.rollback_manager
                    .create_snapshot(deployment)
                    .await?;
                deployment.snapshot_id = Some(snapshot_id);
                deployer.pre_check(deployment).await
            }
            PipelineStage::Deploy => deployer.deploy(deployment).await,
            PipelineStage::HealthCheck => {
                self.health_checker
                    .run_checks(deployment)
                    .await
                    .map(|logs| (logs, vec![]))
            }
        }
    }
}
```

## 3. Deployer Interface

### 3.1 Deployer Trait

All deployment targets implement this trait:

```rust
#[async_trait]
pub trait Deployer: Send + Sync {
    /// Target type this deployer handles
    fn target_type(&self) -> DeployTargetType;

    /// Validate deployment configuration before starting
    async fn validate_config(&self, config: &DeployConfig) -> Result<Vec<String>>;

    /// Build stage — compile, bundle, etc.
    async fn build(&self, deployment: &Deployment) -> Result<(Vec<String>, Vec<ArtifactRef>)>;

    /// Test stage — run test suite
    async fn test(&self, deployment: &Deployment) -> Result<(Vec<String>, Vec<ArtifactRef>)>;

    /// Package stage — create deployable artifact (Docker image, tarball, etc.)
    async fn package(&self, deployment: &Deployment) -> Result<(Vec<String>, Vec<ArtifactRef>)>;

    /// Pre-check — verify target environment is ready
    async fn pre_check(&self, deployment: &Deployment) -> Result<(Vec<String>, Vec<ArtifactRef>)>;

    /// Deploy — actually push the artifact to the target
    async fn deploy(&self, deployment: &Deployment) -> Result<(Vec<String>, Vec<ArtifactRef>)>;

    /// Create a snapshot of current state for rollback
    async fn snapshot(&self, deployment: &Deployment) -> Result<Snapshot>;

    /// Rollback to a previous snapshot
    async fn rollback(&self, deployment: &Deployment, snapshot: &Snapshot) -> Result<()>;
}
```

### 3.2 Deploy Target Types

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DeployTarget {
    Docker(DockerTarget),
    Vps(VpsTarget),
    Vercel(VercelTarget),
    Kubernetes(K8sTarget),
    WeChatMiniProgram(WeChatTarget),
    Static(StaticTarget),   // Static file hosting (S3, Cloudflare Pages, etc.)
    Custom(CustomTarget),   // User-defined deployment via shell commands
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DeployTargetType {
    Docker,
    Vps,
    Vercel,
    Kubernetes,
    WeChatMiniProgram,
    Static,
    Custom,
}
```

## 4. Deployer Implementations

### 4.1 Docker Deployer

Builds Docker images and manages containers using the bollard crate:

```rust
pub struct DockerDeployer {
    docker: bollard::Docker,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockerTarget {
    /// Docker host (local socket or remote TCP)
    pub host: Option<String>,
    /// Dockerfile path relative to project root
    pub dockerfile: String,
    /// Image name and tag format
    pub image_name: String,
    /// Container name
    pub container_name: String,
    /// Port mappings: host:container
    pub ports: Vec<PortMapping>,
    /// Environment variables
    pub env: HashMap<String, String>,
    /// Volume mounts
    pub volumes: Vec<VolumeMount>,
    /// Docker network
    pub network: Option<String>,
    /// Resource limits
    pub resources: Option<ContainerResources>,
    /// Restart policy
    pub restart_policy: RestartPolicy,
    /// Docker Compose file (if using compose instead of single container)
    pub compose_file: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortMapping {
    pub host_port: u16,
    pub container_port: u16,
    pub protocol: String, // "tcp" or "udp"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeMount {
    pub host_path: String,
    pub container_path: String,
    pub read_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerResources {
    pub memory_limit: Option<String>,   // e.g. "512m"
    pub cpu_limit: Option<f64>,         // e.g. 1.5 cores
    pub memory_reservation: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RestartPolicy {
    No,
    Always,
    OnFailure { max_retries: u32 },
    UnlessStopped,
}

#[async_trait]
impl Deployer for DockerDeployer {
    fn target_type(&self) -> DeployTargetType {
        DeployTargetType::Docker
    }

    async fn build(&self, deployment: &Deployment) -> Result<(Vec<String>, Vec<ArtifactRef>)> {
        let target = deployment.target.as_docker()?;
        let mut logs = vec![];

        // If compose file exists, use docker-compose build
        if let Some(compose) = &target.compose_file {
            logs.push(format!("Building with docker-compose: {}", compose));
            // Run: docker-compose -f {compose} build
            let output = self.run_compose_command(
                &deployment.workspace_path(),
                compose,
                &["build"],
            ).await?;
            logs.extend(output);
        } else {
            // Single container: docker build
            let tag = format!("{}:{}", target.image_name, deployment.git_ref_short());
            logs.push(format!("Building Docker image: {}", tag));
            let output = self.build_image(
                &deployment.workspace_path(),
                &target.dockerfile,
                &tag,
            ).await?;
            logs.extend(output);
        }

        Ok((logs, vec![]))
    }

    async fn deploy(&self, deployment: &Deployment) -> Result<(Vec<String>, Vec<ArtifactRef>)> {
        let target = deployment.target.as_docker()?;
        let mut logs = vec![];

        if let Some(compose) = &target.compose_file {
            // docker-compose up -d
            logs.push("Deploying with docker-compose up -d".into());
            let output = self.run_compose_command(
                &deployment.workspace_path(),
                compose,
                &["up", "-d", "--remove-orphans"],
            ).await?;
            logs.extend(output);
        } else {
            // Stop old container (if exists)
            let _ = self.stop_container(&target.container_name).await;
            let _ = self.remove_container(&target.container_name).await;

            // Start new container
            let tag = format!("{}:{}", target.image_name, deployment.git_ref_short());
            logs.push(format!("Starting container {} from {}", target.container_name, tag));
            self.create_and_start_container(&tag, target).await?;
            logs.push("Container started successfully".into());
        }

        Ok((logs, vec![]))
    }

    async fn snapshot(&self, deployment: &Deployment) -> Result<Snapshot> {
        let target = deployment.target.as_docker()?;
        // Commit current container state as a snapshot image
        let snapshot_tag = format!(
            "{}-snapshot-{}",
            target.image_name,
            Utc::now().format("%Y%m%d%H%M%S")
        );
        self.docker.commit_container(&target.container_name, &snapshot_tag).await?;

        Ok(Snapshot {
            id: Uuid::new_v4(),
            snapshot_type: SnapshotType::DockerImage(snapshot_tag),
            created_at: Utc::now(),
        })
    }

    async fn rollback(&self, deployment: &Deployment, snapshot: &Snapshot) -> Result<()> {
        let target = deployment.target.as_docker()?;
        if let SnapshotType::DockerImage(tag) = &snapshot.snapshot_type {
            let _ = self.stop_container(&target.container_name).await;
            let _ = self.remove_container(&target.container_name).await;
            self.create_and_start_container(tag, target).await?;
        }
        Ok(())
    }

    // ... test, package, pre_check, validate_config implementations
}
```

### 4.2 VPS/SSH Deployer

Deploys to remote servers via SSH (using the `russh` crate):

```rust
pub struct VpsDeployer {
    // SSH connection pool
    ssh_pool: SshConnectionPool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VpsTarget {
    /// SSH host
    pub host: String,
    /// SSH port
    pub port: u16,
    /// SSH user
    pub user: String,
    /// Auth method
    pub auth: SshAuth,
    /// Remote deployment directory
    pub deploy_dir: String,
    /// Pre-deploy commands (run before copying files)
    pub pre_commands: Vec<String>,
    /// Post-deploy commands (run after copying files)
    pub post_commands: Vec<String>,
    /// Service management
    pub service: Option<ServiceConfig>,
    /// Files/dirs to exclude from sync
    pub exclude: Vec<String>,
    /// Use rsync for file transfer
    pub use_rsync: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SshAuth {
    Key { path: String },
    KeyFromVault { key_name: String },
    Agent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceConfig {
    /// Service manager type
    pub manager: ServiceManager,
    /// Service name
    pub name: String,
    /// Restart command (if custom)
    pub restart_command: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServiceManager {
    Systemd,
    Supervisor,
    Pm2,
    Custom,
}

#[async_trait]
impl Deployer for VpsDeployer {
    async fn deploy(&self, deployment: &Deployment) -> Result<(Vec<String>, Vec<ArtifactRef>)> {
        let target = deployment.target.as_vps()?;
        let mut logs = vec![];
        let session = self.ssh_pool.get_session(target).await?;

        // Run pre-deploy commands
        for cmd in &target.pre_commands {
            logs.push(format!("Pre-deploy: {}", cmd));
            let output = session.execute(cmd).await?;
            logs.push(output);
        }

        // Sync files to remote
        if target.use_rsync {
            logs.push("Syncing files via rsync...".into());
            let output = self.rsync_to_remote(deployment, target).await?;
            logs.extend(output);
        } else {
            logs.push("Uploading files via SCP...".into());
            self.scp_upload(deployment, target, &session).await?;
        }

        // Run post-deploy commands
        for cmd in &target.post_commands {
            logs.push(format!("Post-deploy: {}", cmd));
            let output = session.execute(cmd).await?;
            logs.push(output);
        }

        // Restart service
        if let Some(service) = &target.service {
            logs.push(format!("Restarting service: {}", service.name));
            let restart_cmd = match &service.restart_command {
                Some(cmd) => cmd.clone(),
                None => match service.manager {
                    ServiceManager::Systemd => {
                        format!("sudo systemctl restart {}", service.name)
                    }
                    ServiceManager::Supervisor => {
                        format!("sudo supervisorctl restart {}", service.name)
                    }
                    ServiceManager::Pm2 => {
                        format!("pm2 restart {}", service.name)
                    }
                    ServiceManager::Custom => {
                        return Err(anyhow!("Custom service manager requires restart_command"));
                    }
                },
            };
            let output = session.execute(&restart_cmd).await?;
            logs.push(output);
        }

        Ok((logs, vec![]))
    }

    async fn snapshot(&self, deployment: &Deployment) -> Result<Snapshot> {
        let target = deployment.target.as_vps()?;
        let session = self.ssh_pool.get_session(target).await?;

        // Create a tarball snapshot of the deploy directory
        let snapshot_name = format!(
            "snapshot-{}.tar.gz",
            Utc::now().format("%Y%m%d%H%M%S")
        );
        let snapshot_path = format!("{}/{}", target.deploy_dir, snapshot_name);
        session.execute(&format!(
            "tar -czf {} -C {} .",
            snapshot_path, target.deploy_dir
        )).await?;

        Ok(Snapshot {
            id: Uuid::new_v4(),
            snapshot_type: SnapshotType::RemoteTarball {
                host: target.host.clone(),
                path: snapshot_path,
            },
            created_at: Utc::now(),
        })
    }

    async fn rollback(&self, deployment: &Deployment, snapshot: &Snapshot) -> Result<()> {
        let target = deployment.target.as_vps()?;
        let session = self.ssh_pool.get_session(target).await?;

        if let SnapshotType::RemoteTarball { path, .. } = &snapshot.snapshot_type {
            // Clear deploy dir and extract snapshot
            session.execute(&format!(
                "rm -rf {}/* && tar -xzf {} -C {}",
                target.deploy_dir, path, target.deploy_dir
            )).await?;

            // Restart service
            if let Some(service) = &target.service {
                let restart_cmd = self.build_restart_command(service);
                session.execute(&restart_cmd).await?;
            }
        }
        Ok(())
    }
}
```

### 4.3 Vercel Deployer

Deploys frontend projects via Vercel CLI/API:

```rust
pub struct VercelDeployer {
    http_client: reqwest::Client,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VercelTarget {
    /// Vercel API token (from key vault)
    pub token_key: String,
    /// Vercel project ID
    pub project_id: Option<String>,
    /// Vercel team/org ID
    pub team_id: Option<String>,
    /// Production domain
    pub production_domain: Option<String>,
    /// Framework preset (next, vite, nuxt, etc.)
    pub framework: Option<String>,
    /// Build command override
    pub build_command: Option<String>,
    /// Output directory override
    pub output_dir: Option<String>,
    /// Environment variables to set
    pub env: HashMap<String, String>,
}

#[async_trait]
impl Deployer for VercelDeployer {
    async fn deploy(&self, deployment: &Deployment) -> Result<(Vec<String>, Vec<ArtifactRef>)> {
        let target = deployment.target.as_vercel()?;
        let mut logs = vec![];

        // Use Vercel CLI for deployment
        let mut args = vec!["deploy", "--yes"];

        if deployment.environment == Environment::Production {
            args.push("--prod");
        }

        if let Some(token) = self.resolve_token(&target.token_key).await? {
            args.extend(["--token", &token]);
        }

        logs.push(format!("Deploying to Vercel (env: {:?})...", deployment.environment));
        let output = self.run_vercel_cli(
            &deployment.workspace_path(),
            &args,
        ).await?;

        // Parse deployment URL from output
        let deploy_url = self.parse_deployment_url(&output)?;
        logs.push(format!("Deployed to: {}", deploy_url));

        Ok((logs, vec![]))
    }

    async fn snapshot(&self, deployment: &Deployment) -> Result<Snapshot> {
        // Vercel keeps deployment history natively
        // Snapshot = current production deployment URL
        let target = deployment.target.as_vercel()?;
        let current_url = self.get_current_production_url(target).await?;

        Ok(Snapshot {
            id: Uuid::new_v4(),
            snapshot_type: SnapshotType::VercelDeployment {
                deployment_url: current_url,
            },
            created_at: Utc::now(),
        })
    }

    async fn rollback(&self, deployment: &Deployment, snapshot: &Snapshot) -> Result<()> {
        // Vercel supports instant rollback to previous deployment
        if let SnapshotType::VercelDeployment { deployment_url } = &snapshot.snapshot_type {
            let target = deployment.target.as_vercel()?;
            self.promote_deployment(target, deployment_url).await?;
        }
        Ok(())
    }
}
```

### 4.4 Kubernetes Deployer

Deploys to Kubernetes clusters via `kube` crate:

```rust
pub struct K8sDeployer {
    // kube::Client created from kubeconfig
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct K8sTarget {
    /// Kubeconfig path or context name
    pub kubeconfig: KubeConfigSource,
    /// Target namespace
    pub namespace: String,
    /// Deployment manifest templates (Kubernetes YAML)
    pub manifests: Vec<String>,
    /// Helm chart (alternative to raw manifests)
    pub helm: Option<HelmConfig>,
    /// Container registry for pushing images
    pub registry: Option<ContainerRegistry>,
    /// Rolling update strategy
    pub strategy: K8sUpdateStrategy,
    /// Readiness timeout
    pub readiness_timeout: Duration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum KubeConfigSource {
    Default,
    File(String),
    Context(String),
    InCluster,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelmConfig {
    pub chart: String,           // chart path or repo/chart
    pub release_name: String,
    pub values_file: Option<String>,
    pub set_values: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerRegistry {
    pub url: String,
    pub username_key: String,    // key vault reference
    pub password_key: String,    // key vault reference
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum K8sUpdateStrategy {
    RollingUpdate {
        max_unavailable: String,
        max_surge: String,
    },
    Recreate,
    BlueGreen,
    Canary {
        initial_weight: u32,   // percentage
        increment: u32,
        interval: Duration,
    },
}

#[async_trait]
impl Deployer for K8sDeployer {
    async fn deploy(&self, deployment: &Deployment) -> Result<(Vec<String>, Vec<ArtifactRef>)> {
        let target = deployment.target.as_k8s()?;
        let mut logs = vec![];

        if let Some(helm) = &target.helm {
            // Helm-based deployment
            logs.push(format!("Helm upgrade: {}", helm.release_name));
            let output = self.helm_upgrade(deployment, helm, target).await?;
            logs.extend(output);
        } else {
            // Raw manifest deployment
            for manifest in &target.manifests {
                logs.push(format!("Applying manifest: {}", manifest));
                let rendered = self.render_manifest(manifest, deployment)?;
                self.apply_manifest(&rendered, &target.namespace).await?;
            }
        }

        // Wait for rollout to complete
        logs.push("Waiting for rollout to complete...".into());
        self.wait_for_rollout(deployment, target).await?;
        logs.push("Rollout completed successfully".into());

        Ok((logs, vec![]))
    }

    async fn snapshot(&self, deployment: &Deployment) -> Result<Snapshot> {
        let target = deployment.target.as_k8s()?;

        // Record current revision for rollback
        let revision = if let Some(helm) = &target.helm {
            self.get_helm_revision(&helm.release_name, &target.namespace).await?
        } else {
            self.get_k8s_revision(deployment, target).await?
        };

        Ok(Snapshot {
            id: Uuid::new_v4(),
            snapshot_type: SnapshotType::K8sRevision {
                namespace: target.namespace.clone(),
                revision,
            },
            created_at: Utc::now(),
        })
    }

    async fn rollback(&self, deployment: &Deployment, snapshot: &Snapshot) -> Result<()> {
        if let SnapshotType::K8sRevision { namespace, revision } = &snapshot.snapshot_type {
            let target = deployment.target.as_k8s()?;
            if let Some(helm) = &target.helm {
                // helm rollback
                self.helm_rollback(&helm.release_name, revision, namespace).await?;
            } else {
                // kubectl rollout undo
                self.kubectl_rollout_undo(deployment, namespace, revision).await?;
            }
        }
        Ok(())
    }
}
```

### 4.5 WeChat Mini Program Deployer

Deploys WeChat Mini Programs via the ci API:

```rust
pub struct WeChatMiniProgramDeployer;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeChatTarget {
    /// WeChat App ID
    pub app_id: String,
    /// Upload key path (from key vault)
    pub upload_key: String,
    /// Project path
    pub project_path: String,
    /// Version string
    pub version: String,
    /// Description for this version
    pub description: String,
    /// Whether to auto-submit for review
    pub auto_submit_review: bool,
    /// Node.js path for miniprogram-ci
    pub node_path: Option<String>,
}

#[async_trait]
impl Deployer for WeChatMiniProgramDeployer {
    async fn deploy(&self, deployment: &Deployment) -> Result<(Vec<String>, Vec<ArtifactRef>)> {
        let target = deployment.target.as_wechat()?;
        let mut logs = vec![];

        // Use miniprogram-ci npm package to upload
        logs.push(format!(
            "Uploading WeChat Mini Program v{}: {}",
            target.version, target.description
        ));

        let output = self.run_miniprogram_ci(
            "upload",
            target,
            deployment,
        ).await?;
        logs.extend(output);

        if target.auto_submit_review {
            logs.push("Submitting for WeChat review...".into());
            let output = self.submit_for_review(target).await?;
            logs.extend(output);
        }

        Ok((logs, vec![]))
    }

    async fn snapshot(&self, deployment: &Deployment) -> Result<Snapshot> {
        // WeChat keeps its own version history
        let target = deployment.target.as_wechat()?;
        Ok(Snapshot {
            id: Uuid::new_v4(),
            snapshot_type: SnapshotType::WeChatVersion {
                app_id: target.app_id.clone(),
                version: target.version.clone(),
            },
            created_at: Utc::now(),
        })
    }

    async fn rollback(&self, deployment: &Deployment, snapshot: &Snapshot) -> Result<()> {
        // WeChat rollback = resubmit previous version
        // This requires manual approval on WeChat side, so we notify the owner
        if let SnapshotType::WeChatVersion { app_id, version } = &snapshot.snapshot_type {
            tracing::warn!(
                "WeChat Mini Program rollback requires manual action. \
                 Previous version: {} for app {}",
                version, app_id
            );
        }
        Ok(())
    }
}
```

### 4.6 Custom Deployer

For user-defined deployment via shell commands:

```rust
pub struct CustomDeployer;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomTarget {
    /// Build command
    pub build_command: Option<String>,
    /// Test command
    pub test_command: Option<String>,
    /// Package command
    pub package_command: Option<String>,
    /// Pre-check command
    pub pre_check_command: Option<String>,
    /// Deploy command
    pub deploy_command: String,
    /// Rollback command
    pub rollback_command: Option<String>,
    /// Health check command (exit 0 = healthy)
    pub health_check_command: Option<String>,
    /// Working directory for commands
    pub working_dir: Option<String>,
    /// Environment variables
    pub env: HashMap<String, String>,
    /// Timeout per command
    pub command_timeout: Option<Duration>,
}
```

## 5. Rollback Manager

### 5.1 Snapshot System

Snapshots capture the pre-deployment state so we can revert if anything goes wrong:

```rust
pub type SnapshotId = Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: SnapshotId,
    pub snapshot_type: SnapshotType,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SnapshotType {
    /// Docker container committed to image
    DockerImage(String),
    /// Tarball on remote server
    RemoteTarball { host: String, path: String },
    /// Vercel deployment URL
    VercelDeployment { deployment_url: String },
    /// Kubernetes revision number
    K8sRevision { namespace: String, revision: String },
    /// WeChat Mini Program version
    WeChatVersion { app_id: String, version: String },
    /// Git commit SHA (for simple file-based rollback)
    GitRef(String),
    /// Local directory backup
    LocalBackup { path: String },
}

pub struct RollbackManager {
    deployer_registry: DeployerRegistry,
    db: PgPool,
}

impl RollbackManager {
    /// Create a snapshot before deployment
    pub async fn create_snapshot(&self, deployment: &Deployment) -> Result<SnapshotId> {
        let deployer = self.deployer_registry.get(&deployment.target)?;
        let snapshot = deployer.snapshot(deployment).await?;

        // Persist snapshot to database
        sqlx::query!(
            r#"
            INSERT INTO deployment_snapshots (id, deployment_id, snapshot_type, snapshot_data, created_at)
            VALUES ($1, $2, $3, $4, $5)
            "#,
            snapshot.id,
            deployment.id,
            format!("{:?}", snapshot.snapshot_type),
            serde_json::to_value(&snapshot.snapshot_type)?,
            snapshot.created_at,
        )
        .execute(&self.db)
        .await?;

        Ok(snapshot.id)
    }

    /// Rollback a deployment to its snapshot
    pub async fn rollback(
        &self,
        deployment: &Deployment,
        snapshot_id: &SnapshotId,
    ) -> Result<()> {
        let row = sqlx::query!(
            "SELECT snapshot_data FROM deployment_snapshots WHERE id = $1",
            snapshot_id,
        )
        .fetch_one(&self.db)
        .await?;

        let snapshot_type: SnapshotType = serde_json::from_value(row.snapshot_data)?;
        let snapshot = Snapshot {
            id: *snapshot_id,
            snapshot_type,
            created_at: Utc::now(), // not critical for rollback
        };

        let deployer = self.deployer_registry.get(&deployment.target)?;
        deployer.rollback(deployment, &snapshot).await?;

        // Record rollback in history
        sqlx::query!(
            r#"
            UPDATE deployments
            SET status = 'rolled_back', rolled_back = true, finished_at = NOW()
            WHERE id = $1
            "#,
            deployment.id,
        )
        .execute(&self.db)
        .await?;

        Ok(())
    }

    /// Cleanup old snapshots (keep last N per project)
    pub async fn cleanup_old_snapshots(
        &self,
        project_id: &ProjectId,
        keep: usize,
    ) -> Result<u64> {
        let deleted = sqlx::query_scalar!(
            r#"
            WITH ranked AS (
                SELECT ds.id, ROW_NUMBER() OVER (ORDER BY ds.created_at DESC) as rn
                FROM deployment_snapshots ds
                JOIN deployments d ON ds.deployment_id = d.id
                WHERE d.project_id = $1
            )
            DELETE FROM deployment_snapshots
            WHERE id IN (SELECT id FROM ranked WHERE rn > $2)
            "#,
            project_id,
            keep as i64,
        )
        .execute(&self.db)
        .await?
        .rows_affected();

        Ok(deleted)
    }
}
```

## 6. Health Checker

### 6.1 Health Check Types

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckConfig {
    /// List of health checks to run
    pub checks: Vec<HealthCheck>,
    /// How long to wait before starting checks (let service boot)
    pub initial_delay: Duration,
    /// Interval between retry attempts
    pub interval: Duration,
    /// Number of consecutive successes required
    pub success_threshold: u32,
    /// Number of consecutive failures before declaring unhealthy
    pub failure_threshold: u32,
    /// Total timeout for all health checks
    pub timeout: Duration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HealthCheck {
    /// HTTP GET probe — expects 2xx response
    Http {
        url: String,
        expected_status: Option<u16>,
        expected_body_contains: Option<String>,
        headers: HashMap<String, String>,
    },
    /// TCP connection probe — can we connect?
    Tcp {
        host: String,
        port: u16,
    },
    /// Run a command — exit 0 = healthy
    Command {
        command: String,
        args: Vec<String>,
    },
    /// Docker container health status
    DockerHealth {
        container_name: String,
    },
    /// Kubernetes pod readiness
    K8sReady {
        namespace: String,
        label_selector: String,
    },
}

pub struct HealthChecker {
    http_client: reqwest::Client,
}

impl HealthChecker {
    /// Run all configured health checks for a deployment
    pub async fn run_checks(&self, deployment: &Deployment) -> Result<Vec<String>> {
        let config = &deployment.config.health_check;
        let mut logs = vec![];

        // Wait for initial delay
        logs.push(format!(
            "Waiting {:?} before health checks...",
            config.initial_delay
        ));
        tokio::time::sleep(config.initial_delay).await;

        let deadline = Instant::now() + config.timeout;

        for check in &config.checks {
            let check_name = self.check_name(check);
            logs.push(format!("Running health check: {}", check_name));

            let mut consecutive_success = 0u32;
            let mut consecutive_failure = 0u32;

            loop {
                if Instant::now() >= deadline {
                    return Err(anyhow!(
                        "Health check '{}' timed out after {:?}",
                        check_name,
                        config.timeout
                    ));
                }

                match self.execute_check(check).await {
                    Ok(()) => {
                        consecutive_success += 1;
                        consecutive_failure = 0;
                        if consecutive_success >= config.success_threshold {
                            logs.push(format!("✓ {} passed ({} consecutive)", check_name, consecutive_success));
                            break;
                        }
                    }
                    Err(e) => {
                        consecutive_failure += 1;
                        consecutive_success = 0;
                        logs.push(format!(
                            "Health check '{}' attempt failed: {}",
                            check_name, e
                        ));
                        if consecutive_failure >= config.failure_threshold {
                            return Err(anyhow!(
                                "Health check '{}' failed {} consecutive times: {}",
                                check_name, consecutive_failure, e
                            ));
                        }
                    }
                }

                tokio::time::sleep(config.interval).await;
            }
        }

        logs.push("All health checks passed".into());
        Ok(logs)
    }

    /// Execute a single health check probe
    async fn execute_check(&self, check: &HealthCheck) -> Result<()> {
        match check {
            HealthCheck::Http {
                url,
                expected_status,
                expected_body_contains,
                headers,
            } => {
                let mut req = self.http_client.get(url);
                for (k, v) in headers {
                    req = req.header(k, v);
                }
                let resp = req
                    .timeout(Duration::from_secs(10))
                    .send()
                    .await?;

                let status = resp.status().as_u16();
                if let Some(expected) = expected_status {
                    if status != *expected {
                        return Err(anyhow!("Expected status {}, got {}", expected, status));
                    }
                } else if status >= 400 {
                    return Err(anyhow!("HTTP {} response", status));
                }

                if let Some(expected_body) = expected_body_contains {
                    let body = resp.text().await?;
                    if !body.contains(expected_body) {
                        return Err(anyhow!("Response body missing expected: {}", expected_body));
                    }
                }

                Ok(())
            }

            HealthCheck::Tcp { host, port } => {
                tokio::net::TcpStream::connect(format!("{}:{}", host, port))
                    .await
                    .map_err(|e| anyhow!("TCP connect failed: {}", e))?;
                Ok(())
            }

            HealthCheck::Command { command, args } => {
                let output = tokio::process::Command::new(command)
                    .args(args)
                    .output()
                    .await?;
                if output.status.success() {
                    Ok(())
                } else {
                    Err(anyhow!(
                        "Command exited with {}: {}",
                        output.status,
                        String::from_utf8_lossy(&output.stderr)
                    ))
                }
            }

            HealthCheck::DockerHealth { container_name } => {
                let output = tokio::process::Command::new("docker")
                    .args(["inspect", "--format", "{{.State.Health.Status}}", container_name])
                    .output()
                    .await?;
                let status = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if status == "healthy" {
                    Ok(())
                } else {
                    Err(anyhow!("Docker health status: {}", status))
                }
            }

            HealthCheck::K8sReady { namespace, label_selector } => {
                let output = tokio::process::Command::new("kubectl")
                    .args([
                        "wait", "--for=condition=ready", "pod",
                        "-l", label_selector,
                        "-n", namespace,
                        "--timeout=10s",
                    ])
                    .output()
                    .await?;
                if output.status.success() {
                    Ok(())
                } else {
                    Err(anyhow!(
                        "Pods not ready: {}",
                        String::from_utf8_lossy(&output.stderr)
                    ))
                }
            }
        }
    }
}
```

## 7. Environment Manager

Manages deployment environments (dev, staging, production) per project:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum Environment {
    Development,
    Staging,
    Production,
    Custom(String),
}

/// Per-environment deployment configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentConfig {
    pub environment: Environment,
    /// Deploy target for this environment
    pub target: DeployTarget,
    /// Environment-specific variables
    pub env_vars: HashMap<String, String>,
    /// Whether approval is required
    pub require_approval: bool,
    /// Auto-deploy on push to specific branch
    pub auto_deploy_branch: Option<String>,
    /// Health check config
    pub health_check: HealthCheckConfig,
    /// Notification channels for deploy events
    pub notify_channels: Vec<NotifyChannel>,
    /// Max concurrent deployments (prevent overlapping deploys)
    pub max_concurrent: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NotifyChannel {
    Telegram,
    Discord { channel_id: String },
    Slack { channel: String },
    WebDashboard,
}

/// Manages all environments for a project
pub struct EnvironmentManager {
    db: PgPool,
}

impl EnvironmentManager {
    /// Get the deploy config for a specific environment
    pub async fn get_config(
        &self,
        project_id: &ProjectId,
        env: &Environment,
    ) -> Result<EnvironmentConfig> {
        let row = sqlx::query!(
            r#"
            SELECT config_data FROM environment_configs
            WHERE project_id = $1 AND environment = $2
            "#,
            project_id,
            env.as_str(),
        )
        .fetch_one(&self.db)
        .await?;

        Ok(serde_json::from_value(row.config_data)?)
    }

    /// Promote a deployment from one environment to another
    /// (e.g., staging → production using the same artifact)
    pub async fn promote(
        &self,
        deployment_id: &DeploymentId,
        target_env: &Environment,
    ) -> Result<DeploymentId> {
        let source = sqlx::query_as!(
            DeploymentRow,
            "SELECT * FROM deployments WHERE id = $1",
            deployment_id,
        )
        .fetch_one(&self.db)
        .await?;

        // Create new deployment targeting the promoted environment
        let new_id = Uuid::new_v4();
        let target_config = self.get_config(&source.project_id, target_env).await?;

        // The new deployment reuses the same git ref and artifacts
        // but targets a different environment
        Ok(new_id)
    }

    /// List all environments for a project
    pub async fn list_environments(
        &self,
        project_id: &ProjectId,
    ) -> Result<Vec<EnvironmentConfig>> {
        let rows = sqlx::query!(
            "SELECT config_data FROM environment_configs WHERE project_id = $1 ORDER BY environment",
            project_id,
        )
        .fetch_all(&self.db)
        .await?;

        rows.iter()
            .map(|r| Ok(serde_json::from_value(r.config_data.clone())?))
            .collect()
    }
}
```

## 8. Deployment Configuration

### 8.1 Deploy Config

Per-deployment configuration combining project settings and environment overrides:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployConfig {
    /// Whether this environment requires owner approval
    pub require_approval: bool,
    /// Timeout per stage
    pub stage_timeouts: HashMap<PipelineStage, Duration>,
    /// Health check settings
    pub health_check: HealthCheckConfig,
    /// Number of snapshots to keep for rollback
    pub snapshot_retention: usize,
    /// Auto-rollback on health check failure
    pub auto_rollback: bool,
    /// Notification settings
    pub notifications: DeployNotifications,
    /// Whether to run tests as part of the pipeline
    pub run_tests: bool,
    /// Custom build arguments / environment overrides
    pub build_args: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployNotifications {
    /// Notify on deploy start
    pub on_start: bool,
    /// Notify on each stage completion
    pub on_stage: bool,
    /// Notify on success
    pub on_success: bool,
    /// Notify on failure (always true, non-configurable)
    pub on_failure: bool,
    /// Notify on rollback
    pub on_rollback: bool,
}
```

### 8.2 Deployer Registry

Routes deployments to the correct deployer implementation:

```rust
pub struct DeployerRegistry {
    deployers: HashMap<DeployTargetType, Arc<dyn Deployer>>,
}

impl DeployerRegistry {
    pub fn new() -> Self {
        let mut deployers: HashMap<DeployTargetType, Arc<dyn Deployer>> = HashMap::new();

        deployers.insert(DeployTargetType::Docker, Arc::new(DockerDeployer::new()));
        deployers.insert(DeployTargetType::Vps, Arc::new(VpsDeployer::new()));
        deployers.insert(DeployTargetType::Vercel, Arc::new(VercelDeployer::new()));
        deployers.insert(DeployTargetType::Kubernetes, Arc::new(K8sDeployer::new()));
        deployers.insert(
            DeployTargetType::WeChatMiniProgram,
            Arc::new(WeChatMiniProgramDeployer),
        );
        deployers.insert(DeployTargetType::Custom, Arc::new(CustomDeployer));

        Self { deployers }
    }

    pub fn get(&self, target: &DeployTarget) -> Result<&dyn Deployer> {
        let target_type = target.target_type();
        self.deployers
            .get(&target_type)
            .map(|d| d.as_ref())
            .ok_or_else(|| anyhow!("No deployer registered for {:?}", target_type))
    }
}
```

## 9. Approval Gate

Production deployments require owner approval by default:

```rust
pub struct ApprovalGate {
    event_bus: EventBusTx,
    pending_approvals: Arc<DashMap<DeploymentId, oneshot::Sender<ApprovalDecision>>>,
}

#[derive(Debug, Clone)]
pub enum ApprovalDecision {
    Approved,
    Rejected { reason: String },
    TimedOut,
}

impl ApprovalGate {
    /// Request approval from the owner via social gateway
    pub async fn request_approval(
        &self,
        deployment: &Deployment,
        timeout: Duration,
    ) -> Result<bool> {
        let (tx, rx) = oneshot::channel();

        // Store the sender for this deployment
        self.pending_approvals.insert(deployment.id, tx);

        // Send approval request via social gateway
        self.event_bus.send(ArgusEvent::DeployAwaitingApproval(
            deployment.id,
            deployment.project_id,
        ));

        // Wait for response with timeout
        let decision = tokio::time::timeout(timeout, rx)
            .await
            .unwrap_or(Ok(ApprovalDecision::TimedOut))
            .unwrap_or(ApprovalDecision::TimedOut);

        self.pending_approvals.remove(&deployment.id);

        match decision {
            ApprovalDecision::Approved => Ok(true),
            ApprovalDecision::Rejected { reason } => {
                tracing::info!("Deployment {} rejected: {}", deployment.id, reason);
                Ok(false)
            }
            ApprovalDecision::TimedOut => {
                tracing::warn!("Deployment {} approval timed out", deployment.id);
                Ok(false) // Default to reject on timeout
            }
        }
    }

    /// Handle an incoming approval/rejection from the owner
    pub fn handle_decision(&self, deployment_id: DeploymentId, decision: ApprovalDecision) {
        if let Some((_, tx)) = self.pending_approvals.remove(&deployment_id) {
            let _ = tx.send(decision);
        }
    }
}
```

## 10. Deployment History

Track all deployments for audit, rollback reference, and analytics:

```rust
pub struct DeploymentHistory {
    db: PgPool,
}

impl DeploymentHistory {
    /// Record a completed deployment
    pub async fn record(&self, deployment: &Deployment) -> Result<()> {
        sqlx::query!(
            r#"
            INSERT INTO deployments (
                id, project_id, environment, target_type, trigger_type,
                git_ref, git_branch, status, stages_json, snapshot_id,
                config_json, created_at, finished_at, rolled_back, metadata
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
            "#,
            deployment.id,
            deployment.project_id,
            deployment.environment.as_str(),
            deployment.target.target_type_str(),
            serde_json::to_value(&deployment.trigger)?,
            deployment.git_ref,
            deployment.git_branch,
            deployment.status.as_str(),
            serde_json::to_value(&deployment.stages)?,
            deployment.snapshot_id,
            serde_json::to_value(&deployment.config)?,
            deployment.created_at,
            deployment.finished_at,
            deployment.rolled_back,
            serde_json::to_value(&deployment.metadata)?,
        )
        .execute(&self.db)
        .await?;

        Ok(())
    }

    /// Get recent deployments for a project
    pub async fn recent(
        &self,
        project_id: &ProjectId,
        limit: i64,
    ) -> Result<Vec<Deployment>> {
        let rows = sqlx::query!(
            r#"
            SELECT * FROM deployments
            WHERE project_id = $1
            ORDER BY created_at DESC
            LIMIT $2
            "#,
            project_id,
            limit,
        )
        .fetch_all(&self.db)
        .await?;

        // TODO: Implement deserialization of deployment rows into Deployment structs
        // Query: SELECT id, project_id, environment, status, git_ref, created_at, ...
        //        FROM deployments WHERE project_id = $1 ORDER BY created_at DESC LIMIT $2
        // Map each row to Deployment { id, project_id, env, status, git_ref, timestamps, ... }
        Vec::new() // placeholder
    }

    /// Get the last successful deployment for an environment
    pub async fn last_successful(
        &self,
        project_id: &ProjectId,
        env: &Environment,
    ) -> Result<Option<Deployment>> {
        let row = sqlx::query!(
            r#"
            SELECT * FROM deployments
            WHERE project_id = $1 AND environment = $2 AND status = 'succeeded'
            ORDER BY created_at DESC
            LIMIT 1
            "#,
            project_id,
            env.as_str(),
        )
        .fetch_optional(&self.db)
        .await?;

        // TODO: Implement deserialization of the most recent successful deployment
        // Query: SELECT * FROM deployments WHERE project_id = $1 AND environment = $2
        //        AND status = 'succeeded' ORDER BY created_at DESC LIMIT 1
        // Map the optional row to Option<Deployment>, returning None if no successful deploy exists
        Ok(None) // placeholder
    }

    /// Deployment statistics for a project
    pub async fn stats(
        &self,
        project_id: &ProjectId,
        since: DateTime<Utc>,
    ) -> Result<DeploymentStats> {
        let stats = sqlx::query!(
            r#"
            SELECT
                COUNT(*) as total,
                COUNT(*) FILTER (WHERE status = 'succeeded') as succeeded,
                COUNT(*) FILTER (WHERE status LIKE 'failed%') as failed,
                COUNT(*) FILTER (WHERE rolled_back = true) as rolled_back,
                AVG(EXTRACT(EPOCH FROM (finished_at - created_at)))
                    FILTER (WHERE finished_at IS NOT NULL) as avg_duration_secs
            FROM deployments
            WHERE project_id = $1 AND created_at >= $2
            "#,
            project_id,
            since,
        )
        .fetch_one(&self.db)
        .await?;

        Ok(DeploymentStats {
            total: stats.total.unwrap_or(0) as u64,
            succeeded: stats.succeeded.unwrap_or(0) as u64,
            failed: stats.failed.unwrap_or(0) as u64,
            rolled_back: stats.rolled_back.unwrap_or(0) as u64,
            avg_duration: Duration::from_secs_f64(
                stats.avg_duration_secs.unwrap_or(0.0),
            ),
            success_rate: if stats.total.unwrap_or(0) > 0 {
                stats.succeeded.unwrap_or(0) as f64 / stats.total.unwrap_or(0) as f64
            } else {
                0.0
            },
        })
    }
}

#[derive(Debug, Clone)]
pub struct DeploymentStats {
    pub total: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub rolled_back: u64,
    pub avg_duration: Duration,
    pub success_rate: f64,
}
```

## 11. Auto-Deploy Triggers

Integrate with git operations to auto-deploy on branch push:

```rust
pub struct AutoDeployManager {
    env_manager: EnvironmentManager,
    pipeline_executor: PipelineExecutor,
    event_bus: EventBusTx,
}

impl AutoDeployManager {
    /// Listen for git events and trigger auto-deployments
    pub async fn handle_git_push(
        &self,
        project_id: ProjectId,
        branch: &str,
        commit_sha: &str,
    ) -> Result<()> {
        // Check all environments for auto-deploy config matching this branch
        let envs = self.env_manager.list_environments(&project_id).await?;

        for env_config in envs {
            if let Some(auto_branch) = &env_config.auto_deploy_branch {
                if auto_branch == branch {
                    tracing::info!(
                        "Auto-deploy triggered for {:?} on branch {} (commit {})",
                        env_config.environment,
                        branch,
                        &commit_sha[..8]
                    );

                    let mut deployment = Deployment::new(
                        project_id,
                        env_config.environment.clone(),
                        env_config.target.clone(),
                        DeployTrigger::Automatic {
                            task_id: Uuid::nil(), // auto-deploy, no specific task
                        },
                        commit_sha.to_string(),
                        branch.to_string(),
                        DeployConfig::from(&env_config),
                    );

                    // Execute pipeline asynchronously
                    let executor = self.pipeline_executor.clone();
                    tokio::spawn(async move {
                        if let Err(e) = executor.execute(&mut deployment).await {
                            tracing::error!(
                                "Auto-deploy failed for {:?}: {}",
                                deployment.environment,
                                e
                            );
                        }
                    });
                }
            }
        }

        Ok(())
    }
}
```

## 12. Artifact Store

Manages build artifacts (Docker images, tarballs, bundles) across deployments:

```rust
pub struct ArtifactStore {
    base_path: PathBuf,
    db: PgPool,
}

impl ArtifactStore {
    /// Store an artifact from a pipeline stage
    pub async fn store(
        &self,
        deployment_id: &DeploymentId,
        artifact: &ArtifactRef,
        data: &[u8],
    ) -> Result<()> {
        let dir = self.base_path
            .join(deployment_id.to_string());
        tokio::fs::create_dir_all(&dir).await?;

        let path = dir.join(&artifact.name);
        tokio::fs::write(&path, data).await?;

        sqlx::query!(
            r#"
            INSERT INTO deployment_artifacts (
                deployment_id, name, path, size_bytes, checksum_sha256, created_at
            ) VALUES ($1, $2, $3, $4, $5, NOW())
            "#,
            deployment_id,
            artifact.name,
            path.to_string_lossy().to_string(),
            artifact.size_bytes as i64,
            artifact.checksum_sha256,
        )
        .execute(&self.db)
        .await?;

        Ok(())
    }

    /// Retrieve an artifact
    pub async fn get(
        &self,
        deployment_id: &DeploymentId,
        name: &str,
    ) -> Result<Vec<u8>> {
        let row = sqlx::query!(
            "SELECT path FROM deployment_artifacts WHERE deployment_id = $1 AND name = $2",
            deployment_id,
            name,
        )
        .fetch_one(&self.db)
        .await?;

        Ok(tokio::fs::read(&row.path).await?)
    }

    /// Cleanup artifacts older than N days
    pub async fn cleanup(&self, max_age_days: i64) -> Result<u64> {
        let cutoff = Utc::now() - chrono::Duration::days(max_age_days);

        let rows = sqlx::query!(
            "SELECT path FROM deployment_artifacts WHERE created_at < $1",
            cutoff,
        )
        .fetch_all(&self.db)
        .await?;

        for row in &rows {
            let _ = tokio::fs::remove_file(&row.path).await;
        }

        let deleted = sqlx::query!(
            "DELETE FROM deployment_artifacts WHERE created_at < $1",
            cutoff,
        )
        .execute(&self.db)
        .await?
        .rows_affected();

        Ok(deleted)
    }
}
```

## 13. TOML Configuration

> **Note**: Configuration values shown below are system-managed defaults. In production, all configuration is managed through Argus's conversational interface (see `CONVERSATIONAL_CONFIG.md`). Direct TOML file editing is not required.

### 13.1 Global Deploy Settings

```toml
[deploy]
# Default settings for all projects
default_snapshot_retention = 5
artifact_max_age_days = 30
artifact_base_path = "/var/argus/artifacts"

# Default stage timeouts (seconds)
[deploy.stage_timeouts]
build = 600        # 10 minutes
test = 900         # 15 minutes
package = 300      # 5 minutes
pre_check = 60     # 1 minute
deploy = 600       # 10 minutes
health_check = 300 # 5 minutes

# Default health check
[deploy.default_health_check]
initial_delay = "10s"
interval = "5s"
success_threshold = 3
failure_threshold = 5
timeout = "120s"

# Default notifications
[deploy.notifications]
on_start = true
on_stage = false
on_success = true
on_failure = true  # always true
on_rollback = true
```

### 13.2 Per-Project Environment Configuration

```toml
# Example: my-web-app project
[[projects.environments]]
name = "development"
auto_deploy_branch = "develop"
require_approval = false

[projects.environments.target.docker]
dockerfile = "Dockerfile"
image_name = "my-web-app"
container_name = "my-web-app-dev"
ports = [{ host = 3001, container = 3000, protocol = "tcp" }]
restart_policy = "unless_stopped"

[projects.environments.target.docker.env]
NODE_ENV = "development"

[[projects.environments.health_checks]]
type = "http"
url = "http://localhost:3001/health"

# ---

[[projects.environments]]
name = "staging"
auto_deploy_branch = "main"
require_approval = false

[projects.environments.target.vps]
host = "staging.example.com"
port = 22
user = "deploy"
auth = { type = "key_from_vault", key_name = "staging_ssh_key" }
deploy_dir = "/opt/my-web-app"
use_rsync = true
exclude = ["node_modules", ".git", ".env"]
post_commands = ["npm install --production", "npm run migrate"]

[projects.environments.target.vps.service]
manager = "pm2"
name = "my-web-app"

[[projects.environments.health_checks]]
type = "http"
url = "https://staging.example.com/health"

# ---

[[projects.environments]]
name = "production"
require_approval = true

[projects.environments.target.vps]
host = "prod.example.com"
port = 22
user = "deploy"
auth = { type = "key_from_vault", key_name = "prod_ssh_key" }
deploy_dir = "/opt/my-web-app"
use_rsync = true
exclude = ["node_modules", ".git", ".env"]
pre_commands = ["echo 'Starting deployment...'"]
post_commands = ["npm install --production", "npm run migrate"]

[projects.environments.target.vps.service]
manager = "systemd"
name = "my-web-app"

[[projects.environments.health_checks]]
type = "http"
url = "https://app.example.com/health"
expected_status = 200

[[projects.environments.health_checks]]
type = "tcp"
host = "prod.example.com"
port = 443
```

### 13.3 Vercel Project Example

```toml
[[projects.environments]]
name = "production"
require_approval = false
auto_deploy_branch = "main"

[projects.environments.target.vercel]
token_key = "vercel_token"
project_id = "prj_xxxx"
framework = "next"

[[projects.environments.health_checks]]
type = "http"
url = "https://my-app.vercel.app/api/health"
```

### 13.4 Kubernetes Project Example

```toml
[[projects.environments]]
name = "production"
require_approval = true

[projects.environments.target.kubernetes]
kubeconfig = { type = "context", name = "prod-cluster" }
namespace = "my-app"
manifests = ["k8s/deployment.yaml", "k8s/service.yaml", "k8s/ingress.yaml"]

[projects.environments.target.kubernetes.strategy]
type = "rolling_update"
max_unavailable = "25%"
max_surge = "25%"

[projects.environments.target.kubernetes.registry]
url = "registry.example.com"
username_key = "registry_user"
password_key = "registry_pass"
```

## 14. Conversational Deployment Management

All deployment configuration and operations happen through natural language:

### 14.1 Setting Up Deployment

```
Owner: 帮我把 my-web-app 部署到我的服务器上

Argus: 好的，我来帮你配置部署。我需要了解几个信息：

1. 你的服务器地址是什么？
2. 用什么方式部署？(Docker 容器 / 直接部署到 VPS / Vercel / K8s)
3. 需要几个环境？(开发 / 预发布 / 生产)

Owner: 服务器是 prod.example.com，SSH端口22，用Docker部署。
       需要开发和生产两个环境。

Argus: 明白了。我已配置：

📋 **my-web-app 部署配置**
┌─────────┬──────────────────────────────────────┐
│ 环境     │ 配置                                  │
├─────────┼──────────────────────────────────────┤
│ 开发     │ 本地 Docker, 端口 3001                │
│         │ 自动部署: develop 分支                  │
│         │ 无需审批                               │
├─────────┼──────────────────────────────────────┤
│ 生产     │ prod.example.com Docker               │
│         │ 需要你的审批才能部署                     │
│         │ 自动回滚：健康检查失败时                  │
└─────────┴──────────────────────────────────────┘

SSH 密钥需要提前配置好，发给我或者告诉我路径。
```

### 14.2 Triggering a Deployment

```
Owner: 把 my-web-app 部署到生产

Argus: 收到。正在准备 my-web-app 的生产部署。

📦 **部署流水线启动**
├─ Build    ✓ (12s)
├─ Test     ✓ 48/48 通过 (34s)
├─ Package  ✓ Docker image: my-web-app:a3f8c2 (8s)
├─ PreCheck ✓ 已创建快照 (2s)
├─ ⏳ **等待审批**
│
│  commit: a3f8c2d "feat: add payment module"
│  目标: prod.example.com
│
│  [✅ 批准]  [❌ 拒绝]

Owner: ✅ 批准

Argus:
├─ Deploy   ✓ 容器已更新 (15s)
├─ Health   ✓ HTTP 200 / TCP 443 (20s)
└─ **部署成功** ✓

🟢 my-web-app 已成功部署到生产环境
   版本: a3f8c2d
   耗时: 1分31秒
   URL: https://app.example.com
```

### 14.3 Rollback

```
Owner: 生产有问题，回滚 my-web-app

Argus: 立即回滚。

⏪ **回滚 my-web-app 生产环境**
├─ 停止当前容器 ✓
├─ 恢复到上一版本快照 ✓
│  (快照: 2026-02-09 14:30:22, commit b7e1f9a)
├─ 启动旧版本容器 ✓
├─ 健康检查 ✓
└─ **回滚完成** ✓

🟡 my-web-app 已回滚到版本 b7e1f9a
   耗时: 18秒
   请告诉我问题细节，我来排查。
```

### 14.4 Deployment Status

```
Owner: my-web-app 部署情况怎么样

Argus: 📊 **my-web-app 部署统计** (过去30天)

┌──────────────────────────────────┐
│ 总部署次数: 23                    │
│ 成功: 21 (91.3%)                 │
│ 失败: 2 (已自动回滚)              │
│ 平均耗时: 1分45秒                 │
│                                   │
│ 当前版本:                         │
│   开发: a3f8c2d (2小时前)         │
│   生产: b7e1f9a (回滚, 30分钟前)   │
└──────────────────────────────────┘
```

## 15. Event Integration

The Deploy Engine emits and listens for the following events on the event bus:

| Event | Direction | Description |
|-------|-----------|-------------|
| `DeployStarted(ProjectId, Environment)` | Emits | Pipeline begins |
| `DeployStageCompleted(ProjectId, String)` | Emits | A stage finished |
| `DeployAwaitingApproval(DeploymentId, ProjectId)` | Emits | Blocked on approval |
| `DeployApproved(DeploymentId)` | Listens | Owner approved |
| `DeployRejected(DeploymentId, String)` | Listens | Owner rejected |
| `DeploySucceeded(ProjectId, DeploymentInfo)` | Emits | Deployment complete |
| `DeployFailed(ProjectId, String)` | Emits | Deployment failed |
| `DeployRolledBack(ProjectId)` | Emits | Rollback complete |
| `GitPushCompleted(ProjectId, String, String)` | Listens | Triggers auto-deploy check |
| `TaskCompleted(TaskId)` | Listens | May trigger deployment task |

## 16. Performance Targets

| Metric | Target |
|--------|--------|
| Pipeline overhead (excluding build/test) | < 30 seconds |
| Rollback completion | < 60 seconds |
| Health check total time | < 2 minutes |
| Snapshot creation | < 30 seconds |
| Concurrent deployments (different projects) | Up to 10 |
| Deployment history query | < 100ms |
| Auto-deploy trigger latency | < 5 seconds after git push |
