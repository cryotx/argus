# Security — Detailed Design

> **Crate**: `argus-security`

Security is foundational to Argus. As an autonomous agent with access to source code, servers, API keys, and deployment infrastructure, Argus must enforce strict security boundaries. This document covers the key vault, role-based access control, audit logging, worker sandbox enforcement, dangerous operation blocking, communication encryption, and third-party code sandboxing.

## 1. Architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│                      Security Engine                                  │
│                                                                       │
│  ┌──────────────┐  ┌──────────────┐  ┌────────────────────────────┐ │
│  │  Key Vault    │  │  RBAC        │  │  Audit Logger              │ │
│  │  (AES-256-GCM)│  │  Engine      │  │  (complete trail)          │ │
│  │               │  │              │  │                            │ │
│  └──────────────┘  └──────────────┘  └────────────────────────────┘ │
│                                                                       │
│  ┌──────────────┐  ┌──────────────┐  ┌────────────────────────────┐ │
│  │  Sandbox      │  │  Command     │  │  Network                   │ │
│  │  Enforcer     │  │  Guard       │  │  Policy                    │ │
│  │  (worker      │  │  (block      │  │  (TLS, firewall rules)     │ │
│  │   isolation)  │  │   dangerous) │  │                            │ │
│  └──────────────┘  └──────────────┘  └────────────────────────────┘ │
│                                                                       │
│  ┌──────────────┐  ┌──────────────┐  ┌────────────────────────────┐ │
│  │  Code         │  │  Secret      │  │  Rate                      │ │
│  │  Auditor      │  │  Scanner     │  │  Limiter                   │ │
│  │  (3rd party)  │  │  (leak       │  │  (abuse prevention)        │ │
│  │               │  │   detection) │  │                            │ │
│  └──────────────┘  └──────────────┘  └────────────────────────────┘ │
└──────────────────────────────────────────────────────────────────────┘
```

## 2. Key Vault

All secrets (API keys, SSH keys, tokens, passwords) are stored encrypted in PostgreSQL. Never in plaintext, never in config files, never in git.

### 2.1 Encryption Design

```rust
use aes_gcm::{Aead, Aes256Gcm, KeyInit, Nonce};
use argon2::{Argon2, password_hash::SaltString};
use rand::rngs::OsRng;
use zeroize::Zeroize;

/// The Key Vault stores all secrets encrypted with AES-256-GCM
pub struct KeyVault {
    db: PgPool,
    /// Master key derived from the owner's master password via Argon2id
    master_key: Aes256Gcm,
}

/// A stored secret
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultEntry {
    pub name: String,
    pub category: SecretCategory,
    pub encrypted_value: Vec<u8>,
    pub nonce: Vec<u8>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_accessed: Option<DateTime<Utc>>,
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SecretCategory {
    LlmApiKey { provider: String },
    SshKey { host: String },
    DeployToken { platform: String },
    DatabaseCredential { name: String },
    OAuthToken { service: String },
    GenericSecret,
}

impl KeyVault {
    /// Initialize vault from master password
    /// The master password is used to derive the encryption key via Argon2id
    pub fn new(master_password: &str, salt: &[u8], db: PgPool) -> Result<Self> {
        let mut key_bytes = [0u8; 32];

        // Argon2id with recommended parameters
        let argon2 = Argon2::new(
            argon2::Algorithm::Argon2id,
            argon2::Version::V0x13,
            argon2::Params::new(65536, 3, 4, Some(32))?, // 64MB, 3 iterations, 4 parallelism
        );
        argon2.hash_password_into(
            master_password.as_bytes(),
            salt,
            &mut key_bytes,
        )?;

        let cipher = Aes256Gcm::new_from_slice(&key_bytes)?;
        key_bytes.zeroize(); // Zero out key material from memory

        Ok(Self { db, master_key: cipher })
    }

    /// Store a secret in the vault
    pub async fn store(&self, name: &str, value: &str, category: SecretCategory) -> Result<()> {
        // Generate random nonce (96 bits for AES-GCM)
        let nonce_bytes: [u8; 12] = rand::random();
        let nonce = Nonce::from_slice(&nonce_bytes);

        // Encrypt
        let encrypted = self.master_key
            .encrypt(nonce, value.as_bytes())
            .map_err(|e| anyhow!("Encryption failed: {}", e))?;

        sqlx::query!(
            r#"
            INSERT INTO key_vault (name, category, encrypted_value, nonce, created_at, updated_at)
            VALUES ($1, $2, $3, $4, NOW(), NOW())
            ON CONFLICT (name) DO UPDATE
            SET encrypted_value = $3, nonce = $4, updated_at = NOW(),
                category = $2
            "#,
            name,
            serde_json::to_value(&category)?,
            &encrypted,
            &nonce_bytes[..],
        )
        .execute(&self.db)
        .await?;

        tracing::info!("Secret '{}' stored in vault", name);
        Ok(())
    }

    /// Retrieve a secret from the vault
    pub async fn get(&self, name: &str) -> Result<String> {
        let row = sqlx::query!(
            r#"
            SELECT encrypted_value, nonce FROM key_vault WHERE name = $1
            "#,
            name,
        )
        .fetch_optional(&self.db)
        .await?
        .ok_or_else(|| anyhow!("Secret '{}' not found in vault", name))?;

        let nonce = Nonce::from_slice(&row.nonce);
        let decrypted = self.master_key
            .decrypt(nonce, row.encrypted_value.as_ref())
            .map_err(|e| anyhow!("Decryption failed: {}", e))?;

        // Update last accessed timestamp
        sqlx::query!(
            "UPDATE key_vault SET last_accessed = NOW() WHERE name = $1",
            name,
        )
        .execute(&self.db)
        .await?;

        Ok(String::from_utf8(decrypted)?)
    }

    /// Delete a secret
    pub async fn delete(&self, name: &str) -> Result<bool> {
        let result = sqlx::query!(
            "DELETE FROM key_vault WHERE name = $1",
            name,
        )
        .execute(&self.db)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    /// List all secret names and categories (never values)
    pub async fn list(&self) -> Result<Vec<VaultEntrySummary>> {
        let rows = sqlx::query!(
            r#"
            SELECT name, category, created_at, updated_at, last_accessed
            FROM key_vault ORDER BY name
            "#,
        )
        .fetch_all(&self.db)
        .await?;

        Ok(rows.into_iter().map(|r| VaultEntrySummary {
            name: r.name,
            category: serde_json::from_value(r.category).unwrap_or(SecretCategory::GenericSecret),
            created_at: r.created_at,
            updated_at: r.updated_at,
            last_accessed: r.last_accessed,
        }).collect())
    }

    /// Rotate the master password
    /// Re-encrypts ALL secrets with the new key
    pub async fn rotate_master_password(
        &mut self,
        new_password: &str,
        salt: &[u8],
    ) -> Result<u64> {
        // First, decrypt all secrets with current key
        let all_rows = sqlx::query!(
            "SELECT name, encrypted_value, nonce, category FROM key_vault",
        )
        .fetch_all(&self.db)
        .await?;

        let mut decrypted_secrets: Vec<(String, String, serde_json::Value)> = vec![];
        for row in &all_rows {
            let nonce = Nonce::from_slice(&row.nonce);
            let plaintext = self.master_key
                .decrypt(nonce, row.encrypted_value.as_ref())
                .map_err(|e| anyhow!("Decryption failed for {}: {}", row.name, e))?;
            decrypted_secrets.push((
                row.name.clone(),
                String::from_utf8(plaintext)?,
                row.category.clone(),
            ));
        }

        // Derive new key
        let mut key_bytes = [0u8; 32];
        let argon2 = Argon2::new(
            argon2::Algorithm::Argon2id,
            argon2::Version::V0x13,
            argon2::Params::new(65536, 3, 4, Some(32))?,
        );
        argon2.hash_password_into(new_password.as_bytes(), salt, &mut key_bytes)?;
        self.master_key = Aes256Gcm::new_from_slice(&key_bytes)?;
        key_bytes.zeroize();

        // Re-encrypt all secrets with new key
        let mut tx = self.db.begin().await?;
        for (name, plaintext, category) in &decrypted_secrets {
            let nonce_bytes: [u8; 12] = rand::random();
            let nonce = Nonce::from_slice(&nonce_bytes);
            let encrypted = self.master_key
                .encrypt(nonce, plaintext.as_bytes())
                .map_err(|e| anyhow!("Re-encryption failed for {}: {}", name, e))?;

            sqlx::query!(
                r#"
                UPDATE key_vault
                SET encrypted_value = $2, nonce = $3, updated_at = NOW()
                WHERE name = $1
                "#,
                name,
                &encrypted,
                &nonce_bytes[..],
            )
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;

        Ok(decrypted_secrets.len() as u64)
    }
}

#[derive(Debug, Clone)]
pub struct VaultEntrySummary {
    pub name: String,
    pub category: SecretCategory,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_accessed: Option<DateTime<Utc>>,
}
```

### 2.2 Key Vault Rules

1. **Never log secret values** — only names and categories appear in logs
2. **Zeroize key material** — use `zeroize` crate to clear memory after use
3. **Master password not stored** — derived key lives in memory only, lost on restart (owner re-enters on startup)
4. **Nonce uniqueness** — each encryption uses a random 96-bit nonce
5. **Key rotation** — owner can change master password; all secrets are re-encrypted atomically

## 3. Role-Based Access Control (RBAC)

Argus supports multiple access levels for social platform interactions:

### 3.1 Roles

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    /// Full control — the primary owner
    Owner,
    /// Administrative access — can manage projects, workers, config
    Admin,
    /// Can view projects, trigger non-production deploys, query status
    Developer,
    /// Read-only access — can view status and receive notifications
    Viewer,
}

/// A user identity across platforms
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    pub id: Uuid,
    pub name: String,
    pub role: Role,
    pub platforms: Vec<PlatformIdentity>,
    pub created_at: DateTime<Utc>,
    pub last_active: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PlatformIdentity {
    Telegram { user_id: i64, username: Option<String> },
    Discord { user_id: u64, username: String },
    Slack { user_id: String },
    WeChat { openid: String, nickname: Option<String> },
    WebDashboard { email: String, password_hash: String },
}
```

### 3.2 Permission Matrix

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Permission {
    // Project operations
    CreateProject,
    DeleteProject,
    ViewProject,
    ModifyProject,

    // Task operations
    CreateTask,
    CancelTask,
    ViewTask,

    // Worker operations
    ManageWorkers,
    ViewWorkers,

    // Deployment
    DeployDev,
    DeployStaging,
    DeployProduction,
    Rollback,

    // Configuration
    ModifyConfig,
    ViewConfig,
    ManageKeys,

    // Security
    ManageRoles,
    ViewAuditLog,
    ManageSecurity,

    // MCP / Skills
    InstallMcp,
    InstallSkill,
    ManageTrust,

    // System
    SystemRestart,
    SelfEvolution,
    ViewSystemStatus,
}

pub struct RbacEngine {
    db: PgPool,
}

impl RbacEngine {
    /// Check if an identity has a specific permission
    pub fn has_permission(&self, identity: &Identity, perm: &Permission) -> bool {
        self.role_permissions(&identity.role).contains(perm)
    }

    /// Get all permissions for a role
    pub fn role_permissions(&self, role: &Role) -> HashSet<Permission> {
        match role {
            Role::Owner => {
                // Owner has ALL permissions
                self.all_permissions()
            }
            Role::Admin => {
                let mut perms = self.all_permissions();
                perms.remove(&Permission::SelfEvolution);
                perms.remove(&Permission::ManageSecurity);
                perms.remove(&Permission::ManageKeys);
                perms.remove(&Permission::SystemRestart);
                perms
            }
            Role::Developer => {
                let mut perms = HashSet::new();
                perms.insert(Permission::ViewProject);
                perms.insert(Permission::ViewTask);
                perms.insert(Permission::CreateTask);
                perms.insert(Permission::ViewWorkers);
                perms.insert(Permission::DeployDev);
                perms.insert(Permission::DeployStaging);
                perms.insert(Permission::ViewConfig);
                perms.insert(Permission::ViewSystemStatus);
                perms
            }
            Role::Viewer => {
                let mut perms = HashSet::new();
                perms.insert(Permission::ViewProject);
                perms.insert(Permission::ViewTask);
                perms.insert(Permission::ViewWorkers);
                perms.insert(Permission::ViewConfig);
                perms.insert(Permission::ViewSystemStatus);
                perms
            }
        }
    }

    /// Authenticate a user from a platform message
    pub async fn authenticate(
        &self,
        platform_id: &PlatformIdentity,
    ) -> Result<Option<Identity>> {
        // Look up identity by platform ID
        let row = match platform_id {
            PlatformIdentity::Telegram { user_id, .. } => {
                sqlx::query!(
                    r#"
                    SELECT i.* FROM identities i
                    JOIN identity_platforms ip ON i.id = ip.identity_id
                    WHERE ip.platform = 'telegram' AND ip.platform_user_id = $1
                    "#,
                    user_id.to_string(),
                )
                .fetch_optional(&self.db)
                .await?
            }
            // ... similar for other platforms
            _ => None,
        };

        // Deserialize into Identity
        Ok(row.map(|r| Identity {
            id: r.id,
            name: r.name,
            role: serde_json::from_value(r.role).unwrap_or(Role::Viewer),
            platforms: vec![platform_id.clone()],
            created_at: r.created_at,
            last_active: r.last_active,
        }))
    }

    /// Guard a specific action — returns error if not permitted
    pub async fn guard(
        &self,
        identity: &Identity,
        permission: Permission,
    ) -> Result<()> {
        if self.has_permission(identity, &permission) {
            Ok(())
        } else {
            Err(anyhow!(
                "Permission denied: {} does not have {:?} permission (role: {:?})",
                identity.name,
                permission,
                identity.role,
            ))
        }
    }
}
```

### 3.3 Permission Matrix Table

| Permission | Owner | Admin | Developer | Viewer |
|------------|-------|-------|-----------|--------|
| Create/Delete Project | Yes | Yes | No | No |
| View Project | Yes | Yes | Yes | Yes |
| Modify Project | Yes | Yes | No | No |
| Create/Cancel Task | Yes | Yes | Yes | No |
| View Task | Yes | Yes | Yes | Yes |
| Manage Workers | Yes | Yes | No | No |
| Deploy Dev | Yes | Yes | Yes | No |
| Deploy Staging | Yes | Yes | Yes | No |
| Deploy Production | Yes | Yes | No | No |
| Rollback | Yes | Yes | No | No |
| Modify Config | Yes | Yes | No | No |
| View Config | Yes | Yes | Yes | Yes |
| Manage Keys | Yes | No | No | No |
| Manage Roles | Yes | No | No | No |
| View Audit Log | Yes | Yes | No | No |
| Manage Security | Yes | No | No | No |
| Install MCP/Skill | Yes | Yes | No | No |
| System Restart | Yes | No | No | No |
| Self-Evolution | Yes | No | No | No |
| View System Status | Yes | Yes | Yes | Yes |

## 4. Complete Audit Logging

Every significant action in Argus is logged for security and debugging:

### 4.1 Audit Log Model

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLogEntry {
    pub id: Uuid,
    pub timestamp: DateTime<Utc>,
    /// Who performed the action
    pub actor: AuditActor,
    /// What happened
    pub action: AuditAction,
    /// What was affected
    pub resource: AuditResource,
    /// Outcome
    pub outcome: AuditOutcome,
    /// Additional context
    pub details: serde_json::Value,
    /// IP address (for web dashboard)
    pub ip_address: Option<String>,
    /// Session/conversation ID
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuditActor {
    Owner,
    Admin { identity_id: Uuid, name: String },
    Developer { identity_id: Uuid, name: String },
    System,                    // Argus itself
    Worker { worker_id: Uuid },
    Scheduler,
    AutoConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuditAction {
    // Authentication
    LoginSuccess,
    LoginFailed,
    LoginAttemptBlocked,

    // Configuration
    ConfigChanged { domain: String },
    ConfigUndone { domain: String },

    // Project management
    ProjectCreated,
    ProjectDeleted,
    ProjectModified,

    // Task management
    TaskCreated,
    TaskCompleted,
    TaskFailed,
    TaskCancelled,

    // Deployment
    DeployStarted,
    DeployApproved,
    DeployRejected,
    DeploySucceeded,
    DeployFailed,
    DeployRolledBack,

    // Worker operations
    WorkerSpawned,
    WorkerKilled,
    WorkerCommandBlocked,

    // Security
    SecretAccessed { name: String },
    SecretCreated { name: String },
    SecretDeleted { name: String },
    MasterPasswordRotated,
    PermissionDenied { permission: String },
    DangerousCommandBlocked { command: String },
    SandboxViolation { details: String },

    // MCP / Skills
    McpServerInstalled { name: String },
    McpServerRemoved { name: String },
    SkillInstalled { name: String },
    SkillExecuted { name: String },
    SecurityAuditCompleted { target: String },

    // Self-evolution
    SelfEvolutionStarted,
    SelfEvolutionDeployed,
    SelfEvolutionRolledBack,

    // System
    SystemStarted,
    SystemShutdown,
    BackupCreated,
    BackupRestored,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuditResource {
    Project { id: ProjectId, name: String },
    Task { id: Uuid },
    Worker { id: Uuid },
    Config { path: String },
    Secret { name: String },
    McpServer { name: String },
    Skill { name: String },
    System,
    Deployment { id: DeploymentId },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuditOutcome {
    Success,
    Failure { reason: String },
    Blocked { reason: String },
    Warned { warning: String },
}
```

### 4.2 Audit Logger

```rust
pub struct AuditLogger {
    db: PgPool,
    /// In-memory buffer for high-frequency events (batch insert)
    buffer: Arc<Mutex<Vec<AuditLogEntry>>>,
    /// Flush interval
    flush_interval: Duration,
}

impl AuditLogger {
    pub async fn log(&self, entry: AuditLogEntry) {
        // High-priority events flush immediately
        if entry.is_security_critical() {
            self.flush_single(&entry).await;
        } else {
            // Buffer for batch insert
            let mut buf = self.buffer.lock().await;
            buf.push(entry);
            if buf.len() >= 100 {
                let entries = std::mem::take(&mut *buf);
                drop(buf);
                self.flush_batch(entries).await;
            }
        }
    }

    async fn flush_single(&self, entry: &AuditLogEntry) {
        sqlx::query!(
            r#"
            INSERT INTO audit_log (
                id, timestamp, actor, action, resource,
                outcome, details, ip_address, session_id
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            "#,
            entry.id,
            entry.timestamp,
            serde_json::to_value(&entry.actor).unwrap(),
            serde_json::to_value(&entry.action).unwrap(),
            serde_json::to_value(&entry.resource).unwrap(),
            serde_json::to_value(&entry.outcome).unwrap(),
            entry.details,
            entry.ip_address,
            entry.session_id,
        )
        .execute(&self.db)
        .await
        .ok();
    }

    /// Periodic flush of buffered entries
    pub async fn run_flush_loop(&self) {
        let mut interval = tokio::time::interval(self.flush_interval);
        loop {
            interval.tick().await;
            let entries = {
                let mut buf = self.buffer.lock().await;
                std::mem::take(&mut *buf)
            };
            if !entries.is_empty() {
                self.flush_batch(entries).await;
            }
        }
    }

    /// Query audit logs with filters
    pub async fn query(&self, filter: &AuditQueryFilter) -> Result<Vec<AuditLogEntry>> {
        let rows = sqlx::query!(
            r#"
            SELECT * FROM audit_log
            WHERE ($1::timestamptz IS NULL OR timestamp >= $1)
              AND ($2::timestamptz IS NULL OR timestamp <= $2)
              AND ($3::text IS NULL OR actor::text LIKE $3)
              AND ($4::text IS NULL OR action::text LIKE $4)
            ORDER BY timestamp DESC
            LIMIT $5
            "#,
            filter.since,
            filter.until,
            filter.actor_filter,
            filter.action_filter,
            filter.limit.unwrap_or(100) as i64,
        )
        .fetch_all(&self.db)
        .await?;

        // TODO: Implement deserialization of audit log rows into AuditLogEntry structs
        // Query: SELECT id, actor, action, resource, outcome, ip_address, timestamp, metadata
        //        FROM audit_log WHERE timestamp BETWEEN $1 AND $2
        //        AND ($3 IS NULL OR actor LIKE $3) AND ($4 IS NULL OR action = $4)
        //        ORDER BY timestamp DESC LIMIT $5
        // Map each row to AuditLogEntry { actor, action, resource, outcome, timestamp, ... }
        Ok(Vec::new()) // placeholder
    }
}

impl AuditLogEntry {
    fn is_security_critical(&self) -> bool {
        matches!(
            &self.action,
            AuditAction::LoginFailed
                | AuditAction::LoginAttemptBlocked
                | AuditAction::PermissionDenied { .. }
                | AuditAction::DangerousCommandBlocked { .. }
                | AuditAction::SandboxViolation { .. }
                | AuditAction::SecretAccessed { .. }
                | AuditAction::MasterPasswordRotated
                | AuditAction::SelfEvolutionStarted
                | AuditAction::SelfEvolutionDeployed
        )
    }
}
```

## 5. Worker Sandbox Enforcement

Workers are isolated to prevent damage to the host system or cross-project contamination.

### 5.1 Sandbox Modes

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SandboxMode {
    /// Directory-level isolation (lightweight)
    /// Worker can only access its assigned project directory
    Directory,
    /// Docker container isolation (strong)
    /// Worker runs in an ephemeral container with limited resources
    Docker,
    /// No sandbox (owner explicitly opted out)
    None,
}

pub struct SandboxEnforcer {
    mode: SandboxMode,
    command_guard: CommandGuard,
    resource_limiter: ResourceLimiter,
}

impl SandboxEnforcer {
    /// Validate a command before execution in a worker
    pub fn validate_command(
        &self,
        command: &str,
        worker_context: &WorkerContext,
    ) -> Result<CommandVerdict> {
        // Step 1: Check against blocked commands
        let guard_result = self.command_guard.check(command)?;
        if let CommandVerdict::Blocked(reason) = &guard_result {
            return Ok(guard_result);
        }

        // Step 2: Check path access (directory sandbox)
        if self.mode == SandboxMode::Directory {
            let paths = self.extract_paths(command);
            for path in &paths {
                if !self.is_within_sandbox(path, &worker_context.workspace) {
                    return Ok(CommandVerdict::Blocked(format!(
                        "Path '{}' is outside sandbox directory '{}'",
                        path, worker_context.workspace
                    )));
                }
            }
        }

        // Step 3: Check resource limits
        if !self.resource_limiter.can_execute(worker_context) {
            return Ok(CommandVerdict::Blocked(
                "Resource limit exceeded".into(),
            ));
        }

        Ok(CommandVerdict::Allowed)
    }

    /// Check if a path is within the sandbox boundary
    fn is_within_sandbox(&self, path: &str, workspace: &str) -> bool {
        let resolved = std::path::Path::new(path)
            .canonicalize()
            .unwrap_or_else(|_| std::path::PathBuf::from(path));
        let workspace_path = std::path::Path::new(workspace)
            .canonicalize()
            .unwrap_or_else(|_| std::path::PathBuf::from(workspace));

        resolved.starts_with(&workspace_path)
    }
}

#[derive(Debug, Clone)]
pub enum CommandVerdict {
    Allowed,
    Blocked(String),
    AllowedWithWarning(String),
}
```

### 5.2 Docker Sandbox Configuration

When using Docker sandbox mode, each worker runs in an isolated container:

```rust
pub struct DockerSandboxConfig {
    /// Base image for worker containers
    pub base_image: String,
    /// Memory limit per container
    pub memory_limit: String,
    /// CPU limit (fractional cores)
    pub cpu_limit: f64,
    /// Disk space limit
    pub disk_limit: String,
    /// Network access policy
    pub network: NetworkPolicy,
    /// Read-only filesystem (except workspace)
    pub read_only_root: bool,
    /// Drop all Linux capabilities except those needed
    pub drop_capabilities: bool,
    /// No new privileges flag
    pub no_new_privileges: bool,
    /// Seccomp profile
    pub seccomp_profile: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NetworkPolicy {
    /// Full internet access (needed for npm install, pip install, etc.)
    Full,
    /// Only access to specific hosts
    AllowList(Vec<String>),
    /// No network access at all
    None,
}

impl DockerSandboxConfig {
    pub fn default_secure() -> Self {
        Self {
            base_image: "argus-worker:latest".into(),
            memory_limit: "2g".into(),
            cpu_limit: 2.0,
            disk_limit: "10g".into(),
            network: NetworkPolicy::Full, // workers need internet for dependencies
            read_only_root: true,
            drop_capabilities: true,
            no_new_privileges: true,
            seccomp_profile: Some("default".into()),
        }
    }
}
```

## 6. Command Guard

Blocks dangerous commands that could damage the system:

### 6.1 Blocked Command Patterns

```rust
pub struct CommandGuard {
    /// Blocked command patterns (regex)
    blocked_patterns: Vec<BlockedPattern>,
    /// Custom user-defined blocked commands
    custom_blocked: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct BlockedPattern {
    pub pattern: Regex,
    pub reason: String,
    pub severity: Severity,
    /// Can the owner override this block?
    pub overridable: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Severity {
    /// Will cause irreversible damage
    Critical,
    /// High risk but potentially intentional
    High,
    /// Moderate risk
    Medium,
}

impl CommandGuard {
    pub fn new() -> Self {
        let blocked_patterns = vec![
            // === CRITICAL: Never allow ===
            BlockedPattern {
                pattern: Regex::new(r"rm\s+(-[rRf]+\s+)*/\s*$").unwrap(),
                reason: "Deleting root filesystem".into(),
                severity: Severity::Critical,
                overridable: false,
            },
            BlockedPattern {
                pattern: Regex::new(r"rm\s+(-[rRf]+\s+)*/boot").unwrap(),
                reason: "Deleting boot partition".into(),
                severity: Severity::Critical,
                overridable: false,
            },
            BlockedPattern {
                pattern: Regex::new(r"mkfs\.\w+\s+/dev/").unwrap(),
                reason: "Formatting disk partition".into(),
                severity: Severity::Critical,
                overridable: false,
            },
            BlockedPattern {
                pattern: Regex::new(r"dd\s+.*of=/dev/[sh]d").unwrap(),
                reason: "Direct disk write with dd".into(),
                severity: Severity::Critical,
                overridable: false,
            },
            BlockedPattern {
                pattern: Regex::new(r":\(\)\s*\{\s*:\|\:\s*&\s*\}\s*;").unwrap(),
                reason: "Fork bomb detected".into(),
                severity: Severity::Critical,
                overridable: false,
            },

            // === HIGH: Block unless owner explicitly approves ===
            BlockedPattern {
                pattern: Regex::new(r"git\s+push\s+.*--force\s+.*main|master").unwrap(),
                reason: "Force push to main/master branch".into(),
                severity: Severity::High,
                overridable: true,
            },
            BlockedPattern {
                pattern: Regex::new(r"git\s+push\s+.*-f\s+.*main|master").unwrap(),
                reason: "Force push to main/master branch".into(),
                severity: Severity::High,
                overridable: true,
            },
            BlockedPattern {
                pattern: Regex::new(r"DROP\s+DATABASE", ).unwrap(),
                reason: "Dropping database".into(),
                severity: Severity::High,
                overridable: true,
            },
            BlockedPattern {
                pattern: Regex::new(r"TRUNCATE\s+TABLE.*CASCADE").unwrap(),
                reason: "Truncating table with cascade".into(),
                severity: Severity::High,
                overridable: true,
            },
            BlockedPattern {
                pattern: Regex::new(r"chmod\s+(-R\s+)?777\s+/").unwrap(),
                reason: "Setting world-writable permissions on system directory".into(),
                severity: Severity::High,
                overridable: true,
            },
            BlockedPattern {
                pattern: Regex::new(r"curl\s+.*\|\s*(ba)?sh").unwrap(),
                reason: "Piping remote script to shell".into(),
                severity: Severity::High,
                overridable: true,
            },
            BlockedPattern {
                pattern: Regex::new(r"wget\s+.*\|\s*(ba)?sh").unwrap(),
                reason: "Piping remote script to shell".into(),
                severity: Severity::High,
                overridable: true,
            },

            // === MEDIUM: Warn but allow ===
            BlockedPattern {
                pattern: Regex::new(r"rm\s+-rf\s+").unwrap(),
                reason: "Recursive force delete".into(),
                severity: Severity::Medium,
                overridable: true,
            },
            BlockedPattern {
                pattern: Regex::new(r"sudo\s+").unwrap(),
                reason: "Running command with sudo".into(),
                severity: Severity::Medium,
                overridable: true,
            },
        ];

        Self {
            blocked_patterns,
            custom_blocked: vec![],
        }
    }

    pub fn check(&self, command: &str) -> Result<CommandVerdict> {
        for pattern in &self.blocked_patterns {
            if pattern.pattern.is_match(command) {
                match pattern.severity {
                    Severity::Critical => {
                        return Ok(CommandVerdict::Blocked(format!(
                            "BLOCKED (critical): {}",
                            pattern.reason
                        )));
                    }
                    Severity::High => {
                        return Ok(CommandVerdict::Blocked(format!(
                            "BLOCKED (high risk): {}. Owner approval required to override.",
                            pattern.reason
                        )));
                    }
                    Severity::Medium => {
                        return Ok(CommandVerdict::AllowedWithWarning(format!(
                            "Warning: {}",
                            pattern.reason
                        )));
                    }
                }
            }
        }

        // Check custom blocked commands
        for blocked in &self.custom_blocked {
            if command.contains(blocked) {
                return Ok(CommandVerdict::Blocked(format!(
                    "BLOCKED: matches custom rule '{}'",
                    blocked
                )));
            }
        }

        Ok(CommandVerdict::Allowed)
    }
}
```

## 7. Secret Scanner

Prevents accidental secret leakage in code, logs, and outputs:

```rust
pub struct SecretScanner {
    patterns: Vec<SecretPattern>,
}

#[derive(Debug, Clone)]
pub struct SecretPattern {
    pub name: String,
    pub regex: Regex,
    pub severity: Severity,
}

impl SecretScanner {
    pub fn new() -> Self {
        let patterns = vec![
            SecretPattern {
                name: "AWS Access Key".into(),
                regex: Regex::new(r"AKIA[0-9A-Z]{16}").unwrap(),
                severity: Severity::Critical,
            },
            SecretPattern {
                name: "AWS Secret Key".into(),
                regex: Regex::new(r"(?i)aws_secret_access_key\s*=\s*\S+").unwrap(),
                severity: Severity::Critical,
            },
            SecretPattern {
                name: "Private Key".into(),
                regex: Regex::new(r"-----BEGIN (RSA |EC |DSA )?PRIVATE KEY-----").unwrap(),
                severity: Severity::Critical,
            },
            SecretPattern {
                name: "OpenAI API Key".into(),
                regex: Regex::new(r"sk-[a-zA-Z0-9]{20,}").unwrap(),
                severity: Severity::High,
            },
            SecretPattern {
                name: "GitHub Token".into(),
                regex: Regex::new(r"gh[pousr]_[A-Za-z0-9_]{36,}").unwrap(),
                severity: Severity::High,
            },
            SecretPattern {
                name: "Generic API Key".into(),
                regex: Regex::new(r#"(?i)(api_key|apikey|api-key|secret_key|secretkey)\s*[:=]\s*["']?\S{20,}"#).unwrap(),
                severity: Severity::Medium,
            },
            SecretPattern {
                name: "Database URL with password".into(),
                regex: Regex::new(r"(?i)(postgres|mysql|mongodb)://\w+:\S+@").unwrap(),
                severity: Severity::High,
            },
            SecretPattern {
                name: "JWT Token".into(),
                regex: Regex::new(r"eyJ[A-Za-z0-9-_]+\.eyJ[A-Za-z0-9-_]+\.[A-Za-z0-9-_.+/]*").unwrap(),
                severity: Severity::Medium,
            },
        ];

        Self { patterns }
    }

    /// Scan text for potential secrets
    pub fn scan(&self, text: &str) -> Vec<SecretFinding> {
        let mut findings = vec![];

        for pattern in &self.patterns {
            for mat in pattern.regex.find_iter(text) {
                findings.push(SecretFinding {
                    pattern_name: pattern.name.clone(),
                    severity: pattern.severity.clone(),
                    position: mat.start()..mat.end(),
                    redacted: self.redact(mat.as_str()),
                });
            }
        }

        findings
    }

    /// Redact a secret value, showing only first/last 4 chars
    fn redact(&self, value: &str) -> String {
        if value.len() <= 8 {
            "***REDACTED***".into()
        } else {
            format!("{}...{}", &value[..4], &value[value.len()-4..])
        }
    }

    /// Scan a file for secrets before git commit
    pub fn scan_file(&self, path: &str) -> Result<Vec<SecretFinding>> {
        let content = std::fs::read_to_string(path)?;
        Ok(self.scan(&content))
    }

    /// Scan worker output and redact any secrets before logging
    pub fn sanitize_output(&self, output: &str) -> String {
        let mut sanitized = output.to_string();
        for pattern in &self.patterns {
            sanitized = pattern.regex.replace_all(&sanitized, "***REDACTED***").to_string();
        }
        sanitized
    }
}

#[derive(Debug, Clone)]
pub struct SecretFinding {
    pub pattern_name: String,
    pub severity: Severity,
    pub position: std::ops::Range<usize>,
    pub redacted: String,
}
```

## 8. Communication Encryption

### 8.1 External Communication

```
┌──────────┐         TLS 1.3          ┌──────────────┐
│  Owner   │ ◄──────────────────────► │  Argus       │
│  Browser │    HTTPS/WSS              │  Web         │
│          │                           │  Dashboard   │
└──────────┘                           └──────────────┘

┌──────────┐         TLS 1.2+         ┌──────────────┐
│  Telegram│ ◄──────────────────────► │  Argus       │
│  Discord │    Platform APIs          │  Social      │
│  Slack   │    (HTTPS only)           │  Gateway     │
└──────────┘                           └──────────────┘
```

- All web dashboard connections require HTTPS (TLS 1.3 preferred)
- WebSocket connections upgrade from HTTPS (WSS only)
- Social platform APIs use their native HTTPS endpoints
- SSH connections for VPS deployment use standard SSH encryption

### 8.2 Internal Communication

```rust
/// Web dashboard TLS configuration
pub struct TlsConfig {
    /// Path to TLS certificate
    pub cert_path: String,
    /// Path to TLS private key
    pub key_path: String,
    /// Minimum TLS version
    pub min_version: TlsVersion,
    /// Auto-renew via ACME (Let's Encrypt)
    pub acme: Option<AcmeConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TlsVersion {
    Tls12,
    Tls13,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcmeConfig {
    pub email: String,
    pub domains: Vec<String>,
    pub staging: bool,  // use Let's Encrypt staging for testing
}
```

## 9. Rate Limiting

Protect against abuse (especially for web dashboard and social platform endpoints):

```rust
pub struct RateLimiter {
    /// Per-identity rate limits
    limits: Arc<DashMap<Uuid, RateState>>,
    config: RateLimitConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    /// Max requests per minute per identity
    pub requests_per_minute: u32,
    /// Max failed login attempts before lockout
    pub max_login_failures: u32,
    /// Lockout duration after too many failures
    pub lockout_duration: Duration,
    /// Max concurrent sessions per identity
    pub max_concurrent_sessions: u32,
}

struct RateState {
    request_timestamps: VecDeque<Instant>,
    failed_logins: u32,
    locked_until: Option<Instant>,
}

impl RateLimiter {
    pub fn check(&self, identity_id: &Uuid) -> Result<()> {
        let mut state = self.limits
            .entry(*identity_id)
            .or_insert_with(|| RateState {
                request_timestamps: VecDeque::new(),
                failed_logins: 0,
                locked_until: None,
            });

        // Check lockout
        if let Some(locked_until) = state.locked_until {
            if Instant::now() < locked_until {
                return Err(anyhow!("Account locked. Try again later."));
            }
            state.locked_until = None;
            state.failed_logins = 0;
        }

        // Clean old timestamps
        let cutoff = Instant::now() - Duration::from_secs(60);
        while state.request_timestamps.front().map_or(false, |&t| t < cutoff) {
            state.request_timestamps.pop_front();
        }

        // Check rate
        if state.request_timestamps.len() >= self.config.requests_per_minute as usize {
            return Err(anyhow!("Rate limit exceeded"));
        }

        state.request_timestamps.push_back(Instant::now());
        Ok(())
    }

    pub fn record_login_failure(&self, identity_id: &Uuid) {
        let mut state = self.limits
            .entry(*identity_id)
            .or_insert_with(|| RateState {
                request_timestamps: VecDeque::new(),
                failed_logins: 0,
                locked_until: None,
            });

        state.failed_logins += 1;
        if state.failed_logins >= self.config.max_login_failures {
            state.locked_until = Some(
                Instant::now() + self.config.lockout_duration
            );
        }
    }
}
```

## 10. Third-Party Code Auditing

See `MCP_AND_SKILLS.md` for the full security audit pipeline. Summary:

1. **Static Analysis** — scan for dangerous API calls, network access, file system access
2. **Behavioral Analysis** — run in isolated sandbox, monitor syscalls
3. **LLM Review** — AI-powered code review for subtle malicious patterns
4. **Verdict** — Safe / Warning / Dangerous, with trust level assignment

## 11. TOML Configuration

> **Note**: Configuration values shown below are system-managed defaults. In production, all configuration is managed through Argus's conversational interface (see `CONVERSATIONAL_CONFIG.md`). Direct TOML file editing is not required.

```toml
[security]
# Sandbox mode for workers
sandbox_mode = "docker"  # "docker" | "directory" | "none"

# Audit logging
audit_level = "full"     # "full" | "security_only" | "minimal"
audit_retention_days = 90

# Rate limiting
[security.rate_limit]
requests_per_minute = 60
max_login_failures = 5
lockout_duration = "15m"
max_concurrent_sessions = 3

# Key vault
[security.vault]
# Argon2id parameters for master key derivation
argon2_memory_kb = 65536
argon2_iterations = 3
argon2_parallelism = 4

# TLS
[security.tls]
cert_path = "/etc/argus/tls/cert.pem"
key_path = "/etc/argus/tls/key.pem"
min_version = "tls13"

# ACME (Let's Encrypt auto-renewal)
[security.tls.acme]
enabled = true
email = "owner@example.com"
domains = ["argus.example.com"]
staging = false

# Command guard — additional blocked patterns
[security.blocked_commands]
custom = [
    "shutdown",
    "reboot",
    "halt",
]

# Network policy for Docker sandboxed workers
[security.docker_sandbox]
base_image = "argus-worker:latest"
memory_limit = "2g"
cpu_limit = 2.0
disk_limit = "10g"
network = "full"        # "full" | "allowlist" | "none"
read_only_root = true
drop_capabilities = true
no_new_privileges = true
```

## 12. Conversational Security Management

```
Owner: 显示密钥列表

Argus: 🔐 **密钥保险箱**

┌──────────────────────┬─────────────────┬─────────────┐
│ 名称                  │ 类别             │ 最后使用      │
├──────────────────────┼─────────────────┼─────────────┤
│ openai_key_1         │ LLM API Key     │ 2分钟前       │
│ anthropic_key        │ LLM API Key     │ 1小时前       │
│ deepseek_key         │ LLM API Key     │ 5分钟前       │
│ staging_ssh_key      │ SSH Key         │ 昨天          │
│ prod_ssh_key         │ SSH Key         │ 3天前         │
│ vercel_token         │ Deploy Token    │ 1周前         │
│ telegram_bot_token   │ OAuth Token     │ 持续使用      │
└──────────────────────┴─────────────────┴─────────────┘

7个密钥，全部已加密存储。
```

```
Owner: 添加新的 Anthropic key

Argus: 好的，请发送 API Key（我会立即加密存储，聊天记录中不会保留明文）。

Owner: sk-ant-api03-xxxxxxxxxxxxx

Argus: ✓ 已加密存储为 `anthropic_key_2`。

       已有的 anthropic_key 保留，现在会轮换使用两个 key。
       需要删除旧的吗？
```

```
Owner: 最近有什么安全事件吗

Argus: 📊 **安全概况** (过去7天)

✓ 无高危事件

┌────────────────────────────────────────────────┐
│ 被阻止的命令: 3                                  │
│   - rm -rf /tmp/project (sandbox violation)     │
│   - curl ... | bash (high risk)                 │
│   - chmod 777 / (high risk)                     │
│                                                  │
│ 密钥访问: 1,247 次（全部正常）                     │
│ 登录失败: 0                                      │
│ 沙箱违规: 1 (worker 尝试访问 /etc)                │
│ 第三方审计: 2 个 MCP 服务器通过审查               │
└────────────────────────────────────────────────┘
```

## 13. Security Invariants

These rules are enforced at all times and cannot be disabled:

1. **Secrets never appear in logs, outputs, or chat history** — always redacted
2. **Master password is never stored** — only the derived key exists in memory
3. **Critical commands (rm -rf /, fork bombs) cannot be overridden** — even by the owner
4. **All deployments to production require authentication** — even in auto-deploy mode
5. **Self-evolution changes must pass all tests** before deployment
6. **Audit log is append-only** — entries cannot be deleted or modified
7. **Key vault encryption uses random nonces** — never reused
8. **Worker processes run with minimal privileges** — no root access in Docker sandbox
9. **Database connections use TLS** when connecting to remote PostgreSQL
10. **Web dashboard sessions expire** after configurable idle timeout (default: 30 minutes)
