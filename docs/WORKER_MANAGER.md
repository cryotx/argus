# Worker Manager — Detailed Design

The Worker Manager controls a pool of AI coding CLI subprocesses that execute tasks. Each worker is an isolated instance of an AI coding tool (OpenCode, Claude Code, Aider, or a generic CLI) running in a sandbox.

## 1. Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                     Worker Manager                               │
│                                                                  │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │                  Worker Pool                                │ │
│  │  ┌─────────┐ ┌─────────┐ ┌─────────┐ ┌─────────┐         │ │
│  │  │Worker 1 │ │Worker 2 │ │Worker 3 │ │Worker N │         │ │
│  │  │OpenCode │ │Claude   │ │Aider    │ │Generic  │         │ │
│  │  │Idle     │ │Busy:T42 │ │Busy:T17 │ │Idle     │         │ │
│  │  └─────────┘ └─────────┘ └─────────┘ └─────────┘         │ │
│  └────────────────────────────────────────────────────────────┘ │
│                                                                  │
│  ┌──────────────┐  ┌──────────────┐  ┌────────────────────────┐│
│  │Worker        │  │Health        │  │Worker Selector         ││
│  │Lifecycle     │  │Monitor       │  │(smart assignment)      ││
│  │(spawn/kill)  │  │(heartbeat)   │  │                        ││
│  └──────────────┘  └──────────────┘  └────────────────────────┘│
│                                                                  │
│  ┌──────────────┐  ┌──────────────┐  ┌────────────────────────┐│
│  │Sandbox       │  │Resource      │  │Protocol                ││
│  │Manager       │  │Limiter       │  │(communication layer)   ││
│  │(dir/docker)  │  │(cpu/mem/disk)│  │                        ││
│  └──────────────┘  └──────────────┘  └────────────────────────┘│
└─────────────────────────────────────────────────────────────────┘
```

## 2. Worker Types

### 2.1 Driver Trait

All worker types implement a unified driver interface:

```rust
#[async_trait]
trait WorkerDriver: Send + Sync {
    /// Get the worker type name
    fn worker_type(&self) -> WorkerType;
    
    /// Check if this worker type is available on the system
    async fn is_available(&self) -> bool;
    
    /// Spawn a new worker process
    async fn spawn(&self, config: &WorkerConfig) -> Result<WorkerHandle>;
    
    /// Send a task to the worker
    async fn send_task(&self, handle: &mut WorkerHandle, task: &TaskPayload) -> Result<()>;
    
    /// Read output from the worker (streaming)
    async fn read_output(&self, handle: &mut WorkerHandle) -> Result<WorkerOutput>;
    
    /// Check if worker is still alive
    async fn is_alive(&self, handle: &WorkerHandle) -> bool;
    
    /// Gracefully stop the worker
    async fn stop(&self, handle: &mut WorkerHandle) -> Result<()>;
    
    /// Force kill the worker
    async fn kill(&self, handle: &mut WorkerHandle) -> Result<()>;
    
    /// Get worker's resource usage
    async fn resource_usage(&self, handle: &WorkerHandle) -> Result<ResourceUsage>;
}

enum WorkerType {
    OpenCode,
    ClaudeCode,
    Aider,
    GenericCli,
}

/// Collaboration capability tier — determines whether a worker type
/// can participate in role-based collaboration sessions.
/// See COLLABORATION.md Section 2.3 for full details.
enum WorkerCapability {
    /// Solo-only: no structured JSON output or system prompt control.
    SoloOnly,
    /// Supports role-based collaboration: structured output, system prompt
    /// injection, session persistence.
    CollaborationCapable,
}

impl WorkerType {
    fn capability(&self) -> WorkerCapability {
        match self {
            WorkerType::OpenCode => WorkerCapability::CollaborationCapable,
            WorkerType::ClaudeCode => WorkerCapability::CollaborationCapable,
            WorkerType::Aider => WorkerCapability::SoloOnly,
            WorkerType::GenericCli => WorkerCapability::SoloOnly,
        }
    }
}
```

### 2.2 OpenCode Driver

```rust
struct OpenCodeDriver {
    binary_path: PathBuf,    // Path to opencode binary
    default_model: String,   // e.g., "anthropic/claude-sonnet-4-20250514"
}

#[async_trait]
impl WorkerDriver for OpenCodeDriver {
    fn worker_type(&self) -> WorkerType {
        WorkerType::OpenCode
    }
    
    async fn is_available(&self) -> bool {
        // Check if opencode binary exists and is executable
        tokio::fs::metadata(&self.binary_path).await.is_ok()
    }
    
    async fn spawn(&self, config: &WorkerConfig) -> Result<WorkerHandle> {
        let child = tokio::process::Command::new(&self.binary_path)
            .current_dir(&config.workspace_dir)
            .env("OPENCODE_MODEL", &config.model_override.unwrap_or(&self.default_model))
            .env("OPENCODE_AUTO_APPROVE", "true")  // Non-interactive mode
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        
        Ok(WorkerHandle {
            pid: child.id().unwrap(),
            process: child,
            started_at: Utc::now(),
            worker_type: WorkerType::OpenCode,
        })
    }
    
    async fn send_task(&self, handle: &mut WorkerHandle, task: &TaskPayload) -> Result<()> {
        let prompt = format!(
            "{}\n\n## Acceptance Criteria\n{}\n\n## Test Commands\n{}",
            task.description,
            task.acceptance_criteria.join("\n"),
            task.test_commands.join("\n"),
        );
        
        // Write prompt to stdin
        let stdin = handle.process.stdin.as_mut().unwrap();
        stdin.write_all(prompt.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;
        
        Ok(())
    }
    
    async fn read_output(&self, handle: &mut WorkerHandle) -> Result<WorkerOutput> {
        let stdout = handle.process.stdout.as_mut().unwrap();
        let mut output = String::new();
        let mut reader = BufReader::new(stdout);
        
        // Read until worker signals completion or EOF
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line).await {
                Ok(0) => break, // EOF
                Ok(_) => {
                    output.push_str(&line);
                    // Check for completion marker
                    if line.contains("TASK_COMPLETE") || line.contains("TASK_FAILED") {
                        break;
                    }
                }
                Err(e) => return Err(e.into()),
            }
        }
        
        Ok(WorkerOutput {
            raw_output: output,
            exit_code: handle.process.try_wait()?.map(|s| s.code().unwrap_or(-1)),
        })
    }
}
```

### 2.3 Claude Code Driver

```rust
struct ClaudeCodeDriver {
    binary_path: PathBuf,    // Path to claude binary
}

#[async_trait]
impl WorkerDriver for ClaudeCodeDriver {
    async fn spawn(&self, config: &WorkerConfig) -> Result<WorkerHandle> {
        let child = tokio::process::Command::new(&self.binary_path)
            .arg("--print")              // Non-interactive, print output
            .arg("--dangerously-skip-permissions")  // Auto-approve all operations
            .current_dir(&config.workspace_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        
        Ok(WorkerHandle {
            pid: child.id().unwrap(),
            process: child,
            started_at: Utc::now(),
            worker_type: WorkerType::ClaudeCode,
        })
    }
    
    // ... similar to OpenCode driver ...
}
```

### 2.4 Aider Driver

```rust
struct AiderDriver {
    binary_path: PathBuf,
    default_model: String,
}

#[async_trait]
impl WorkerDriver for AiderDriver {
    async fn spawn(&self, config: &WorkerConfig) -> Result<WorkerHandle> {
        let child = tokio::process::Command::new(&self.binary_path)
            .arg("--yes-always")         // Auto-approve
            .arg("--no-auto-commits")    // Let Argus handle Git
            .arg("--model")
            .arg(&config.model_override.unwrap_or(&self.default_model))
            .current_dir(&config.workspace_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        
        Ok(WorkerHandle {
            pid: child.id().unwrap(),
            process: child,
            started_at: Utc::now(),
            worker_type: WorkerType::Aider,
        })
    }
}
```

### 2.5 Generic CLI Driver

For running arbitrary CLI commands (not AI coding tools):

```rust
struct GenericCliDriver;

#[async_trait]
impl WorkerDriver for GenericCliDriver {
    async fn spawn(&self, config: &WorkerConfig) -> Result<WorkerHandle> {
        let child = tokio::process::Command::new("bash")
            .arg("-c")
            .arg(&config.command)
            .current_dir(&config.workspace_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        
        Ok(WorkerHandle {
            pid: child.id().unwrap(),
            process: child,
            started_at: Utc::now(),
            worker_type: WorkerType::GenericCli,
        })
    }
}
```

### 2.6 Collaboration Capability & Role Prompt Injection

When the Collaboration Orchestrator assigns a role (Architect, Developer, Reviewer, Tester, SecurityAuditor) to a worker, the driver must inject a role-specific system prompt. Only `CollaborationCapable` tools support this:

**OpenCode** — Two injection methods:
- `opencode serve` HTTP API: start as HTTP server, send role prompt + task via `POST /api/message` with `format: json`
- `opencode run --agent <role-name> --format json`: pre-configure an Agent per role in the project's opencode config, invoke by name

**Claude Code** — First-class CLI support:
- `--system-prompt <role_prompt>` for role injection
- `--output-format json` for structured output parsing
- `--allowedTools <tool_list>` for per-role permission whitelisting (e.g., Reviewer gets read-only tools)
- `--session-id <id>` for multi-turn collaboration persistence

**Aider / GenericCli** — Not supported for collaboration (no system prompt control, no structured output). These tools are used only in Solo mode.

> See [COLLABORATION.md](./COLLABORATION.md) Sections 7.1 and 7.2 for full driver implementation examples with `spawn_with_role()`.

## 3. Worker Pool

```rust
struct WorkerPool {
    /// All workers (active + idle)
    workers: HashMap<Uuid, Worker>,
    
    /// Configuration
    max_workers: usize,                  // Default: 5
    max_workers_per_project: usize,      // Default: 3
    
    /// Available drivers
    drivers: HashMap<WorkerType, Box<dyn WorkerDriver>>,
    
    /// Metrics
    total_spawned: u64,
    total_tasks_executed: u64,
}

struct Worker {
    id: Uuid,
    worker_type: WorkerType,
    state: WorkerState,
    handle: WorkerHandle,
    
    // Assignment
    current_task_id: Option<Uuid>,
    current_project_id: Option<Uuid>,
    
    // Config
    config: WorkerConfig,
    sandbox: SandboxHandle,
    
    // Stats
    tasks_completed: u64,
    tasks_failed: u64,
    total_runtime: Duration,
    spawned_at: DateTime<Utc>,
    last_activity_at: DateTime<Utc>,
}

enum WorkerState {
    Spawning,
    Ready,
    Busy,
    Stopping,
    Crashed,
}

struct WorkerConfig {
    workspace_dir: PathBuf,
    model_override: Option<String>,
    command: Option<String>,            // For GenericCli
    timeout: Duration,                   // Default: 30 min
    resource_limits: ResourceLimits,
    sandbox_type: SandboxType,
    environment: HashMap<String, String>,
}
```

## 4. Worker Selector

Smart worker selection based on task requirements, worker history, and collaboration capability:

```rust
struct WorkerSelector;

impl WorkerSelector {
    /// Select the best worker type for a task
    async fn select_worker_type(&self, task: &Task, pool: &WorkerPool) -> WorkerType {
        // 1. If task specifies a worker type hint, use it
        if let Some(hint) = &task.worker_type_hint {
            if pool.drivers.contains_key(hint) && pool.drivers[hint].is_available().await {
                return hint.clone();
            }
        }
        
        // 2. Based on task type
        match task.task_type() {
            TaskType::CodeGeneration => {
                // Prefer OpenCode or Claude Code for code generation
                self.prefer_in_order(&[
                    WorkerType::OpenCode,
                    WorkerType::ClaudeCode,
                    WorkerType::Aider,
                ], pool)
            }
            TaskType::CodeReview => {
                // Use a different tool than the one that wrote the code
                self.select_for_review(task, pool)
            }
            TaskType::Testing => {
                // Generic CLI is fine for running tests
                WorkerType::GenericCli
            }
            TaskType::ShellCommand => {
                WorkerType::GenericCli
            }
            _ => WorkerType::OpenCode,
        }
    }
    
    /// Select a worker for review (different from the writer)
    fn select_for_review(&self, task: &Task, pool: &WorkerPool) -> WorkerType {
        let writer = task.metadata.get("written_by_worker")
            .and_then(|w| WorkerType::from_str(w).ok());
        
        match writer {
            Some(WorkerType::OpenCode) => WorkerType::ClaudeCode,
            Some(WorkerType::ClaudeCode) => WorkerType::OpenCode,
            Some(WorkerType::Aider) => WorkerType::OpenCode,
            _ => WorkerType::OpenCode,
        }
    }

    /// Select a worker for a collaboration role.
    /// Only considers CollaborationCapable tools (OpenCode, Claude Code).
    /// When force_different_model is true, ensures the reviewer's underlying
    /// LLM model differs from the developer's.
    fn select_for_collaboration_role(
        &self,
        role: &CollaborationRole,
        session: &CollaborationSession,
        pool: &WorkerPool,
    ) -> WorkerType {
        // Filter to collaboration-capable tools only
        let capable_types: Vec<WorkerType> = pool.drivers.keys()
            .filter(|t| t.capability() == WorkerCapability::CollaborationCapable)
            .cloned()
            .collect();

        // For reviewer/auditor roles with force_different_model,
        // ensure a different underlying LLM than the developer
        if session.review_policy.force_different_model
            && matches!(role, CollaborationRole::Reviewer | CollaborationRole::SecurityAuditor)
        {
            if let Some(dev_assignment) = session.role_assignments.get(&CollaborationRole::Developer) {
                // Pick a tool+model combination that differs from the developer's
                return self.select_different_model_worker(
                    &dev_assignment.model,
                    &capable_types,
                    pool,
                );
            }
        }

        // Default: prefer OpenCode, then Claude Code
        self.prefer_in_order(&capable_types, pool)
    }
    
    /// Select an existing idle worker or spawn a new one
    async fn get_or_spawn_worker(
        &self,
        pool: &mut WorkerPool,
        worker_type: WorkerType,
        project_id: Uuid,
        workspace_dir: &Path,
    ) -> Result<Uuid> {
        // 1. Try to reuse an idle worker of the same type in the same project
        if let Some(worker) = pool.find_idle_worker(worker_type, Some(project_id)) {
            return Ok(worker.id);
        }
        
        // 2. Try to reuse any idle worker of the same type
        if let Some(worker) = pool.find_idle_worker(worker_type, None) {
            // Reconfigure for new project
            worker.reconfigure(workspace_dir).await;
            return Ok(worker.id);
        }
        
        // 3. Check if we can spawn a new worker
        if pool.workers.len() < pool.max_workers {
            let worker_id = pool.spawn_worker(worker_type, workspace_dir).await?;
            return Ok(worker_id);
        }
        
        // 4. No capacity — return error (scheduler will wait)
        Err(Error::NoWorkerCapacity)
    }
}
```

> See [COLLABORATION.md](./COLLABORATION.md) Sections 2.3 and 12.2 for full details on capability tiers and Worker Manager integration.

## 5. Sandbox Isolation

### 5.1 Sandbox Types

```rust
enum SandboxType {
    /// Directory-level isolation (default)
    /// Worker can only access its assigned workspace directory
    Directory,
    
    /// Docker container isolation (higher security)
    /// Worker runs inside a Docker container with limited access
    Docker,
    
    /// No isolation (for trusted operations only)
    None,
}
```

### 5.2 Directory Sandbox

```rust
struct DirectorySandbox {
    workspace_dir: PathBuf,
    allowed_paths: Vec<PathBuf>,       // Additional allowed paths
}

impl DirectorySandbox {
    fn new(project_id: Uuid) -> Self {
        let workspace_dir = PathBuf::from(format!(
            "/var/argus/workspaces/{}",
            project_id
        ));
        
        Self {
            workspace_dir,
            allowed_paths: vec![
                PathBuf::from("/tmp"),
                PathBuf::from("/var/argus/shared"),  // Shared tools/configs
            ],
        }
    }
    
    /// Validate that a path is within the sandbox
    fn validate_path(&self, path: &Path) -> bool {
        path.starts_with(&self.workspace_dir) ||
        self.allowed_paths.iter().any(|p| path.starts_with(p))
    }
}
```

### 5.3 Docker Sandbox

```rust
struct DockerSandbox {
    container_id: Option<String>,
    image: String,                     // Default: "argus-worker:latest"
    workspace_mount: PathBuf,
    resource_limits: ResourceLimits,
}

impl DockerSandbox {
    async fn create(&mut self, config: &WorkerConfig) -> Result<()> {
        let options = bollard::container::Config {
            image: Some(self.image.clone()),
            host_config: Some(bollard::models::HostConfig {
                binds: Some(vec![
                    format!("{}:/workspace", self.workspace_mount.display()),
                ]),
                memory: Some(self.resource_limits.max_memory_bytes as i64),
                nano_cpus: Some((self.resource_limits.max_cpu_cores * 1e9) as i64),
                disk_quota: Some(self.resource_limits.max_disk_bytes as i64),
                network_mode: Some("bridge".into()),
                // Security options
                security_opt: Some(vec![
                    "no-new-privileges:true".into(),
                ]),
                // Drop all capabilities, add only needed ones
                cap_drop: Some(vec!["ALL".into()]),
                cap_add: Some(vec!["NET_RAW".into()]),  // For npm install etc.
                ..Default::default()
            }),
            working_dir: Some("/workspace".into()),
            user: Some("1000:1000".into()),  // Non-root
            ..Default::default()
        };
        
        let docker = bollard::Docker::connect_with_local_defaults()?;
        let container = docker
            .create_container::<String, String>(None, options)
            .await?;
        
        self.container_id = Some(container.id);
        docker.start_container::<String>(&container.id, None).await?;
        
        Ok(())
    }
    
    async fn destroy(&self) -> Result<()> {
        if let Some(id) = &self.container_id {
            let docker = bollard::Docker::connect_with_local_defaults()?;
            docker.stop_container(id, Some(bollard::container::StopContainerOptions {
                t: 10,  // 10 second grace period
            })).await?;
            docker.remove_container(id, None).await?;
        }
        Ok(())
    }
}
```

## 6. Resource Limits

```rust
struct ResourceLimits {
    /// Maximum memory in bytes (default: 2 GB)
    max_memory_bytes: u64,
    
    /// Maximum CPU cores (default: 2.0)
    max_cpu_cores: f64,
    
    /// Maximum disk space in bytes (default: 10 GB)
    max_disk_bytes: u64,
    
    /// Maximum execution time (default: 30 min)
    max_runtime: Duration,
    
    /// Maximum output size in bytes (default: 50 MB)
    max_output_bytes: u64,
    
    /// Network access allowed (default: true)
    network_enabled: bool,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_memory_bytes: 2 * 1024 * 1024 * 1024,   // 2 GB
            max_cpu_cores: 2.0,
            max_disk_bytes: 10 * 1024 * 1024 * 1024,    // 10 GB
            max_runtime: Duration::from_secs(30 * 60),    // 30 min
            max_output_bytes: 50 * 1024 * 1024,           // 50 MB
            network_enabled: true,
        }
    }
}

struct ResourceMonitor {
    poll_interval: Duration,   // Default: 5 seconds
}

impl ResourceMonitor {
    /// Monitor a worker's resource usage
    async fn monitor_worker(&self, worker: &Worker) -> Result<()> {
        let mut interval = tokio::time::interval(self.poll_interval);
        
        loop {
            interval.tick().await;
            
            let usage = self.get_process_usage(worker.handle.pid).await?;
            
            // Check memory
            if usage.memory_bytes > worker.config.resource_limits.max_memory_bytes {
                tracing::warn!(
                    "Worker {} exceeding memory limit: {} > {}",
                    worker.id, usage.memory_bytes, worker.config.resource_limits.max_memory_bytes
                );
                // Kill worker
                return Err(Error::ResourceExceeded("memory"));
            }
            
            // Check runtime
            let runtime = Utc::now() - worker.spawned_at;
            if runtime > worker.config.resource_limits.max_runtime {
                tracing::warn!("Worker {} exceeding runtime limit", worker.id);
                return Err(Error::ResourceExceeded("runtime"));
            }
        }
    }
    
    /// Get process resource usage from /proc
    async fn get_process_usage(&self, pid: u32) -> Result<ResourceUsage> {
        let stat = tokio::fs::read_to_string(format!("/proc/{}/stat", pid)).await?;
        let statm = tokio::fs::read_to_string(format!("/proc/{}/statm", pid)).await?;
        
        // Parse /proc/[pid]/stat for CPU
        // Parse /proc/[pid]/statm for memory
        // ...
        
        Ok(ResourceUsage {
            memory_bytes: 0,  // parsed from statm
            cpu_percent: 0.0, // calculated from stat
            disk_bytes: 0,    // from du or similar
        })
    }
}
```

## 7. Worker Lifecycle

```rust
impl WorkerPool {
    /// Spawn a new worker
    async fn spawn_worker(
        &mut self,
        worker_type: WorkerType,
        workspace_dir: &Path,
    ) -> Result<Uuid> {
        let worker_id = Uuid::new_v4();
        let driver = self.drivers.get(&worker_type)
            .ok_or(Error::DriverNotFound(worker_type))?;
        
        // 1. Build config
        let config = WorkerConfig {
            workspace_dir: workspace_dir.to_path_buf(),
            resource_limits: ResourceLimits::default(),
            sandbox_type: worker_type.default_sandbox(),
            ..Default::default()
        };
        
        // 2. Create sandbox
        let sandbox = match worker_type.default_sandbox() {
            SandboxType::Directory => SandboxHandle::Directory(
                DirectorySandbox::new_for_path(workspace_dir)
            ),
            SandboxType::Docker => {
                let mut docker_sandbox = DockerSandbox::new(workspace_dir);
                docker_sandbox.create(&config).await?;
                SandboxHandle::Docker(docker_sandbox)
            }
            SandboxType::None => SandboxHandle::None,
        };
        
        // 3. Spawn process
        let handle = driver.spawn(&config).await?;
        
        // 4. Register worker
        let worker = Worker {
            id: worker_id,
            worker_type,
            state: WorkerState::Ready,
            handle,
            current_task_id: None,
            current_project_id: None,
            config,
            sandbox,
            tasks_completed: 0,
            tasks_failed: 0,
            total_runtime: Duration::ZERO,
            spawned_at: Utc::now(),
            last_activity_at: Utc::now(),
        };
        
        self.workers.insert(worker_id, worker);
        self.total_spawned += 1;
        
        // 5. Start resource monitor
        let monitor = self.resource_monitor.clone();
        tokio::spawn(async move {
            if let Err(e) = monitor.monitor_worker(&worker).await {
                tracing::error!("Worker {} resource violation: {}", worker_id, e);
            }
        });
        
        self.event_bus.emit(ArgusEvent::WorkerSpawned(worker_id, worker_type)).await;
        
        Ok(worker_id)
    }
    
    /// Assign a task to a worker
    async fn assign_task(&mut self, worker_id: Uuid, task: &Task) -> Result<()> {
        let worker = self.workers.get_mut(&worker_id)
            .ok_or(Error::WorkerNotFound(worker_id))?;
        
        worker.state = WorkerState::Busy;
        worker.current_task_id = Some(task.id);
        worker.current_project_id = Some(task.project_id);
        worker.last_activity_at = Utc::now();
        
        let driver = &self.drivers[&worker.worker_type];
        
        let payload = TaskPayload {
            description: task.description.clone(),
            acceptance_criteria: task.acceptance_criteria.clone(),
            test_commands: task.test_commands.clone(),
            context: self.build_task_context(task).await,
        };
        
        driver.send_task(&mut worker.handle, &payload).await?;
        
        self.event_bus.emit(
            ArgusEvent::WorkerBusy(worker_id, task.id)
        ).await;
        
        Ok(())
    }
    
    /// Handle worker crash
    async fn handle_worker_crash(&mut self, worker_id: Uuid) {
        if let Some(worker) = self.workers.get_mut(&worker_id) {
            worker.state = WorkerState::Crashed;
            worker.tasks_failed += 1;
            
            // Report task failure if one was assigned
            if let Some(task_id) = worker.current_task_id {
                self.event_bus.emit(ArgusEvent::TaskFailed(
                    task_id,
                    TaskError {
                        error_type: TaskErrorType::WorkerCrashed,
                        message: format!("Worker {} ({:?}) crashed", worker_id, worker.worker_type),
                        ..Default::default()
                    },
                )).await;
            }
            
            // Cleanup sandbox
            worker.sandbox.cleanup().await;
            
            self.event_bus.emit(ArgusEvent::WorkerCrashed(worker_id, "Process died".into())).await;
            
            // Remove crashed worker
            self.workers.remove(&worker_id);
        }
    }
    
    /// Idle worker cleanup (kill workers idle too long)
    async fn cleanup_idle_workers(&mut self) {
        let idle_threshold = Duration::from_secs(300); // 5 minutes
        let now = Utc::now();
        
        let idle_workers: Vec<Uuid> = self.workers.iter()
            .filter(|(_, w)| w.state == WorkerState::Ready)
            .filter(|(_, w)| now - w.last_activity_at > idle_threshold)
            .map(|(id, _)| *id)
            .collect();
        
        for worker_id in idle_workers {
            if self.workers.len() > 1 { // Keep at least one worker alive
                tracing::info!("Killing idle worker {}", worker_id);
                self.stop_worker(worker_id).await;
            }
        }
    }
}
```

## 8. Health Monitor

```rust
struct WorkerHealthMonitor {
    check_interval: Duration,   // Default: 10 seconds
}

impl WorkerHealthMonitor {
    /// Periodic health check loop
    async fn run(&self, pool: Arc<Mutex<WorkerPool>>) {
        let mut interval = tokio::time::interval(self.check_interval);
        
        loop {
            interval.tick().await;
            
            let mut pool = pool.lock().await;
            let worker_ids: Vec<Uuid> = pool.workers.keys().cloned().collect();
            
            for worker_id in worker_ids {
                let worker = &pool.workers[&worker_id];
                let driver = &pool.drivers[&worker.worker_type];
                
                if !driver.is_alive(&worker.handle).await {
                    tracing::warn!("Worker {} is not alive, handling crash", worker_id);
                    pool.handle_worker_crash(worker_id).await;
                }
            }
        }
    }
}
```

## 9. Worker Communication Protocol

Workers communicate through a simple protocol over stdin/stdout:

```rust
/// Message sent to worker
struct TaskPayload {
    /// The main task description / prompt
    description: String,
    
    /// What must be true when the task is done
    acceptance_criteria: Vec<String>,
    
    /// Commands to run to verify success
    test_commands: Vec<String>,
    
    /// Additional context from memory
    context: String,
    
    /// Files that should be modified (hints)
    target_files: Vec<String>,
}

/// Message received from worker
struct WorkerOutput {
    /// Raw stdout output
    raw_output: String,
    
    /// Process exit code (if exited)
    exit_code: Option<i32>,
}

/// Parsed worker result
enum WorkerResult {
    Success {
        files_changed: Vec<String>,
        summary: String,
    },
    Failure {
        error: String,
        partial_changes: Vec<String>,
    },
    NeedsInput {
        question: String,
    },
}
```

## 10. Worker Statistics

```rust
struct WorkerStats {
    // Pool metrics
    total_workers: usize,
    idle_workers: usize,
    busy_workers: usize,
    
    // Performance
    avg_task_duration: Duration,
    success_rate: f64,              // 0.0 ~ 1.0
    
    // By worker type
    per_type: HashMap<WorkerType, WorkerTypeStats>,
    
    // Resource usage
    total_memory_used: u64,
    total_cpu_percent: f64,
}

struct WorkerTypeStats {
    worker_type: WorkerType,
    total_tasks: u64,
    success_rate: f64,
    avg_duration: Duration,
    avg_tokens_per_task: u64,
}
```

## 11. Configuration

```toml
# config/workers.toml

[pool]
max_workers = 5
max_workers_per_project = 3
idle_timeout_secs = 300
default_sandbox = "directory"    # "directory" | "docker" | "none"

[drivers.opencode]
enabled = true
binary_path = "/usr/local/bin/opencode"
default_model = "anthropic/claude-sonnet-4-20250514"
priority = 1

[drivers.claude_code]
enabled = true
binary_path = "/usr/local/bin/claude"
priority = 2

[drivers.aider]
enabled = true
binary_path = "/usr/local/bin/aider"
default_model = "deepseek/deepseek-chat"
priority = 3

[drivers.generic_cli]
enabled = true
priority = 10

[resource_limits]
max_memory_gb = 2
max_cpu_cores = 2.0
max_disk_gb = 10
max_runtime_minutes = 30
max_output_mb = 50
network_enabled = true

[docker]
image = "argus-worker:latest"
network = "argus-net"
```

## 12. Conversational Management

```
User: "现在有几个worker在跑？"
Argus: "当前Worker状态:
        总数: 3/5
        ├── Worker-1 (OpenCode) — 忙碌: 处理项目A的支付集成 (已运行8分钟)
        ├── Worker-2 (ClaudeCode) — 忙碌: 处理项目B的用户认证 (已运行3分钟)
        └── Worker-3 (OpenCode) — 空闲 (等待任务)
        
        今日统计: 完成12个任务, 失败1个, 成功率92%"

User: "把最大worker数改成8"
Argus: "已更新: max_workers 5 → 8
        当前服务器资源充足，可以支持8个并发worker。"

User: "别用Aider了，它老是出问题"
Argus: "已禁用 Aider driver。
        当前可用: OpenCode (优先) → ClaudeCode
        Aider的2个正在执行的任务将在完成后不再分配新任务。"
```
