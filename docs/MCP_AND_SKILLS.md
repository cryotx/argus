# MCP & Skills System — Detailed Design

> **Crates**: `argus-mcp`, `argus-skills`

Argus acts as an MCP (Model Context Protocol) **Host**, calling external MCP Servers to extend its capabilities. The Skills System provides a higher-level abstraction over both MCP tools and native Rust functions.

## 1. Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                  MCP & Skills Engine                             │
│                                                                  │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │                  Tool Router                                │ │
│  │  Brain requests tool → Router finds MCP server or Skill     │ │
│  └────────────────────────┬───────────────────────────────────┘ │
│                           │                                      │
│  ┌────────────────────────┴───────────────────────────────────┐ │
│  │              MCP Client Manager                             │ │
│  │                                                              │ │
│  │  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────────┐  │ │
│  │  │Built-in  │ │Verified  │ │Third-    │ │Self-         │  │ │
│  │  │Servers   │ │Servers   │ │party     │ │generated     │  │ │
│  │  │(fs,shell)│ │(audited) │ │(scanned) │ │(by Argus)    │  │ │
│  │  └──────────┘ └──────────┘ └──────────┘ └──────────────┘  │ │
│  └────────────────────────────────────────────────────────────┘ │
│                                                                  │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │              Skills Engine                                  │ │
│  │                                                              │ │
│  │  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────────┐  │ │
│  │  │Built-in  │ │Third-    │ │Self-     │ │Skill         │  │ │
│  │  │Skills    │ │party     │ │generated │ │Generator     │  │ │
│  │  │(~20)     │ │Skills    │ │Skills    │ │(LLM writes)  │  │ │
│  │  └──────────┘ └──────────┘ └──────────┘ └──────────────┘  │ │
│  └────────────────────────────────────────────────────────────┘ │
│                                                                  │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │              Security Auditor                               │ │
│  │  Static scan + Behavioral analysis + LLM review             │ │
│  └────────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

## 2. MCP Client Manager

### 2.1 MCP Protocol Support

Argus implements the MCP client side, supporting all three transports:

```rust
enum McpTransport {
    /// Standard I/O — MCP server runs as a subprocess
    Stdio {
        command: String,
        args: Vec<String>,
        env: HashMap<String, String>,
    },
    
    /// Server-Sent Events — MCP server runs as HTTP endpoint
    Sse {
        url: String,
        headers: HashMap<String, String>,
    },
    
    /// Streamable HTTP — newer MCP transport
    StreamableHttp {
        url: String,
        headers: HashMap<String, String>,
    },
}

struct McpClient {
    server_id: String,
    transport: McpTransport,
    trust_level: TrustLevel,
    
    // Discovered capabilities
    tools: Vec<McpTool>,
    resources: Vec<McpResource>,
    prompts: Vec<McpPrompt>,
    
    // State
    process: Option<tokio::process::Child>,  // For stdio transport
    is_connected: bool,
    last_ping: Option<DateTime<Utc>>,
}

struct McpTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,  // JSON Schema
}

impl McpClient {
    /// Connect to the MCP server and discover capabilities
    async fn connect(&mut self) -> Result<()> {
        match &self.transport {
            McpTransport::Stdio { command, args, env } => {
                let child = tokio::process::Command::new(command)
                    .args(args)
                    .envs(env.iter())
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()?;
                
                self.process = Some(child);
                
                // Send initialize request
                self.send_jsonrpc("initialize", json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {
                        "name": "argus",
                        "version": env!("CARGO_PKG_VERSION"),
                    }
                })).await?;
                
                // Discover tools
                let tools_response = self.send_jsonrpc("tools/list", json!({})).await?;
                self.tools = serde_json::from_value(tools_response["tools"].clone())?;
                
                self.is_connected = true;
            }
            McpTransport::Sse { url, headers } => {
                // HTTP-based connection
                // ...
            }
            McpTransport::StreamableHttp { url, headers } => {
                // Streamable HTTP connection
                // ...
            }
        }
        
        Ok(())
    }
    
    /// Call a tool on the MCP server
    async fn call_tool(&self, tool_name: &str, arguments: serde_json::Value) -> Result<serde_json::Value> {
        // Security check
        if !self.is_tool_allowed(tool_name, &arguments) {
            return Err(Error::ToolBlocked(tool_name.into()));
        }
        
        let response = self.send_jsonrpc("tools/call", json!({
            "name": tool_name,
            "arguments": arguments,
        })).await?;
        
        Ok(response)
    }
}
```

### 2.2 Server Registry

```rust
struct McpServerRegistry {
    /// All registered MCP servers
    servers: HashMap<String, McpServerEntry>,
    
    /// Community server index (cached)
    community_index: Vec<CommunityServer>,
    
    /// Last index refresh
    index_refreshed_at: Option<DateTime<Utc>>,
}

struct McpServerEntry {
    id: String,
    name: String,
    description: String,
    transport: McpTransport,
    trust_level: TrustLevel,
    
    // Audit results
    audit_result: Option<AuditResult>,
    audited_at: Option<DateTime<Utc>>,
    
    // State
    enabled: bool,
    auto_start: bool,
    client: Option<McpClient>,
    
    // Usage stats
    total_calls: u64,
    last_used_at: Option<DateTime<Utc>>,
}

enum TrustLevel {
    /// Built-in servers (fs, shell, db) — full trust
    Builtin,
    
    /// Verified by Argus's security audit — high trust
    Verified,
    
    /// Third-party, passed basic scan — limited trust
    ThirdParty,
    
    /// Untrusted — heavily sandboxed
    Untrusted,
}
```

### 2.3 Built-in MCP Servers

Argus ships with built-in MCP servers for core operations:

| Server | Tools | Description |
|--------|-------|-------------|
| `argus-fs` | read_file, write_file, list_dir, search_files, create_dir | Filesystem operations |
| `argus-shell` | run_command, run_script | Shell command execution |
| `argus-db` | query, execute, schema | PostgreSQL operations |
| `argus-git` | status, commit, push, pull, branch, diff, log | Git operations |
| `argus-browser` | navigate, click, type, screenshot, extract | Browser automation (via Playwright) |

### 2.4 Auto-Discovery

```rust
impl McpServerRegistry {
    /// Discover MCP servers from community registries
    async fn discover(&mut self, query: &str) -> Vec<CommunityServer> {
        // 1. Search community registries
        let results = self.search_registries(query).await;
        
        // 2. Rank by relevance, popularity, and trust
        let ranked = self.rank_results(results);
        
        ranked
    }
    
    /// Conversational installation flow
    async fn install_interactively(
        &mut self,
        server: &CommunityServer,
        conversation: &mut Conversation,
    ) -> Result<String> {
        // 1. Security audit
        conversation.send("正在对该MCP Server进行安全审计...").await;
        let audit = self.security_auditor.audit(server).await?;
        
        match audit.verdict {
            AuditVerdict::Safe => {
                conversation.send(&format!(
                    "安全审计通过 ✓\n信任等级: {:?}\n开始安装...",
                    audit.trust_level
                )).await;
            }
            AuditVerdict::Warning { warnings } => {
                let msg = format!(
                    "安全审计发现以下警告:\n{}\n是否继续安装？",
                    warnings.join("\n")
                );
                let approved = conversation.ask_confirmation(&msg).await?;
                if !approved {
                    return Err(Error::InstallationCancelled);
                }
            }
            AuditVerdict::Dangerous { reasons } => {
                conversation.send(&format!(
                    "安全审计未通过 ✗\n原因:\n{}\n已拒绝安装。",
                    reasons.join("\n")
                )).await;
                return Err(Error::SecurityAuditFailed);
            }
        }
        
        // 2. Install dependencies
        if let Some(deps) = &server.dependencies {
            conversation.send(&format!("安装依赖: {}", deps.join(", "))).await;
            self.install_dependencies(deps).await?;
        }
        
        // 3. Configure
        let transport = self.configure_transport(server).await?;
        
        // 4. Register
        let entry = McpServerEntry {
            id: server.id.clone(),
            name: server.name.clone(),
            description: server.description.clone(),
            transport,
            trust_level: audit.trust_level,
            audit_result: Some(audit),
            enabled: true,
            auto_start: true,
            ..Default::default()
        };
        
        self.servers.insert(server.id.clone(), entry);
        
        // 5. Connect and verify
        let client = self.connect_server(&server.id).await?;
        let tools: Vec<String> = client.tools.iter().map(|t| t.name.clone()).collect();
        
        conversation.send(&format!(
            "安装完成！可用工具:\n{}",
            tools.iter().map(|t| format!("  - {}", t)).collect::<Vec<_>>().join("\n")
        )).await;
        
        Ok(server.id.clone())
    }
}
```

## 3. Security Auditor

### 3.1 Audit Pipeline

```
MCP Server Source Code / Package
         │
         ▼
┌────────────────────┐
│ 1. Static Scan     │
│ - Dependency check  │
│ - Known CVE scan    │
│ - Malicious pattern │
│ - Obfuscated code   │
└────────┬───────────┘
         │
         ▼
┌────────────────────┐
│ 2. Behavioral      │
│    Analysis         │
│ - Network access    │
│ - File system scope │
│ - Process spawning  │
│ - Data exfil check  │
└────────┬───────────┘
         │
         ▼
┌────────────────────┐
│ 3. LLM Review      │
│ - Code purpose      │
│ - Backdoor detection│
│ - Permission scope  │
│ - Trust assessment  │
└────────┬───────────┘
         │
         ▼
┌────────────────────┐
│ 4. Verdict          │
│ Safe / Warning /    │
│ Dangerous           │
└────────────────────┘
```

### 3.2 Implementation

```rust
struct SecurityAuditor {
    llm: Arc<LlmEngine>,
    cve_database: CveDatabase,
}

struct AuditResult {
    verdict: AuditVerdict,
    trust_level: TrustLevel,
    
    // Detailed findings
    static_scan: StaticScanResult,
    behavioral: BehavioralResult,
    llm_review: LlmReviewResult,
    
    // Risk score (0.0 = safe, 1.0 = dangerous)
    risk_score: f32,
    
    timestamp: DateTime<Utc>,
}

enum AuditVerdict {
    Safe,
    Warning { warnings: Vec<String> },
    Dangerous { reasons: Vec<String> },
}

impl SecurityAuditor {
    async fn audit(&self, server: &CommunityServer) -> Result<AuditResult> {
        // 1. Static Scan
        let static_result = self.static_scan(server).await?;
        if static_result.has_critical_issues() {
            return Ok(AuditResult {
                verdict: AuditVerdict::Dangerous {
                    reasons: static_result.critical_issues.clone(),
                },
                risk_score: 1.0,
                ..Default::default()
            });
        }
        
        // 2. Behavioral Analysis
        let behavioral = self.behavioral_analysis(server).await?;
        
        // 3. LLM Review
        let llm_review = self.llm_review(server, &static_result, &behavioral).await?;
        
        // 4. Compute verdict
        let risk_score = self.compute_risk_score(&static_result, &behavioral, &llm_review);
        
        let verdict = if risk_score < 0.3 {
            AuditVerdict::Safe
        } else if risk_score < 0.7 {
            let warnings = self.collect_warnings(&static_result, &behavioral, &llm_review);
            AuditVerdict::Warning { warnings }
        } else {
            let reasons = self.collect_dangers(&static_result, &behavioral, &llm_review);
            AuditVerdict::Dangerous { reasons }
        };
        
        let trust_level = match &verdict {
            AuditVerdict::Safe => TrustLevel::Verified,
            AuditVerdict::Warning { .. } => TrustLevel::ThirdParty,
            AuditVerdict::Dangerous { .. } => TrustLevel::Untrusted,
        };
        
        Ok(AuditResult {
            verdict,
            trust_level,
            static_scan: static_result,
            behavioral,
            llm_review,
            risk_score,
            timestamp: Utc::now(),
        })
    }
    
    /// Static code scan
    async fn static_scan(&self, server: &CommunityServer) -> Result<StaticScanResult> {
        let mut issues = Vec::new();
        
        // Check for known CVEs in dependencies
        if let Some(deps) = &server.dependencies {
            for dep in deps {
                if let Some(cves) = self.cve_database.check(dep).await {
                    for cve in cves {
                        issues.push(ScanIssue {
                            severity: cve.severity,
                            description: format!("Known CVE in {}: {}", dep, cve.id),
                        });
                    }
                }
            }
        }
        
        // Scan source for suspicious patterns
        let patterns = vec![
            (r"eval\(", "Dynamic code execution (eval)"),
            (r"exec\(", "Process execution"),
            (r"child_process", "Child process spawning"),
            (r"net\.connect|http\.request", "Outbound network connections"),
            (r"Buffer\.from.*base64", "Base64 encoded data (potential obfuscation)"),
            (r"process\.env\.", "Environment variable access"),
            (r"fs\.readFile.*\/etc\/", "Reading sensitive system files"),
            (r"crypto\.createCipher", "Encryption (potential data exfiltration)"),
        ];
        
        for (pattern, description) in patterns {
            if server.source_contains(pattern).await {
                issues.push(ScanIssue {
                    severity: Severity::Warning,
                    description: description.into(),
                });
            }
        }
        
        Ok(StaticScanResult { issues })
    }
    
    /// LLM-based code review
    async fn llm_review(
        &self,
        server: &CommunityServer,
        static_result: &StaticScanResult,
        behavioral: &BehavioralResult,
    ) -> Result<LlmReviewResult> {
        let source = server.get_source_code().await?;
        
        let prompt = format!(
            "You are a security auditor reviewing an MCP server for the Argus system.\n\
             Analyze this code for:\n\
             1. Backdoors or malicious behavior\n\
             2. Data exfiltration attempts\n\
             3. Excessive permissions beyond stated purpose\n\
             4. Code obfuscation\n\
             5. Overall trustworthiness\n\n\
             Server description: {}\n\
             Static scan findings: {:?}\n\
             Behavioral analysis: {:?}\n\n\
             Source code:\n```\n{}\n```\n\n\
             Return JSON: {{\n\
               \"is_safe\": bool,\n\
               \"confidence\": 0.0-1.0,\n\
               \"findings\": [\"...\"],\n\
               \"recommendation\": \"safe|warning|dangerous\",\n\
               \"reasoning\": \"...\"\n\
             }}",
            server.description,
            static_result.issues,
            behavioral,
            &source[..source.len().min(50000)],  // Limit source length
        );
        
        let response = self.llm.chat(ModelClass::Reasoning, &[
            Message::system("You are an expert security auditor."),
            Message::user(&prompt),
        ]).await?;
        
        let review: LlmReviewResult = serde_json::from_str(&response)?;
        Ok(review)
    }
}
```

## 4. Tool Router

The Tool Router maps Brain Engine tool-use requests to the appropriate MCP server or Skill:

```rust
struct ToolRouter {
    mcp_registry: Arc<McpServerRegistry>,
    skills_engine: Arc<SkillsEngine>,
}

impl ToolRouter {
    /// Route a tool call to the right handler
    async fn route_tool_call(
        &self,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value> {
        // 1. Check built-in MCP servers first
        if let Some(server) = self.mcp_registry.find_tool_server(tool_name, TrustLevel::Builtin) {
            return server.client.call_tool(tool_name, arguments).await;
        }
        
        // 2. Check skills
        if let Some(skill) = self.skills_engine.find_skill(tool_name) {
            return skill.execute(arguments).await;
        }
        
        // 3. Check verified MCP servers
        if let Some(server) = self.mcp_registry.find_tool_server(tool_name, TrustLevel::Verified) {
            return server.client.call_tool(tool_name, arguments).await;
        }
        
        // 4. Check third-party MCP servers
        if let Some(server) = self.mcp_registry.find_tool_server(tool_name, TrustLevel::ThirdParty) {
            return server.client.call_tool(tool_name, arguments).await;
        }
        
        // 5. Tool not found — suggest installation
        let suggestions = self.suggest_tool_sources(tool_name).await;
        Err(Error::ToolNotFound {
            name: tool_name.into(),
            suggestions,
        })
    }
    
    /// List all available tools across MCP servers and Skills
    async fn list_all_tools(&self) -> Vec<AvailableTool> {
        let mut tools = Vec::new();
        
        for server in self.mcp_registry.servers.values() {
            if let Some(client) = &server.client {
                for tool in &client.tools {
                    tools.push(AvailableTool {
                        name: tool.name.clone(),
                        description: tool.description.clone(),
                        source: ToolSource::Mcp(server.id.clone()),
                        trust_level: server.trust_level,
                    });
                }
            }
        }
        
        for skill in self.skills_engine.list_skills() {
            tools.push(AvailableTool {
                name: skill.name.clone(),
                description: skill.description.clone(),
                source: ToolSource::Skill(skill.id.clone()),
                trust_level: TrustLevel::Builtin,
            });
        }
        
        tools
    }
}
```

## 5. Skills System

### 5.1 Skill Manifest

```rust
struct SkillManifest {
    id: String,
    name: String,
    version: String,
    description: String,
    author: String,
    
    /// Required permissions
    permissions: Vec<SkillPermission>,
    
    /// Input schema
    input_schema: serde_json::Value,
    
    /// Output schema
    output_schema: serde_json::Value,
    
    /// Skill type
    skill_type: SkillType,
    
    /// Tags for discovery
    tags: Vec<String>,
}

enum SkillPermission {
    FileSystem { paths: Vec<String>, write: bool },
    Network { hosts: Vec<String> },
    Shell { allowed_commands: Vec<String> },
    Database { read: bool, write: bool },
    Mcp { servers: Vec<String> },
    Llm { max_tokens: Option<u64> },
    Git { write: bool },
}

enum SkillType {
    /// Compiled Rust function (built-in)
    Native,
    
    /// Script (Python, JavaScript, shell)
    Script { runtime: String },
    
    /// LLM prompt template
    Prompt,
    
    /// Composite (calls other skills)
    Composite,
}
```

### 5.2 Built-in Skills (~20)

```rust
// Core development skills
mod builtin {
    pub struct ScaffoldProject;      // Create project from template
    pub struct CodeGenerate;         // Generate code from description
    pub struct CodeReview;           // AI code review
    pub struct CodeRefactor;         // Refactor code
    pub struct WriteTests;           // Generate unit/integration tests
    pub struct RunTests;             // Execute test suite
    pub struct FixBug;               // Diagnose and fix bugs
    pub struct DebugAnalyze;         // Analyze error logs
    
    // Infrastructure skills
    pub struct SetupServer;          // Configure server (Nginx, systemd, etc.)
    pub struct DockerBuild;          // Build and manage Docker images
    pub struct DatabaseMigrate;      // Run database migrations
    pub struct SslSetup;             // Set up SSL certificates (Let's Encrypt)
    
    // Documentation skills
    pub struct GenerateDoc;          // Generate documentation
    pub struct WriteChangelog;       // Generate changelog from Git history
    pub struct CreateReadme;         // Generate README.md
    
    // Analysis skills
    pub struct AnalyzeCodebase;      // Analyze codebase structure and metrics
    pub struct SecurityScan;         // Scan for security vulnerabilities
    pub struct PerformanceProfile;   // Profile and suggest optimizations
    
    // Operations skills
    pub struct MonitorHealth;        // Check service health
    pub struct BackupData;           // Backup databases and files
    pub struct RestoreData;          // Restore from backup
}
```

### 5.3 Skill Execution

```rust
#[async_trait]
trait Skill: Send + Sync {
    fn manifest(&self) -> &SkillManifest;
    
    async fn execute(
        &self,
        input: serde_json::Value,
        context: &SkillContext,
    ) -> Result<serde_json::Value>;
    
    fn validate_input(&self, input: &serde_json::Value) -> Result<()> {
        // Validate against input_schema
        jsonschema::validate(input, &self.manifest().input_schema)?;
        Ok(())
    }
}

struct SkillContext {
    /// Project this skill is running for
    project_id: Option<Uuid>,
    
    /// Working directory
    workspace_dir: PathBuf,
    
    /// Access to LLM
    llm: Arc<LlmEngine>,
    
    /// Access to memory
    memory: Arc<MemoryEngine>,
    
    /// Access to other tools
    tool_router: Arc<ToolRouter>,
    
    /// Permission checker
    permissions: PermissionChecker,
}

struct SkillsEngine {
    skills: HashMap<String, Box<dyn Skill>>,
    permission_checker: PermissionChecker,
}

impl SkillsEngine {
    async fn execute_skill(
        &self,
        skill_name: &str,
        input: serde_json::Value,
        context: &SkillContext,
    ) -> Result<serde_json::Value> {
        let skill = self.skills.get(skill_name)
            .ok_or(Error::SkillNotFound(skill_name.into()))?;
        
        // 1. Validate input
        skill.validate_input(&input)?;
        
        // 2. Check permissions
        self.permission_checker.check(skill.manifest(), context)?;
        
        // 3. Execute with timeout
        let timeout = Duration::from_secs(300); // 5 min default
        match tokio::time::timeout(timeout, skill.execute(input, context)).await {
            Ok(result) => result,
            Err(_) => Err(Error::SkillTimeout(skill_name.into())),
        }
    }
}
```

### 5.4 Self-Generated Skills

When Argus encounters a need for a capability it doesn't have, it can write a new skill:

```rust
struct SkillGenerator {
    llm: Arc<LlmEngine>,
    skills_engine: Arc<SkillsEngine>,
    security_auditor: Arc<SecurityAuditor>,
    event_bus: EventBusTx,
}

impl SkillGenerator {
    async fn generate_skill(
        &self,
        description: &str,
        examples: &[(&str, &str)],  // (input, expected_output)
    ) -> Result<GeneratedSkill> {
        // 1. Generate skill code
        let prompt = format!(
            "Generate a new Argus skill. The skill should:\n\
             {}\n\n\
             Requirements:\n\
             - Write a Python script that can be called with JSON input on stdin\n\
             - Output JSON result on stdout\n\
             - Handle errors gracefully\n\
             - Include a manifest with required permissions\n\n\
             Examples:\n{}\n\n\
             Return JSON:\n\
             {{\n\
               \"manifest\": {{ ... }},\n\
               \"code\": \"...\",\n\
               \"test_cases\": [ ... ]\n\
             }}",
            description,
            examples.iter()
                .map(|(i, o)| format!("  Input: {}\n  Output: {}", i, o))
                .collect::<Vec<_>>().join("\n")
        );
        
        let response = self.llm.chat(ModelClass::Coding, &[
            Message::system("You are a skill generator for the Argus autonomous agent system."),
            Message::user(&prompt),
        ]).await?;
        
        let generated: GeneratedSkillDraft = serde_json::from_str(&response)?;
        
        // 2. Security audit the generated code
        let audit = self.security_auditor.audit_code(&generated.code).await?;
        if !audit.is_safe() {
            return Err(Error::GeneratedSkillUnsafe(audit.reasons));
        }
        
        // 3. Run test cases
        for test in &generated.test_cases {
            let result = self.run_skill_code(&generated.code, &test.input).await?;
            if result != test.expected_output {
                return Err(Error::GeneratedSkillTestFailed {
                    input: test.input.clone(),
                    expected: test.expected_output.clone(),
                    actual: result,
                });
            }
        }
        
        // 4. Save skill
        let skill_id = format!("generated_{}", Uuid::new_v4().simple());
        let skill_dir = PathBuf::from(format!("/var/argus/skills/{}", skill_id));
        tokio::fs::create_dir_all(&skill_dir).await?;
        tokio::fs::write(skill_dir.join("manifest.json"), serde_json::to_string_pretty(&generated.manifest)?).await?;
        tokio::fs::write(skill_dir.join("skill.py"), &generated.code).await?;
        
        // 5. Register skill
        let skill = ScriptSkill::load(&skill_dir).await?;
        self.skills_engine.register(skill_id.clone(), Box::new(skill));
        
        self.event_bus.emit(ArgusEvent::SkillGenerated(skill_id.clone())).await;
        
        Ok(GeneratedSkill {
            id: skill_id,
            manifest: generated.manifest,
        })
    }
}
```

## 6. Permission Model

```rust
struct PermissionChecker {
    /// Per-trust-level permission limits
    trust_policies: HashMap<TrustLevel, PermissionPolicy>,
}

struct PermissionPolicy {
    /// Maximum allowed permissions for this trust level
    allowed: Vec<SkillPermission>,
    
    /// Operations that require owner approval regardless
    requires_approval: Vec<String>,
    
    /// Completely blocked operations
    blocked: Vec<String>,
}

impl PermissionChecker {
    fn check(&self, manifest: &SkillManifest, context: &SkillContext) -> Result<()> {
        let policy = &self.trust_policies[&context.trust_level];
        
        for permission in &manifest.permissions {
            match permission {
                SkillPermission::Shell { allowed_commands } => {
                    // Check each command against blocklist
                    for cmd in allowed_commands {
                        if policy.blocked.iter().any(|b| cmd.contains(b)) {
                            return Err(Error::PermissionDenied(
                                format!("Command '{}' is blocked for trust level {:?}", cmd, context.trust_level)
                            ));
                        }
                    }
                }
                SkillPermission::FileSystem { paths, write } => {
                    // Ensure paths are within allowed scope
                    for path in paths {
                        if !context.workspace_dir.join(path).starts_with(&context.workspace_dir) {
                            return Err(Error::PermissionDenied(
                                format!("Path '{}' is outside workspace", path)
                            ));
                        }
                    }
                }
                SkillPermission::Network { hosts } => {
                    // Third-party and untrusted can't access arbitrary hosts
                    if context.trust_level == TrustLevel::Untrusted && !hosts.is_empty() {
                        return Err(Error::PermissionDenied(
                            "Network access blocked for untrusted skills".into()
                        ));
                    }
                }
                _ => {}
            }
        }
        
        Ok(())
    }
}
```

## 7. Configuration

> **Note**: Configuration values shown below are system-managed defaults. In production, all configuration is managed through Argus's conversational interface (see `CONVERSATIONAL_CONFIG.md`). Direct TOML file editing is not required.

```toml
# config/mcp.toml

[servers.filesystem]
type = "builtin"
enabled = true

[servers.shell]
type = "builtin"
enabled = true

[servers.database]
type = "builtin"
enabled = true

[servers.git]
type = "builtin"
enabled = true

[servers.playwright]
type = "stdio"
command = "npx"
args = ["-y", "@anthropic-ai/mcp-server-playwright"]
auto_start = false
trust_level = "verified"

# Third-party servers added via conversation
# [servers.custom_server]
# ...

[skills]
skills_dir = "/var/argus/skills"
self_generate_enabled = true
max_generated_skills = 50

[security]
audit_before_install = true
block_dangerous_operations = true
sandbox_untrusted = true
```

## 8. Conversational Management

```
User: "我需要一个能操作浏览器的工具"
Argus: "我找到以下MCP Server:
        1. @anthropic-ai/mcp-server-playwright (官方, 验证过)
           - 提供: navigate, click, type, screenshot, extract
        2. @anthropic-ai/mcp-server-puppeteer
           - 提供: similar functionality
        
        推荐安装 Playwright Server (更稳定)。要安装吗？"

User: "装"
Argus: "正在安全审计... ✓ 通过 (信任等级: Verified)
        正在安装依赖... ✓ 
        正在连接... ✓ 发现5个工具
        
        可用工具:
        - navigate: 导航到URL
        - click: 点击元素
        - type: 输入文本
        - screenshot: 截图
        - extract: 提取页面内容
        
        安装完成！你可以说'帮我打开xxx网站'来使用。"

User: "帮我写个能自动签到的skill"
Argus: "好的，我来生成一个自动签到Skill:
        
        分析中...
        - 需要浏览器操作 (Playwright MCP)
        - 需要定时触发
        - 需要存储登录凭证
        
        已生成 Skill: auto_checkin_v1
        - 权限: 浏览器、网络、定时器
        - 已通过安全审计
        - 测试: ✓ 通过
        
        使用方法: '每天早上9点自动签到xxx'"
```
