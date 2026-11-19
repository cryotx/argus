# Conversational Configuration — Detailed Design

> **Crate**: `argus-core` (config module)

All Argus configuration happens through natural language conversation. There are no manual config files to edit — the owner speaks, Argus understands, validates, applies, and confirms. This document defines how natural language is parsed into configuration changes, the configurable domains, multi-turn dialogs, undo capability, and the audit trail.

## 1. Architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│                  Conversational Config Engine                          │
│                                                                       │
│  ┌──────────────┐  ┌──────────────┐  ┌────────────────────────────┐ │
│  │  Intent       │  │  Config      │  │  Dialog                    │ │
│  │  Classifier   │→│  Resolver    │→│  Manager                   │ │
│  │  (from Brain) │  │  (map to     │  │  (multi-turn if needed)   │ │
│  │               │  │   domain)    │  │                            │ │
│  └──────────────┘  └──────────────┘  └────────────┬───────────────┘ │
│                                                     │                 │
│                                                     ▼                 │
│  ┌──────────────┐  ┌──────────────┐  ┌────────────────────────────┐ │
│  │  Validator    │  │  Applier     │  │  Change Builder            │ │
│  │  (check       │←│  (write to   │←│  (NL → structured change)  │ │
│  │   constraints)│  │   DB/config) │  │                            │ │
│  └──────────────┘  └──────────────┘  └────────────────────────────┘ │
│                                                                       │
│  ┌──────────────┐  ┌──────────────┐  ┌────────────────────────────┐ │
│  │  Undo         │  │  Audit       │  │  Config                    │ │
│  │  Manager      │  │  Trail       │  │  Store                     │ │
│  │  (revert)     │  │  (log all)   │  │  (PostgreSQL + cache)      │ │
│  └──────────────┘  └──────────────┘  └────────────────────────────┘ │
└──────────────────────────────────────────────────────────────────────┘
```

## 2. Configurable Domains

Every aspect of Argus is configurable through conversation. Domains are organized hierarchically:

### 2.1 Domain Registry

```rust
/// All configurable domains in Argus
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ConfigDomain {
    /// LLM provider settings
    Llm(LlmConfigKey),
    /// Memory system settings
    Memory(MemoryConfigKey),
    /// Worker pool settings
    Worker(WorkerConfigKey),
    /// MCP server management
    Mcp(McpConfigKey),
    /// Skills management
    Skills(SkillsConfigKey),
    /// Notification & social settings
    Notification(NotificationConfigKey),
    /// Owner profile & preferences
    OwnerProfile(OwnerProfileConfigKey),
    /// Project-specific settings
    Project(ProjectId, ProjectConfigKey),
    /// Deployment settings
    Deploy(DeployConfigKey),
    /// Security settings
    Security(SecurityConfigKey),
    /// Scheduler settings
    Scheduler(SchedulerConfigKey),
    /// System-wide settings
    System(SystemConfigKey),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum LlmConfigKey {
    AddProvider,
    RemoveProvider,
    SetApiKey { provider: String },
    SetDefaultModel,
    SetModelForTask { task_type: String },
    SetGlobalBudget,
    SetProjectBudget { project_id: ProjectId },
    SetFailoverChain,
    EnableProvider { provider: String },
    DisableProvider { provider: String },
    SetRateLimit { provider: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum MemoryConfigKey {
    SetShortTermTtl,
    SetDecayRate,
    SetConsolidationInterval,
    SetImportanceThreshold,
    SetMaxWorkingMemoryTokens,
    AddPermanentMemory,
    RemovePermanentMemory,
    SetRecallTopK,
    SetSimilarityThreshold,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum WorkerConfigKey {
    SetMaxWorkers,
    SetDefaultDriver,
    SetWorkerTimeout,
    SetIdleTimeout,
    EnableSandbox,
    SetResourceLimits,
    SetWorkerPreference { task_type: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum NotificationConfigKey {
    SetDisturbanceLevel,
    SetActiveHours,
    SetPlatformPriority,
    EnablePlatform { platform: String },
    DisablePlatform { platform: String },
    SetDigestSchedule,
    SetNotificationFilter,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum OwnerProfileConfigKey {
    SetName,
    SetLanguage,
    SetCommunicationStyle,
    SetApprovalMode,
    SetTimezone,
    AddPreference,
    RemovePreference,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ProjectConfigKey {
    SetName,
    SetDescription,
    SetGitRepo,
    SetBranches,
    SetDeployTarget,
    SetTestCommand,
    SetBuildCommand,
    SetPriority,
    SetAutoApprove,
    AddEnvironment,
    RemoveEnvironment,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum DeployConfigKey {
    SetDefaultTarget,
    SetApprovalPolicy,
    SetHealthCheckDefaults,
    SetSnapshotRetention,
    SetArtifactMaxAge,
    SetAutoRollback,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum SecurityConfigKey {
    SetSandboxMode,
    AddBlockedCommand,
    RemoveBlockedCommand,
    SetAuditLevel,
    SetKeyVaultMasterPassword,
    AddTrustedSource,
    RemoveTrustedSource,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum SchedulerConfigKey {
    SetMaxConcurrentTasks,
    SetDefaultPriority,
    SetRetryPolicy,
    SetCheckpointInterval,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum SystemConfigKey {
    SetLogLevel,
    SetDataDirectory,
    SetDatabaseUrl,
    SetWebDashboardPort,
    SetBackupSchedule,
}
```

### 2.2 Domain Map

| Domain | Config Keys | Examples (Chinese) |
|--------|-------------|-------------------|
| LLM | Provider, API key, model routing, budget | "设置 OpenAI 的 API key"、"预算限制每月50美元" |
| Memory | TTL, decay rate, importance threshold | "记忆保留改为48小时"、"提高记忆重要性门槛" |
| Worker | Pool size, timeout, driver preference | "最多同时开5个worker"、"复杂任务用 Claude Code" |
| MCP | Server install/remove, trust level | "安装 Playwright MCP 服务器"、"卸载那个不安全的" |
| Skills | Enable/disable, install, permissions | "禁用那个文件删除技能"、"安装代码格式化技能" |
| Notification | Hours, channels, disturbance level | "晚上11点到早上8点别打扰我"、"重要消息发Telegram" |
| Owner Profile | Language, style, approval mode | "回复简洁点"、"关键决策都需要我确认" |
| Project | Repo, branches, deploy, test commands | "这个项目用 pytest 跑测试"、"main 分支自动部署" |
| Deploy | Targets, approval, health checks | "生产环境需要我审批"、"用 Docker 部署" |
| Security | Sandbox, blocked commands, audit | "禁止执行 rm -rf"、"开启完整审计日志" |
| Scheduler | Concurrency, priority, retry | "最多同时3个任务"、"失败重试3次" |
| System | Log level, ports, backup | "日志级别调成 debug"、"每天凌晨3点备份" |

## 3. Intent-to-Config Mapping

### 3.1 Config Intent Resolution

The Brain classifies incoming messages and routes config-related intents to the Config Engine:

```rust
/// Config-related intents from the Brain's classifier
#[derive(Debug, Clone)]
pub enum ConfigIntent {
    /// Direct setting change: "设置X为Y"
    SetValue {
        domain: ConfigDomain,
        value: serde_json::Value,
    },
    /// Toggle: "启用/禁用X"
    Toggle {
        domain: ConfigDomain,
        enabled: bool,
    },
    /// Add item: "添加X"
    AddItem {
        domain: ConfigDomain,
        item: serde_json::Value,
    },
    /// Remove item: "删除/移除X"
    RemoveItem {
        domain: ConfigDomain,
        item_identifier: String,
    },
    /// Query current value: "X 的设置是什么"
    Query {
        domain: ConfigDomain,
    },
    /// Undo last change: "撤销刚才的修改"
    Undo,
    /// Complex/ambiguous — needs multi-turn dialog
    NeedsDialog {
        partial_domain: Option<ConfigDomain>,
        raw_text: String,
    },
}

/// Resolves natural language into a structured ConfigIntent
pub struct ConfigResolver {
    llm: LlmRouter,
}

impl ConfigResolver {
    /// Parse a natural language config request
    pub async fn resolve(&self, message: &str, context: &ConversationContext) -> Result<ConfigIntent> {
        let prompt = format!(
            r#"You are a configuration parser for Argus. Parse the user's message into a structured config change.

Available domains and their keys:
{domain_registry}

User message: "{message}"

Recent conversation context:
{context}

Respond with JSON:
{{
  "action": "set_value" | "toggle" | "add_item" | "remove_item" | "query" | "undo" | "needs_dialog",
  "domain": "<domain path>",
  "value": <value if applicable>,
  "confidence": 0.0-1.0,
  "missing_info": ["<what else is needed>"]
}}
"#,
            domain_registry = self.domain_registry_description(),
            message = message,
            context = context.recent_summary(),
        );

        let response = self.llm.complete(&prompt, ModelClass::Fast).await?;
        let parsed: ConfigParseResult = serde_json::from_str(&response)?;

        if parsed.confidence < 0.7 || !parsed.missing_info.is_empty() {
            Ok(ConfigIntent::NeedsDialog {
                partial_domain: parsed.domain,
                raw_text: message.to_string(),
            })
        } else {
            self.build_intent(parsed)
        }
    }
}
```

### 3.2 Natural Language Patterns

Common patterns the resolver handles:

| Pattern (Chinese) | Parsed Intent | Domain |
|-------------------|---------------|--------|
| "设置 X 为/到 Y" | SetValue | Auto-detected from X |
| "把 X 改成 Y" | SetValue | Auto-detected from X |
| "启用/开启 X" | Toggle(true) | Auto-detected from X |
| "禁用/关闭 X" | Toggle(false) | Auto-detected from X |
| "添加/安装 X" | AddItem | Auto-detected from X |
| "删除/移除/卸载 X" | RemoveItem | Auto-detected from X |
| "X 是什么/多少" | Query | Auto-detected from X |
| "撤销/回退" | Undo | N/A |
| "现在的配置" | Query (all) | System |

| Pattern (English) | Parsed Intent | Domain |
|-------------------|---------------|--------|
| "Set X to Y" | SetValue | Auto-detected |
| "Change X to Y" | SetValue | Auto-detected |
| "Enable/Turn on X" | Toggle(true) | Auto-detected |
| "Disable/Turn off X" | Toggle(false) | Auto-detected |
| "Add/Install X" | AddItem | Auto-detected |
| "Remove/Uninstall X" | RemoveItem | Auto-detected |
| "What is X set to?" | Query | Auto-detected |
| "Undo that" | Undo | N/A |

## 4. Change Builder

### 4.1 Configuration Change

A structured, validated, reversible config change:

```rust
/// A single configuration change
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigChange {
    pub id: Uuid,
    pub domain: ConfigDomain,
    pub action: ConfigAction,
    /// The previous value (for undo)
    pub previous_value: Option<serde_json::Value>,
    /// The new value
    pub new_value: serde_json::Value,
    /// Human-readable description
    pub description: String,
    /// When this change was applied
    pub applied_at: DateTime<Utc>,
    /// Whether this change has been undone
    pub undone: bool,
    /// The message that triggered this change
    pub trigger_message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConfigAction {
    Set,
    Toggle,
    Add,
    Remove,
    BulkUpdate(Vec<ConfigChange>),
}

/// Builds a ConfigChange from a resolved intent
pub struct ChangeBuilder {
    config_store: ConfigStore,
}

impl ChangeBuilder {
    pub async fn build(&self, intent: &ConfigIntent) -> Result<ConfigChange> {
        match intent {
            ConfigIntent::SetValue { domain, value } => {
                // Read current value for undo capability
                let previous = self.config_store.get(domain).await?;

                Ok(ConfigChange {
                    id: Uuid::new_v4(),
                    domain: domain.clone(),
                    action: ConfigAction::Set,
                    previous_value: previous,
                    new_value: value.clone(),
                    description: format!(
                        "Set {} to {}",
                        domain.display_name(),
                        serde_json::to_string_pretty(value)?
                    ),
                    applied_at: Utc::now(),
                    undone: false,
                    trigger_message: String::new(), // filled by caller
                })
            }

            ConfigIntent::Toggle { domain, enabled } => {
                let previous = self.config_store.get(domain).await?;

                Ok(ConfigChange {
                    id: Uuid::new_v4(),
                    domain: domain.clone(),
                    action: ConfigAction::Toggle,
                    previous_value: previous,
                    new_value: serde_json::Value::Bool(*enabled),
                    description: format!(
                        "{} {}",
                        if *enabled { "Enable" } else { "Disable" },
                        domain.display_name()
                    ),
                    applied_at: Utc::now(),
                    undone: false,
                    trigger_message: String::new(),
                })
            }

            // Similar for AddItem, RemoveItem
            _ => Err(anyhow!("Cannot build change from {:?}", intent)),
        }
    }
}
```

## 5. Validator

Before applying changes, validate constraints:

```rust
pub struct ConfigValidator;

#[derive(Debug, Clone)]
pub struct ValidationResult {
    pub valid: bool,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

impl ConfigValidator {
    pub fn validate(&self, change: &ConfigChange) -> ValidationResult {
        let mut result = ValidationResult {
            valid: true,
            warnings: vec![],
            errors: vec![],
        };

        match &change.domain {
            ConfigDomain::Llm(LlmConfigKey::SetGlobalBudget) => {
                if let Some(budget) = change.new_value.as_f64() {
                    if budget <= 0.0 {
                        result.errors.push("Budget must be positive".into());
                        result.valid = false;
                    }
                    if budget > 10000.0 {
                        result.warnings.push(
                            "Budget exceeds $10,000 — are you sure?".into(),
                        );
                    }
                } else {
                    result.errors.push("Budget must be a number".into());
                    result.valid = false;
                }
            }

            ConfigDomain::Worker(WorkerConfigKey::SetMaxWorkers) => {
                if let Some(max) = change.new_value.as_u64() {
                    if max == 0 {
                        result.errors.push("Must have at least 1 worker".into());
                        result.valid = false;
                    }
                    if max > 50 {
                        result.warnings.push(
                            "50+ workers may exhaust system resources".into(),
                        );
                    }
                }
            }

            ConfigDomain::Notification(NotificationConfigKey::SetActiveHours) => {
                // Validate time format
                if let Some(hours) = change.new_value.as_object() {
                    let start = hours.get("start").and_then(|v| v.as_str());
                    let end = hours.get("end").and_then(|v| v.as_str());
                    if start.is_none() || end.is_none() {
                        result.errors.push(
                            "Active hours must have 'start' and 'end' times".into(),
                        );
                        result.valid = false;
                    }
                }
            }

            ConfigDomain::Security(SecurityConfigKey::SetSandboxMode) => {
                if change.new_value == serde_json::Value::Bool(false) {
                    result.warnings.push(
                        "Disabling sandbox mode removes worker isolation — security risk!".into(),
                    );
                }
            }

            // ... validation rules for all domains
            _ => {}
        }

        result
    }
}
```

## 6. Config Applier

Writes validated changes to the database and notifies relevant modules:

```rust
pub struct ConfigApplier {
    config_store: ConfigStore,
    audit_trail: AuditTrail,
    event_bus: EventBusTx,
    undo_manager: UndoManager,
}

impl ConfigApplier {
    pub async fn apply(&self, change: ConfigChange) -> Result<ApplyResult> {
        // 1. Validate
        let validation = ConfigValidator.validate(&change);
        if !validation.valid {
            return Ok(ApplyResult::ValidationFailed(validation));
        }

        // 2. Store the change for undo
        self.undo_manager.push(change.clone()).await?;

        // 3. Write to config store
        self.config_store.set(&change.domain, &change.new_value).await?;

        // 4. Record in audit trail
        self.audit_trail.record(AuditEntry {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            action: format!("config.{}", change.action.as_str()),
            domain: change.domain.path(),
            old_value: change.previous_value.clone(),
            new_value: Some(change.new_value.clone()),
            description: change.description.clone(),
            source: AuditSource::OwnerConversation,
        }).await?;

        // 5. Notify affected modules via event bus
        self.event_bus.send(ArgusEvent::ConfigChanged(
            change.domain.clone(),
            change.new_value.clone(),
        ));

        // 6. Return result with warnings if any
        Ok(ApplyResult::Applied {
            change_id: change.id,
            warnings: validation.warnings,
        })
    }
}

pub enum ApplyResult {
    Applied {
        change_id: Uuid,
        warnings: Vec<String>,
    },
    ValidationFailed(ValidationResult),
    NeedsConfirmation {
        change: ConfigChange,
        reason: String,
    },
}
```

## 7. Config Store

Centralized configuration storage backed by PostgreSQL with an in-memory cache:

```rust
pub struct ConfigStore {
    db: PgPool,
    cache: Arc<DashMap<String, CachedConfig>>,
}

struct CachedConfig {
    value: serde_json::Value,
    updated_at: DateTime<Utc>,
}

impl ConfigStore {
    /// Get a config value by domain path
    pub async fn get(&self, domain: &ConfigDomain) -> Result<Option<serde_json::Value>> {
        let path = domain.path();

        // Check cache first
        if let Some(cached) = self.cache.get(&path) {
            return Ok(Some(cached.value.clone()));
        }

        // Fall back to database
        let row = sqlx::query!(
            "SELECT value FROM config WHERE path = $1",
            path,
        )
        .fetch_optional(&self.db)
        .await?;

        if let Some(row) = row {
            let value: serde_json::Value = row.value;
            self.cache.insert(path, CachedConfig {
                value: value.clone(),
                updated_at: Utc::now(),
            });
            Ok(Some(value))
        } else {
            Ok(None)
        }
    }

    /// Set a config value
    pub async fn set(
        &self,
        domain: &ConfigDomain,
        value: &serde_json::Value,
    ) -> Result<()> {
        let path = domain.path();

        sqlx::query!(
            r#"
            INSERT INTO config (path, value, updated_at)
            VALUES ($1, $2, NOW())
            ON CONFLICT (path) DO UPDATE SET value = $2, updated_at = NOW()
            "#,
            path,
            value,
        )
        .execute(&self.db)
        .await?;

        // Update cache
        self.cache.insert(path, CachedConfig {
            value: value.clone(),
            updated_at: Utc::now(),
        });

        Ok(())
    }

    /// Get all config values under a domain prefix
    pub async fn get_all(&self, prefix: &str) -> Result<HashMap<String, serde_json::Value>> {
        let rows = sqlx::query!(
            "SELECT path, value FROM config WHERE path LIKE $1",
            format!("{}%", prefix),
        )
        .fetch_all(&self.db)
        .await?;

        Ok(rows.into_iter().map(|r| (r.path, r.value)).collect())
    }

    /// Export all configuration as structured JSON
    pub async fn export_all(&self) -> Result<serde_json::Value> {
        let rows = sqlx::query!(
            "SELECT path, value FROM config ORDER BY path",
        )
        .fetch_all(&self.db)
        .await?;

        let mut config = serde_json::Map::new();
        for row in rows {
            config.insert(row.path, row.value);
        }
        Ok(serde_json::Value::Object(config))
    }
}
```

## 8. Undo Manager

Supports "undo that" to revert the most recent config change:

```rust
pub struct UndoManager {
    /// Stack of recent changes (per session, kept in memory)
    recent_changes: Arc<Mutex<Vec<ConfigChange>>>,
    config_store: ConfigStore,
    audit_trail: AuditTrail,
    event_bus: EventBusTx,
    /// Maximum undo history depth
    max_depth: usize,
}

impl UndoManager {
    pub fn new(config_store: ConfigStore, audit_trail: AuditTrail, event_bus: EventBusTx) -> Self {
        Self {
            recent_changes: Arc::new(Mutex::new(Vec::new())),
            config_store,
            audit_trail,
            event_bus,
            max_depth: 50,
        }
    }

    /// Push a change onto the undo stack
    pub async fn push(&self, change: ConfigChange) -> Result<()> {
        let mut stack = self.recent_changes.lock().await;
        stack.push(change);
        if stack.len() > self.max_depth {
            stack.remove(0);
        }
        Ok(())
    }

    /// Undo the most recent change
    pub async fn undo(&self) -> Result<Option<UndoResult>> {
        let mut stack = self.recent_changes.lock().await;

        // Find the most recent un-undone change
        let idx = stack.iter().rposition(|c| !c.undone);
        let change = match idx {
            Some(i) => &mut stack[i],
            None => return Ok(None), // nothing to undo
        };

        // Restore previous value
        if let Some(previous) = &change.previous_value {
            self.config_store.set(&change.domain, previous).await?;
        } else {
            // If there was no previous value, remove the config entry
            self.config_store.delete(&change.domain).await?;
        }

        change.undone = true;

        // Audit the undo
        self.audit_trail.record(AuditEntry {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            action: "config.undo".into(),
            domain: change.domain.path(),
            old_value: Some(change.new_value.clone()),
            new_value: change.previous_value.clone(),
            description: format!("Undo: {}", change.description),
            source: AuditSource::OwnerConversation,
        }).await?;

        // Notify modules
        if let Some(prev) = &change.previous_value {
            self.event_bus.send(ArgusEvent::ConfigChanged(
                change.domain.clone(),
                prev.clone(),
            ));
        }

        Ok(Some(UndoResult {
            change_id: change.id,
            description: change.description.clone(),
            restored_value: change.previous_value.clone(),
        }))
    }

    /// Undo a specific change by ID
    pub async fn undo_specific(&self, change_id: Uuid) -> Result<Option<UndoResult>> {
        let mut stack = self.recent_changes.lock().await;

        let change = stack.iter_mut().find(|c| c.id == change_id && !c.undone);
        match change {
            Some(change) => {
                if let Some(previous) = &change.previous_value {
                    self.config_store.set(&change.domain, previous).await?;
                }
                change.undone = true;
                Ok(Some(UndoResult {
                    change_id: change.id,
                    description: change.description.clone(),
                    restored_value: change.previous_value.clone(),
                }))
            }
            None => Ok(None),
        }
    }

    /// List recent changes (for "show me recent config changes")
    pub async fn recent_changes(&self, limit: usize) -> Vec<ConfigChange> {
        let stack = self.recent_changes.lock().await;
        stack.iter()
            .rev()
            .take(limit)
            .cloned()
            .collect()
    }
}

#[derive(Debug, Clone)]
pub struct UndoResult {
    pub change_id: Uuid,
    pub description: String,
    pub restored_value: Option<serde_json::Value>,
}
```

## 9. Multi-Turn Dialog Manager

For complex configuration changes that need clarification:

```rust
/// Dialog states for config conversations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConfigDialogState {
    /// Waiting for the user to pick a domain
    AwaitingDomain {
        raw_text: String,
        candidates: Vec<ConfigDomain>,
    },
    /// Domain known, waiting for a value
    AwaitingValue {
        domain: ConfigDomain,
        prompt: String,
        value_type: ValueType,
    },
    /// Have the value, waiting for confirmation
    AwaitingConfirmation {
        change: ConfigChange,
        warnings: Vec<String>,
    },
    /// Complex setup with multiple fields
    MultiField {
        domain: ConfigDomain,
        fields: Vec<FieldSpec>,
        collected: HashMap<String, serde_json::Value>,
        current_field: usize,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldSpec {
    pub name: String,
    pub display_name: String,
    pub value_type: ValueType,
    pub required: bool,
    pub default: Option<serde_json::Value>,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ValueType {
    String,
    Integer,
    Float,
    Boolean,
    Duration,
    Url,
    Path,
    OneOf(Vec<String>),
    Secret,  // will be stored encrypted
}

pub struct ConfigDialogManager {
    active_dialogs: DashMap<SessionId, ConfigDialogState>,
    resolver: ConfigResolver,
    change_builder: ChangeBuilder,
    applier: ConfigApplier,
}

impl ConfigDialogManager {
    /// Handle a message in the context of an active config dialog
    pub async fn handle_message(
        &self,
        session_id: &SessionId,
        message: &str,
    ) -> Result<ConfigDialogResponse> {
        // Check if there's an active dialog
        if let Some(state) = self.active_dialogs.get(session_id) {
            self.continue_dialog(session_id, &state, message).await
        } else {
            // Start a new dialog
            self.start_dialog(session_id, message).await
        }
    }

    async fn continue_dialog(
        &self,
        session_id: &SessionId,
        state: &ConfigDialogState,
        message: &str,
    ) -> Result<ConfigDialogResponse> {
        match state {
            ConfigDialogState::AwaitingDomain { candidates, .. } => {
                // User should have clarified which domain
                let domain = self.resolver.match_domain(message, candidates)?;
                // Transition to AwaitingValue or MultiField
                self.transition_to_value_collection(session_id, domain).await
            }

            ConfigDialogState::AwaitingValue { domain, value_type, .. } => {
                // Parse the value from user's message
                let value = self.parse_value(message, value_type)?;
                let change = self.change_builder.build(&ConfigIntent::SetValue {
                    domain: domain.clone(),
                    value,
                }).await?;

                // Validate and check for warnings
                let validation = ConfigValidator.validate(&change);
                if !validation.warnings.is_empty() {
                    self.active_dialogs.insert(*session_id, ConfigDialogState::AwaitingConfirmation {
                        change: change.clone(),
                        warnings: validation.warnings.clone(),
                    });
                    Ok(ConfigDialogResponse::NeedsConfirmation {
                        change_summary: change.description,
                        warnings: validation.warnings,
                    })
                } else {
                    self.active_dialogs.remove(session_id);
                    let result = self.applier.apply(change).await?;
                    Ok(ConfigDialogResponse::Applied(result))
                }
            }

            ConfigDialogState::AwaitingConfirmation { change, .. } => {
                // User confirms or cancels
                let confirmed = self.is_confirmation(message);
                self.active_dialogs.remove(session_id);

                if confirmed {
                    let result = self.applier.apply(change.clone()).await?;
                    Ok(ConfigDialogResponse::Applied(result))
                } else {
                    Ok(ConfigDialogResponse::Cancelled)
                }
            }

            ConfigDialogState::MultiField {
                domain, fields, collected, current_field,
            } => {
                // Collect current field value
                let field = &fields[*current_field];
                let value = self.parse_value(message, &field.value_type)?;
                let mut collected = collected.clone();
                collected.insert(field.name.clone(), value);

                let next = current_field + 1;
                if next < fields.len() {
                    // More fields to collect
                    self.active_dialogs.insert(*session_id, ConfigDialogState::MultiField {
                        domain: domain.clone(),
                        fields: fields.clone(),
                        collected,
                        current_field: next,
                    });
                    Ok(ConfigDialogResponse::AskField {
                        field: fields[next].clone(),
                        progress: format!("{}/{}", next + 1, fields.len()),
                    })
                } else {
                    // All fields collected, build and apply
                    self.active_dialogs.remove(session_id);
                    let value = serde_json::to_value(&collected)?;
                    let change = self.change_builder.build(&ConfigIntent::SetValue {
                        domain: domain.clone(),
                        value,
                    }).await?;
                    let result = self.applier.apply(change).await?;
                    Ok(ConfigDialogResponse::Applied(result))
                }
            }
        }
    }

    /// Check if a message is a confirmation (yes/no)
    fn is_confirmation(&self, message: &str) -> bool {
        let msg = message.trim().to_lowercase();
        matches!(
            msg.as_str(),
            "yes" | "y" | "ok" | "确认" | "好" | "好的" | "是" | "是的"
                | "对" | "没问题" | "可以" | "行" | "嗯" | "approve"
        )
    }
}

pub enum ConfigDialogResponse {
    /// Change applied successfully
    Applied(ApplyResult),
    /// Need user to pick from ambiguous options
    Clarify {
        question: String,
        options: Vec<String>,
    },
    /// Need user to provide a value
    AskValue {
        prompt: String,
        value_type: ValueType,
        current_value: Option<serde_json::Value>,
    },
    /// Need user to confirm due to warnings
    NeedsConfirmation {
        change_summary: String,
        warnings: Vec<String>,
    },
    /// Ask for a specific field in a multi-field setup
    AskField {
        field: FieldSpec,
        progress: String,
    },
    /// User cancelled
    Cancelled,
}
```

## 10. Audit Trail

All config changes are logged for accountability and debugging:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub action: String,
    pub domain: String,
    pub old_value: Option<serde_json::Value>,
    pub new_value: Option<serde_json::Value>,
    pub description: String,
    pub source: AuditSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuditSource {
    OwnerConversation,
    AutoConfig,        // Argus adjusted config automatically
    SystemDefault,     // Initial default value
    Undo,
}

pub struct AuditTrail {
    db: PgPool,
}

impl AuditTrail {
    /// Record an audit entry
    pub async fn record(&self, entry: AuditEntry) -> Result<()> {
        sqlx::query!(
            r#"
            INSERT INTO config_audit_log (
                id, timestamp, action, domain, old_value, new_value,
                description, source
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
            entry.id,
            entry.timestamp,
            entry.action,
            entry.domain,
            entry.old_value,
            entry.new_value,
            entry.description,
            serde_json::to_value(&entry.source)?,
        )
        .execute(&self.db)
        .await?;

        Ok(())
    }

    /// Query audit history
    pub async fn query(
        &self,
        filter: AuditFilter,
    ) -> Result<Vec<AuditEntry>> {
        let rows = sqlx::query!(
            r#"
            SELECT * FROM config_audit_log
            WHERE ($1::text IS NULL OR domain LIKE $1)
              AND ($2::timestamptz IS NULL OR timestamp >= $2)
              AND ($3::timestamptz IS NULL OR timestamp <= $3)
            ORDER BY timestamp DESC
            LIMIT $4
            "#,
            filter.domain_prefix,
            filter.since,
            filter.until,
            filter.limit.unwrap_or(50) as i64,
        )
        .fetch_all(&self.db)
        .await?;

        // TODO: Implement deserialization of audit trail rows into AuditEntry structs
        // Query: SELECT id, domain, key, old_value, new_value, changed_by, changed_at
        //        FROM config_audit WHERE domain LIKE $1 AND changed_at BETWEEN $2 AND $3
        //        ORDER BY changed_at DESC LIMIT $4
        // Map each row to AuditEntry { domain, key, old_value, new_value, actor, timestamp }
        Ok(Vec::new()) // placeholder
    }
}

pub struct AuditFilter {
    pub domain_prefix: Option<String>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    pub limit: Option<usize>,
}
```

## 11. Conversational Examples

### 11.1 Simple Config Change

```
Owner: 最多同时开3个worker

Argus: 好的，worker 最大并发数已从 5 改为 3。
       （如需撤销，说"撤销"即可）
```

### 11.2 Config with Confirmation

```
Owner: 关闭沙箱模式

Argus: ⚠️ 关闭沙箱模式会移除 worker 的隔离保护，存在安全风险。
       Worker 将直接访问宿主文件系统和网络。

       确定要关闭吗？

Owner: 算了，不关了

Argus: 好的，沙箱模式保持开启。
```

### 11.3 Multi-Turn Dialog (New Provider Setup)

```
Owner: 添加 Gemini

Argus: 好的，添加 Google Gemini 作为 LLM 提供商。我需要以下信息：

       1/3: 请提供 API Key（输入后我会加密存储）

Owner: AIzaSyD_xxxxxxxxxxxxxxxxxxxxx

Argus: API Key 已加密存储。

       2/3: 使用哪些模型？
       - gemini-2.0-flash（快速，便宜）
       - gemini-2.0-pro（平衡）
       - gemini-2.5-pro（强推理）
       
       可以多选，用逗号分隔。

Owner: flash 和 pro

Argus: 3/3: 月预算限制？（美元，留空不限制）

Owner: 20

Argus: ✓ Gemini 已添加完成：
       ┌──────────────────────────────┐
       │ Provider: Google Gemini       │
       │ Models: flash, pro            │
       │ Budget: $20/月                │
       │ Status: 已启用                 │
       │ Failover 位置: #4 (在 Qwen 后) │
       └──────────────────────────────┘
```

### 11.4 Undo

```
Owner: 撤销刚才的修改

Argus: 已撤销：将 worker 最大并发数从 3 恢复为 5。
```

### 11.5 Undo Specific

```
Owner: 撤销之前添加 Gemini 的操作

Argus: 已撤销添加 Gemini。该提供商已移除，API Key 已从密钥库中删除。
```

### 11.6 Query Current Config

```
Owner: 现在的 LLM 配置是什么

Argus: 📋 **当前 LLM 配置**

┌───────────────┬──────────┬──────────┬─────────┐
│ Provider       │ Status   │ Budget   │ Priority│
├───────────────┼──────────┼──────────┼─────────┤
│ DeepSeek       │ ✓ 启用   │ $30/月   │ 1       │
│ OpenAI         │ ✓ 启用   │ $50/月   │ 2       │
│ Anthropic      │ ✓ 启用   │ $50/月   │ 3       │
│ Qwen (通义)    │ ✓ 启用   │ ¥100/月  │ 4       │
│ Ollama (本地)  │ ✓ 启用   │ 免费     │ 5       │
│ Web Scraper    │ ✓ 备用   │ 免费     │ 6       │
└───────────────┴──────────┴──────────┴─────────┘

全局月预算: $200
本月已用: $47.32 (23.7%)
默认模型: deepseek-chat (快速任务), claude-sonnet-4-20250514 (复杂任务)
```

### 11.7 Complex Config Change

```
Owner: 晚上别打扰我，除非是生产事故

Argus: 明白。我来调整通知设置：

       ✓ 活跃时间: 08:00 - 23:00（之前: 全天）
       ✓ 非活跃时段只推送 Critical 级别通知
       ✓ 生产部署失败 → 立即通知（不受时段限制）
       ✓ 其他消息 → 缓存到早上 08:00 汇总推送

       这样设置可以吗？

Owner: 好的

Argus: 已生效。晚间仅在生产事故时打扰你，其他消息明早汇总。
```

### 11.8 Batch Config

```
Owner: 新项目 my-shop 设置一下：用 Next.js，部署到 Vercel，
       main 分支自动部署，测试用 jest

Argus: 已为 **my-shop** 配置：

       ┌────────────────────────────────┐
       │ 框架: Next.js                   │
       │ 测试: jest (npm test)           │
       │ 部署: Vercel                    │
       │ 自动部署: main → production     │
       │ 审批: 不需要 (Vercel 支持回滚)   │
       │ 健康检查: /api/health HTTP 200  │
       └────────────────────────────────┘

       Vercel token 已有（之前配置过），
       会复用现有的。需要修改什么吗？
```

### 11.9 Config Audit

```
Owner: 最近改了什么配置

Argus: 📋 **最近配置变更** (过去7天)

┌────────────────┬─────────────────────────────────┬──────────┐
│ 时间            │ 变更                             │ 状态      │
├────────────────┼─────────────────────────────────┼──────────┤
│ 2分钟前         │ Worker 最大并发: 5→3→5 (已撤销)  │ 已撤销   │
│ 10分钟前        │ 添加 Gemini provider             │ 已撤销   │
│ 1小时前         │ 通知时间: 全天→08-23时            │ 生效中   │
│ 昨天 14:30      │ my-shop 项目部署配置              │ 生效中   │
│ 昨天 10:15      │ DeepSeek 预算: $20→$30           │ 生效中   │
│ 3天前           │ 沙箱模式: 目录→Docker             │ 生效中   │
└────────────────┴─────────────────────────────────┴──────────┘
```

## 12. Auto-Config (Argus Self-Adjustment)

Argus can automatically adjust certain configurations based on observed patterns:

```rust
pub struct AutoConfigManager {
    config_store: ConfigStore,
    applier: ConfigApplier,
    event_bus: EventBusTx,
}

impl AutoConfigManager {
    /// Periodically analyze usage patterns and suggest/apply optimizations
    pub async fn analyze_and_adjust(&self) -> Result<Vec<AutoAdjustment>> {
        let mut adjustments = vec![];

        // 1. Worker pool sizing
        // If workers are consistently idle, suggest reducing pool size
        let worker_utilization = self.get_worker_utilization().await?;
        if worker_utilization < 0.2 {
            adjustments.push(AutoAdjustment {
                domain: ConfigDomain::Worker(WorkerConfigKey::SetMaxWorkers),
                suggestion: "Worker utilization is below 20%. Consider reducing max_workers.".into(),
                auto_apply: false, // needs owner approval
                new_value: serde_json::json!(3),
            });
        }

        // 2. LLM routing optimization
        // If a cheaper model handles tasks well, route more tasks to it
        let model_performance = self.get_model_performance().await?;
        for (model, stats) in &model_performance {
            if stats.success_rate > 0.95 && stats.is_cheaper_alternative {
                adjustments.push(AutoAdjustment {
                    domain: ConfigDomain::Llm(LlmConfigKey::SetModelForTask {
                        task_type: stats.task_type.clone(),
                    }),
                    suggestion: format!(
                        "{} handles {} tasks with 95%+ success rate and is cheaper. Switching.",
                        model, stats.task_type
                    ),
                    auto_apply: true, // safe to auto-apply
                    new_value: serde_json::json!(model),
                });
            }
        }

        // 3. Memory tuning
        // Adjust decay rate based on retrieval patterns

        // Apply auto-adjustments or notify owner for approval
        for adj in &adjustments {
            if adj.auto_apply {
                let change = ConfigChange {
                    id: Uuid::new_v4(),
                    domain: adj.domain.clone(),
                    action: ConfigAction::Set,
                    previous_value: self.config_store.get(&adj.domain).await?,
                    new_value: adj.new_value.clone(),
                    description: format!("[Auto] {}", adj.suggestion),
                    applied_at: Utc::now(),
                    undone: false,
                    trigger_message: "Auto-adjustment".into(),
                };
                self.applier.apply(change).await?;
            }
        }

        Ok(adjustments)
    }
}

#[derive(Debug, Clone)]
pub struct AutoAdjustment {
    pub domain: ConfigDomain,
    pub suggestion: String,
    pub auto_apply: bool,
    pub new_value: serde_json::Value,
}
```

## 13. Event Integration

| Event | Direction | Description |
|-------|-----------|-------------|
| `ConfigChanged(ConfigDomain, Value)` | Emits | A config value was changed |
| `ConfigUndone(ConfigDomain, Value)` | Emits | A config change was undone |
| `ConfigValidationWarning(String)` | Emits | A change was applied with warnings |
| `OwnerMessage(message)` | Listens | Incoming messages may be config intents |

## 14. Module Hot-Reload

When config changes, affected modules reload dynamically without restarting Argus:

| Config Domain | Reload Method |
|---------------|---------------|
| LLM providers/keys | Hot-swap provider in `LlmRouter` |
| Memory settings | Update parameters in `MemoryEngine` |
| Worker pool size | Scale pool up/down in `WorkerManager` |
| Notification settings | Update `NotificationEngine` filters |
| Deploy settings | Update `EnvironmentConfig` in `DeployEngine` |
| Security rules | Reload `PermissionChecker` rules |
| Scheduler settings | Update `Scheduler` parameters |
| Log level | `tracing` subscriber reload |

Each module subscribes to `ConfigChanged` events for its domain and applies changes without restart:

```rust
// Example: Worker Manager listening for config changes
async fn handle_config_change(&self, domain: ConfigDomain, value: serde_json::Value) {
    match domain {
        ConfigDomain::Worker(WorkerConfigKey::SetMaxWorkers) => {
            let new_max = value.as_u64().unwrap_or(5) as usize;
            self.pool.set_max_workers(new_max).await;
            tracing::info!("Worker pool max_workers updated to {}", new_max);
        }
        ConfigDomain::Worker(WorkerConfigKey::SetWorkerTimeout) => {
            let timeout_secs = value.as_u64().unwrap_or(3600);
            self.pool.set_timeout(Duration::from_secs(timeout_secs)).await;
        }
        _ => {}
    }
}
```
