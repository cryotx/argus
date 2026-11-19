# Role-Based Collaboration System — Detailed Design

Complex software tasks benefit from multiple perspectives — architecture review, code review, testing, security audit. Argus implements a role-based collaboration system where the Brain assigns multiple Worker roles for complex tasks, each with specialized system prompts and responsibilities. Simple tasks bypass collaboration entirely.

## 1. Architecture

```
┌─────────────────────────────────────────────────────────────────────────┐
│                    Brain Engine                                          │
│                                                                          │
│  ┌────────────────┐    ┌────────────────┐    ┌────────────────────────┐ │
│  │ First-Principles│    │ Complexity     │    │ Collaboration          │ │
│  │ Decomposer     │───→│ Assessor       │───→│ Orchestrator           │ │
│  │                │    │                │    │                        │ │
│  └────────────────┘    └────────────────┘    └───────────┬────────────┘ │
│                                                          │              │
└──────────────────────────────────────────────────────────┼──────────────┘
                                                           │
                              ┌─────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                    Worker Manager                                        │
│                                                                          │
│  Simple tasks:                                                           │
│  ┌──────────┐                                                           │
│  │ Worker   │ ← Solo mode, any tool (OpenCode/ClaudeCode/Aider/CLI)    │
│  └──────────┘                                                           │
│                                                                          │
│  Complex tasks:                                                          │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐               │
│  │ Worker   │  │ Worker   │  │ Worker   │  │ Worker   │               │
│  │ Architect│  │ Developer│  │ Reviewer │  │ Tester   │               │
│  │(OpenCode)│  │(ClaudeC.)│  │(OpenCode)│  │(ClaudeC.)│               │
│  └──────────┘  └──────────┘  └──────────┘  └──────────┘               │
│                                                                          │
│  Only OpenCode + Claude Code for collaboration (structured output,       │
│  system prompt injection, session persistence required)                   │
└─────────────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                    .argus/collab/ (File System)                          │
│                                                                          │
│  ├── session-{id}/                                                      │
│  │   ├── architecture.md        ← Architect output                      │
│  │   ├── implementation.md      ← Developer summary                     │
│  │   ├── review-round-1.json    ← Reviewer findings                    │
│  │   ├── review-round-2.json    ← Reviewer re-review                   │
│  │   ├── test-report.json       ← Tester results                       │
│  │   ├── security-audit.json    ← SecurityAuditor findings             │
│  │   └── session.json           ← Session metadata                     │
└─────────────────────────────────────────────────────────────────────────┘
```

## 2. Core Concepts

### 2.1 Collaboration Modes

```rust
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
enum CollaborationMode {
    /// Single worker, no collaboration overhead.
    /// All tools supported: OpenCode, Claude Code, Aider, GenericCli.
    Solo,

    /// One worker writes, another reviews.
    /// Most common for medium-complexity tasks.
    /// Only OpenCode + Claude Code (require structured output + system prompt).
    PairReview,

    /// Full role team: Architect → Developer(s) → Reviewer → Tester → Security Auditor.
    /// Reserved for high-complexity, high-risk tasks.
    /// Only OpenCode + Claude Code.
    FullTeam,
}
```

### 2.2 Collaboration Roles

```rust
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
enum CollaborationRole {
    /// Designs overall architecture, defines interfaces, makes tech decisions.
    /// Output: Architecture document (data models, API contracts, module boundaries).
    Architect,

    /// Implements code based on architecture document and task requirements.
    /// Output: Working code + implementation summary.
    Developer,

    /// Reviews implementation for correctness, quality, maintainability.
    /// Output: Review report (issues, suggestions, verdict: approve/request_changes).
    Reviewer,

    /// Writes and runs tests, validates acceptance criteria.
    /// Output: Test report (pass/fail, coverage, edge cases found).
    Tester,

    /// Audits for security vulnerabilities, injection attacks, secret leaks.
    /// Output: Security audit report (findings, severity, recommendations).
    SecurityAuditor,

    /// Handles deployment configuration, CI/CD, infrastructure concerns.
    /// Output: Deployment plan or config changes.
    DevOps,

    /// Oversees the entire collaboration, resolves conflicts between roles.
    /// This role is always played by the Brain itself, never delegated to a worker.
    TechLead,
}
```

### 2.3 Tool Capability Tiers

Not all AI coding tools support the structured output and system prompt injection required for role-based collaboration:

```rust
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
enum WorkerCapability {
    /// Can only execute tasks in Solo mode.
    /// No structured JSON output, no system prompt control.
    SoloOnly,

    /// Supports role-based collaboration.
    /// Requirements: structured JSON output, system prompt injection, session persistence.
    CollaborationCapable,
}

impl WorkerType {
    fn capability(&self) -> WorkerCapability {
        match self {
            // OpenCode: `opencode run` + `opencode serve` HTTP API,
            //   JSON output (--format json), Agent system for role injection,
            //   session persistence.
            WorkerType::OpenCode => WorkerCapability::CollaborationCapable,

            // Claude Code: `claude -p` print mode, `--output-format json`,
            //   `--system-prompt` / `--append-system-prompt` for role injection,
            //   `--allowedTools` for per-role permission whitelisting,
            //   session persistence via `--session-id`.
            WorkerType::ClaudeCode => WorkerCapability::CollaborationCapable,

            // Aider: `--message` for single-shot, but NO structured JSON output,
            //   NO system prompt control. Solo only.
            WorkerType::Aider => WorkerCapability::SoloOnly,

            // GenericCli: Plain command execution. Solo only.
            WorkerType::GenericCli => WorkerCapability::SoloOnly,
        }
    }
}
```

## 3. Complexity Assessor

The Brain evaluates task complexity after decomposition to determine which collaboration mode to use.

### 3.1 Complexity Factors

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
enum ComplexityFactor {
    /// Task touches multiple modules or crates
    MultiModule { module_count: usize },

    /// Large number of files expected to change
    HighFileCount { estimated_files: usize },

    /// Involves security-sensitive operations (auth, crypto, permissions)
    SecuritySensitive { areas: Vec<String> },

    /// Requires deployment changes or infrastructure modifications
    DeploymentRelated,

    /// Database schema migration
    DatabaseMigration,

    /// Changes to public API surface (breaking changes possible)
    PublicApiChange,

    /// Involves concurrent/async patterns prone to race conditions
    ConcurrencyHazard,

    /// Introduces a new architectural pattern not used elsewhere in the project
    NewArchitecturalPattern { pattern: String },

    /// Crosses project boundaries (shared libraries, monorepo interdependencies)
    CrossProject { projects: Vec<String> },

    /// Involves financial transactions or billing logic
    FinancialLogic,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ComplexityAssessment {
    /// Overall complexity score (0.0 = trivial, 1.0 = extremely complex)
    score: f32,

    /// Individual factors that contributed to the score
    factors: Vec<ComplexityFactor>,

    /// Determined collaboration mode
    mode: CollaborationMode,

    /// Risk level for review policy selection
    risk_level: RiskLevel,

    /// Reasoning from the LLM
    reasoning: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
enum RiskLevel {
    /// UI tweaks, documentation, comments, formatting
    Low,
    /// Normal feature development, bug fixes
    Medium,
    /// Security changes, database migrations, public API changes
    High,
    /// Production deployment, self-evolution, financial logic
    Critical,
}
```

### 3.2 Scoring Algorithm

```rust
impl ComplexityAssessor {
    /// Assess task complexity. Called after the First-Principles Decomposer
    /// produces the task tree — each task is assessed individually.
    async fn assess(&self, task: &Task, project: &Project, memory: &MemoryEngine) -> ComplexityAssessment {
        // 1. Gather factors from task metadata
        let mut factors = Vec::new();
        let mut score: f32 = 0.0;

        // Static analysis of task description and context
        let static_factors = self.detect_static_factors(task, project);
        factors.extend(static_factors);

        // 2. LLM-assisted assessment for nuanced factors
        let llm_factors = self.llm_assess(task, project, memory).await;
        factors.extend(llm_factors);

        // 3. Calculate score from factors
        for factor in &factors {
            score += match factor {
                ComplexityFactor::MultiModule { module_count } => {
                    0.1 * (*module_count as f32).min(5.0)
                }
                ComplexityFactor::HighFileCount { estimated_files } => {
                    0.05 * (*estimated_files as f32).min(10.0)
                }
                ComplexityFactor::SecuritySensitive { .. } => 0.3,
                ComplexityFactor::DeploymentRelated => 0.2,
                ComplexityFactor::DatabaseMigration => 0.25,
                ComplexityFactor::PublicApiChange => 0.2,
                ComplexityFactor::ConcurrencyHazard => 0.3,
                ComplexityFactor::NewArchitecturalPattern { .. } => 0.2,
                ComplexityFactor::CrossProject { .. } => 0.15,
                ComplexityFactor::FinancialLogic => 0.35,
            };
        }

        score = score.clamp(0.0, 1.0);

        // 4. Map score to collaboration mode
        let mode = match score {
            s if s < 0.3 => CollaborationMode::Solo,
            s if s < 0.6 => CollaborationMode::PairReview,
            _ => CollaborationMode::FullTeam,
        };

        // 5. Determine risk level
        let risk_level = self.determine_risk_level(&factors, score);

        ComplexityAssessment {
            score,
            factors,
            mode,
            risk_level,
            reasoning: format!(
                "Complexity score {:.2}: {} factors detected. Mode: {:?}",
                score, factors.len(), mode
            ),
        }
    }

    fn determine_risk_level(&self, factors: &[ComplexityFactor], score: f32) -> RiskLevel {
        // Critical if financial logic or self-evolution related
        if factors.iter().any(|f| matches!(f, ComplexityFactor::FinancialLogic)) {
            return RiskLevel::Critical;
        }

        // High if security-sensitive or DB migration
        if factors.iter().any(|f| matches!(f,
            ComplexityFactor::SecuritySensitive { .. } |
            ComplexityFactor::DatabaseMigration |
            ComplexityFactor::PublicApiChange
        )) {
            return RiskLevel::High;
        }

        match score {
            s if s < 0.2 => RiskLevel::Low,
            s if s < 0.5 => RiskLevel::Medium,
            s if s < 0.7 => RiskLevel::High,
            _ => RiskLevel::Critical,
        }
    }
}
```

### 3.3 LLM Assessment Prompt

```
SYSTEM: You are the complexity assessor for the Argus autonomous agent system.
Analyze the following task and identify complexity factors.

## Task
Title: {task.title}
Description: {task.description}
Project: {project.name} ({project.language}/{project.framework})
Acceptance Criteria: {task.acceptance_criteria}
Target Files (if known): {task.context.files}

## Project Context
Module structure: {project_modules}
Recent similar tasks and their outcomes: {relevant_memories}

## Output
Return JSON:
{
    "factors": [
        { "type": "MultiModule", "module_count": 3 },
        { "type": "SecuritySensitive", "areas": ["authentication", "token handling"] }
    ],
    "reasoning": "This task modifies the auth module which handles JWT tokens..."
}

Only include factors that genuinely apply. Do not inflate complexity.
```

## 4. Review Policies

Review policy is determined by risk level and dictates who reviews code and how many rounds.

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
struct ReviewPolicy {
    /// Minimum number of review rounds
    min_review_rounds: u32,

    /// Maximum number of review rounds before escalating to owner
    max_review_rounds: u32,

    /// Whether the reviewer must use a different LLM model than the developer
    force_different_model: bool,

    /// Whether a security audit is required
    require_security_audit: bool,

    /// Whether owner approval is needed before merge
    require_owner_approval: bool,

    /// Roles required for this review policy
    required_roles: Vec<CollaborationRole>,
}

impl ReviewPolicy {
    fn for_risk_level(risk_level: &RiskLevel) -> Self {
        match risk_level {
            RiskLevel::Low => ReviewPolicy {
                min_review_rounds: 0,
                max_review_rounds: 0,
                force_different_model: false,
                require_security_audit: false,
                require_owner_approval: false,
                required_roles: vec![],  // Solo mode, no collaboration
            },
            RiskLevel::Medium => ReviewPolicy {
                min_review_rounds: 1,
                max_review_rounds: 2,
                force_different_model: false,  // Same model OK
                require_security_audit: false,
                require_owner_approval: false,
                required_roles: vec![
                    CollaborationRole::Developer,
                    CollaborationRole::Reviewer,
                ],
            },
            RiskLevel::High => ReviewPolicy {
                min_review_rounds: 2,
                max_review_rounds: 3,
                force_different_model: true,  // Must use different model
                require_security_audit: true,
                require_owner_approval: false,
                required_roles: vec![
                    CollaborationRole::Architect,
                    CollaborationRole::Developer,
                    CollaborationRole::Reviewer,
                    CollaborationRole::Tester,
                    CollaborationRole::SecurityAuditor,
                ],
            },
            RiskLevel::Critical => ReviewPolicy {
                min_review_rounds: 2,
                max_review_rounds: 3,
                force_different_model: true,
                require_security_audit: true,
                require_owner_approval: true,  // Owner must approve
                required_roles: vec![
                    CollaborationRole::Architect,
                    CollaborationRole::Developer,
                    CollaborationRole::Reviewer,
                    CollaborationRole::Tester,
                    CollaborationRole::SecurityAuditor,
                ],
            },
        }
    }
}
```

### 4.1 Review Policy Summary

| Risk Level | Mode | Model Requirement | Review Rounds | Security Audit | Owner Approval |
|------------|------|-------------------|---------------|----------------|----------------|
| Low | Solo | Any | 0 | No | No |
| Medium | PairReview | Same model OK | 1 | No | No |
| High | FullTeam | Force different model | 2 | Yes | No |
| Critical | FullTeam | Force different model | 2 | Yes | Yes |

### 4.2 Cross-Model Review Matrix

When `force_different_model` is true, the reviewer must use a different underlying LLM than the developer:

| Developer Tool + Model | Reviewer Tool + Model |
|------------------------|-----------------------|
| OpenCode (Claude Sonnet) | Claude Code (Claude Sonnet via different instance) or OpenCode (GPT-4o/DeepSeek) |
| Claude Code (Claude Sonnet) | OpenCode (GPT-4o/DeepSeek) |
| OpenCode (GPT-4o) | Claude Code (Claude Sonnet) or OpenCode (DeepSeek) |
| OpenCode (DeepSeek) | Claude Code (Claude Sonnet) or OpenCode (GPT-4o) |

The key constraint is the underlying LLM model, not the tool. Two OpenCode instances with different models satisfy the requirement.

## 5. Collaboration Session

### 5.1 Session Lifecycle

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
struct CollaborationSession {
    id: Uuid,
    task_id: Uuid,
    project_id: Uuid,

    /// Collaboration mode for this session
    mode: CollaborationMode,

    /// Review policy governing this session
    review_policy: ReviewPolicy,

    /// Complexity assessment that triggered collaboration
    complexity: ComplexityAssessment,

    /// Current phase of collaboration
    phase: CollaborationPhase,

    /// Role assignments: which worker handles which role
    role_assignments: HashMap<CollaborationRole, RoleAssignment>,

    /// Artifacts produced during collaboration
    artifacts: Vec<CollaborationArtifact>,

    /// Review rounds completed
    review_rounds_completed: u32,

    /// Retry counters for test and security fix loops
    test_retry_count: u32,
    security_retry_count: u32,

    /// File system path for collaboration artifacts
    workspace_path: PathBuf,

    /// Timestamps
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    completed_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct RoleAssignment {
    role: CollaborationRole,
    worker_id: Uuid,
    worker_type: WorkerType,
    model: String,           // Underlying LLM model used
    status: RoleStatus,
    started_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
enum RoleStatus {
    Pending,
    InProgress,
    Completed,
    Failed { error: String },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
enum CollaborationPhase {
    /// Architect designs the solution
    Architecture,
    /// Developer(s) implement the code
    Implementation,
    /// Reviewer examines the implementation
    Review { round: u32 },
    /// Tester validates with tests
    Testing,
    /// Security auditor checks for vulnerabilities
    SecurityAudit,
    /// All phases complete, ready to merge
    Merging,
    /// Session completed successfully
    Completed,
    /// Session failed
    Failed { reason: String },
}
```

### 5.2 Collaboration Execution Flow

```
Brain: Task T42 assessed as High complexity (score 0.72)
Brain: Selecting FullTeam mode with force_different_model review
     │
     ▼
Phase 1: ARCHITECTURE
     │
     │  Brain assigns Architect role → Worker-1 (OpenCode, Claude Sonnet)
     │  System prompt: "You are the Architect. Design the solution..."
     │  Input: Task description + project context + relevant memories
     │  Output: architecture.md → .argus/collab/session-{id}/
     │
     ▼
Phase 2: IMPLEMENTATION
     │
     │  Brain assigns Developer role → Worker-2 (Claude Code, Claude Sonnet)
     │  System prompt: "You are the Developer. Implement based on architecture..."
     │  Input: Task description + architecture.md
     │  Output: Code changes + implementation.md
     │
     ▼
Phase 3: REVIEW (Round 1)
     │
     │  Brain assigns Reviewer role → Worker-3 (OpenCode, GPT-4o)
     │  ↑ Different model than Developer (force_different_model = true)
     │  System prompt: "You are the Code Reviewer. Review the implementation..."
     │  Input: architecture.md + implementation.md + git diff
     │  Output: review-round-1.json
     │
     │  Verdict: request_changes?
     │  ├── Yes → Developer addresses feedback → Review Round 2
     │  └── No (approved) → Continue to Testing
     │
     ▼
Phase 4: TESTING
     │
     │  Brain assigns Tester role → Worker-4 (Claude Code)
     │  System prompt: "You are the Tester. Write and run tests..."
     │  Input: implementation.md + code changes + acceptance criteria
     │  Output: test-report.json
     │
     │  All tests pass?
     │  ├── No → Developer fixes → Re-test
     │  └── Yes → Continue to Security Audit
     │
     ▼
Phase 5: SECURITY AUDIT (if required by review policy)
     │
     │  Brain assigns SecurityAuditor role → Worker-5 (OpenCode, DeepSeek)
     │  System prompt: "You are the Security Auditor. Audit for vulnerabilities..."
     │  Input: git diff + architecture.md + security context
     │  Output: security-audit.json
     │
     │  Findings severity?
     │  ├── Critical/High → Developer fixes → Re-audit
     │  └── None/Low/Medium → Continue to Merge
     │
     ▼
Phase 6: MERGE
     │
     │  Brain: All phases passed
     │  ├── Owner approval required? (Critical risk)
     │  │   ├── Yes → Notify owner, wait for approval
     │  │   └── No → Auto-merge
     │  │
     │  Git commit + push + PR (if applicable)
     │  Store experience in memory
     │  Cleanup collaboration artifacts
     │
     ▼
COMPLETED
```

### 5.3 PairReview Simplified Flow

For medium-complexity tasks, the flow is much simpler:

```
Brain: Task T43 assessed as Medium complexity (score 0.42)
Brain: Selecting PairReview mode
     │
     ▼
Phase 1: IMPLEMENTATION (no separate Architecture phase)
     │
     │  Worker-1 (OpenCode, Claude Sonnet): Developer
     │  Output: Code changes + implementation summary
     │
     ▼
Phase 2: REVIEW (1 round)
     │
     │  Worker-2 (Claude Code, Claude Sonnet): Reviewer
     │  Same model OK (force_different_model = false)
     │  Output: Review report
     │
     │  Approved? → Merge
     │  Request changes? → Developer fixes → Auto-approve (max 1 round)
     │
     ▼
COMPLETED
```

## 6. Role System Prompts

Each role gets a specialized system prompt injected via the tool's system prompt mechanism.

### 6.1 Architect Prompt

```
You are the ARCHITECT for this task. Your job is to design the solution before any code is written.

## Your Responsibilities
1. Analyze the requirements and define the technical approach
2. Identify which modules/files need to be created or modified
3. Define data models, API contracts, and interface boundaries
4. Make technology decisions with justification
5. Identify potential risks and edge cases

## Output Format
Write your architecture document in Markdown to the file: {artifact_path}/architecture.md

The document must include:
- Overview of the approach
- Data models (if applicable)
- API/interface contracts (if applicable)
- Module boundaries and responsibilities
- File list with descriptions of changes
- Risk assessment

## Constraints
- Do NOT write implementation code — only design
- Keep the design pragmatic, not over-engineered
- Consider the existing codebase patterns: {project_patterns}
```

### 6.2 Developer Prompt

```
You are the DEVELOPER for this task. Implement the solution based on the architecture document.

## Architecture Document
{architecture_md_content}

## Your Responsibilities
1. Implement the code changes as specified in the architecture document
2. Follow existing code patterns and conventions
3. Write clear, maintainable code
4. Add inline comments for complex logic
5. After implementation, write a summary to: {artifact_path}/implementation.md

## Implementation Summary Format
Your implementation.md must include:
- Files created/modified (with brief description of changes)
- Any deviations from the architecture document (with justification)
- Known limitations or TODO items
- Suggested test cases

## Constraints
- Follow the architecture document closely
- If you disagree with the architecture, note it in your implementation summary but still implement as designed
- Do NOT modify files outside the scope defined by the architecture
```

### 6.3 Reviewer Prompt

```
You are the CODE REVIEWER for this task. Review the implementation for correctness, quality, and adherence to the architecture.

## Architecture Document
{architecture_md_content}

## Implementation Summary
{implementation_md_content}

## Code Changes
{git_diff}

## Your Responsibilities
1. Verify the implementation matches the architecture document
2. Check for bugs, logic errors, and edge cases
3. Assess code quality, readability, and maintainability
4. Check for performance issues
5. Verify error handling is adequate

## Output Format
Write your review to: {artifact_path}/review-round-{round}.json

{
    "verdict": "approve" | "request_changes",
    "overall_assessment": "...",
    "issues": [
        {
            "severity": "critical" | "major" | "minor" | "suggestion",
            "file": "path/to/file.rs",
            "line": 42,
            "description": "...",
            "suggestion": "..."
        }
    ],
    "positive_notes": ["..."],
    "estimated_effort_to_fix": "low" | "medium" | "high"
}

## Review Standards
- "approve": No critical or major issues. Minor issues can be noted but don't block.
- "request_changes": At least one critical or major issue that must be fixed.
- Be constructive. Explain WHY something is a problem, not just WHAT.
- Do NOT nitpick style issues unless they harm readability.
```

### 6.4 Tester Prompt

```
You are the TESTER for this task. Write and run tests to validate the implementation.

## Acceptance Criteria
{acceptance_criteria}

## Implementation Summary
{implementation_md_content}

## Your Responsibilities
1. Write unit tests for new/modified code
2. Write integration tests if the task involves multiple modules
3. Run all existing tests to check for regressions
4. Validate each acceptance criterion
5. Test edge cases and error paths

## Output Format
Write your test report to: {artifact_path}/test-report.json

{
    "verdict": "pass" | "fail",
    "tests_written": 12,
    "tests_passed": 11,
    "tests_failed": 1,
    "test_coverage_delta": "+3.2%",
    "acceptance_criteria_results": [
        { "criterion": "...", "status": "pass" | "fail", "notes": "..." }
    ],
    "edge_cases_tested": ["..."],
    "regression_test_results": {
        "total": 150,
        "passed": 150,
        "failed": 0
    },
    "issues_found": [
        { "description": "...", "severity": "critical" | "major" | "minor" }
    ]
}

## Test Commands
{test_commands}
```

### 6.5 Security Auditor Prompt

```
You are the SECURITY AUDITOR for this task. Audit the code changes for security vulnerabilities.

## Code Changes
{git_diff}

## Architecture Document
{architecture_md_content}

## Your Responsibilities
1. Check for common vulnerabilities: injection (SQL, XSS, command), auth bypass, CSRF, SSRF
2. Check for hardcoded secrets, API keys, credentials
3. Check for insecure cryptographic practices
4. Check for privilege escalation paths
5. Check for data exposure and privacy issues
6. Check for race conditions with security implications
7. Validate input sanitization and output encoding

## Output Format
Write your audit to: {artifact_path}/security-audit.json

{
    "verdict": "pass" | "warn" | "fail",
    "findings": [
        {
            "severity": "critical" | "high" | "medium" | "low" | "info",
            "category": "injection" | "auth" | "crypto" | "secrets" | "privacy" | "other",
            "file": "path/to/file.rs",
            "line": 42,
            "description": "...",
            "recommendation": "...",
            "cwe": "CWE-89"
        }
    ],
    "overall_assessment": "...",
    "recommendations": ["..."]
}

## Severity Guidelines
- critical: Exploitable vulnerability that could lead to data breach or system compromise
- high: Significant vulnerability that should be fixed before merge
- medium: Potential vulnerability, should be tracked for fix
- low: Minor security improvement, non-blocking
- info: Security best practice suggestion
```

## 7. Role Prompt Injection per Tool

### 7.1 OpenCode

OpenCode supports role injection via its Agent system and `opencode serve` HTTP API:

```rust
impl OpenCodeDriver {
    async fn spawn_with_role(
        &self,
        config: &WorkerConfig,
        role: &CollaborationRole,
        role_prompt: &str,
    ) -> Result<WorkerHandle> {
        // Option A: Use opencode serve mode (HTTP API)
        // Start opencode as HTTP server and send prompts via API
        let child = tokio::process::Command::new(&self.binary_path)
            .arg("serve")
            .arg("--port")
            .arg(&config.port.to_string())
            .current_dir(&config.workspace_dir)
            .spawn()?;

        // Send task with role prompt via HTTP
        let client = reqwest::Client::new();
        let response = client.post(format!("http://localhost:{}/api/message", config.port))
            .json(&serde_json::json!({
                "content": format!("{}\n\n---\n\n{}", role_prompt, config.task_description),
                "format": "json"
            }))
            .send()
            .await?;

        // Option B: Use opencode run with Agent configuration
        // Configure an Agent in the project's opencode config that has the role prompt
        // as its system prompt, then invoke with `opencode run`
        let child = tokio::process::Command::new(&self.binary_path)
            .arg("run")
            .arg("--agent")
            .arg(role.agent_name())  // e.g., "argus-architect"
            .arg("--format")
            .arg("json")
            .current_dir(&config.workspace_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;

        Ok(WorkerHandle { /* ... */ })
    }
}
```

### 7.2 Claude Code

Claude Code has first-class support for system prompts and structured output:

```rust
impl ClaudeCodeDriver {
    async fn spawn_with_role(
        &self,
        config: &WorkerConfig,
        role: &CollaborationRole,
        role_prompt: &str,
    ) -> Result<WorkerHandle> {
        let mut cmd = tokio::process::Command::new(&self.binary_path);

        cmd.arg("-p")  // Print mode (non-interactive)
            .arg("--output-format")
            .arg("json")  // Structured JSON output
            .arg("--system-prompt")
            .arg(role_prompt)  // Role-specific system prompt
            .arg("--dangerously-skip-permissions");  // Auto-approve

        // For reviewer role, restrict tools to read-only operations
        if *role == CollaborationRole::Reviewer || *role == CollaborationRole::SecurityAuditor {
            cmd.arg("--allowedTools")
                .arg("Read,Grep,Glob,Bash(read-only)");
        }

        // Session persistence for multi-turn collaboration
        if let Some(session_id) = &config.session_id {
            cmd.arg("--session-id")
                .arg(session_id);
        }

        let child = cmd
            .current_dir(&config.workspace_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        Ok(WorkerHandle { /* ... */ })
    }
}
```

## 8. Inter-Role Communication

Roles communicate through the file system, not directly between processes. The Brain (TechLead) coordinates the flow.

### 8.1 Artifact Types

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
struct CollaborationArtifact {
    id: Uuid,
    session_id: Uuid,
    role: CollaborationRole,
    artifact_type: ArtifactType,
    /// Relative path within the session workspace
    file_path: String,
    /// Size in bytes
    size_bytes: u64,
    created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
enum ArtifactType {
    /// Architecture design document (Markdown)
    ArchitectureDocument,
    /// Implementation summary (Markdown)
    ImplementationSummary,
    /// Code review report (JSON)
    ReviewReport { round: u32 },
    /// Test execution report (JSON)
    TestReport,
    /// Security audit report (JSON)
    SecurityAuditReport,
    /// Deployment plan (Markdown)
    DeploymentPlan,
    /// Session metadata (JSON)
    SessionMetadata,
}
```

### 8.2 Workspace Layout

```
/var/argus/workspaces/{project}/
├── src/                    ← Project source code
├── ...
└── .argus/
    └── collab/
        └── session-{uuid}/
            ├── session.json           ← Session metadata and state
            ├── architecture.md        ← Architect output
            ├── implementation.md      ← Developer summary
            ├── review-round-1.json    ← First review
            ├── review-round-2.json    ← Second review (if needed)
            ├── test-report.json       ← Tester output
            ├── security-audit.json    ← Security auditor output
            └── final-summary.md       ← Brain's final summary
```

### 8.3 Brain Coordination

The Brain acts as TechLead, reading artifacts and deciding the next step:

```rust
impl CollaborationOrchestrator {
    async fn run_session(&self, session: &mut CollaborationSession) -> Result<()> {
        loop {
            match &session.phase {
                CollaborationPhase::Architecture => {
                    self.run_architect(session).await?;
                    session.phase = CollaborationPhase::Implementation;
                }
                CollaborationPhase::Implementation => {
                    self.run_developer(session).await?;
                    session.phase = CollaborationPhase::Review { round: 1 };
                }
                CollaborationPhase::Review { round } => {
                    let review = self.run_reviewer(session, *round).await?;

                    if review.verdict == "approve" || *round >= session.review_policy.max_review_rounds {
                        if session.review_policy.require_security_audit {
                            session.phase = CollaborationPhase::SecurityAudit;
                        } else {
                            session.phase = CollaborationPhase::Testing;
                        }
                    } else {
                        // Developer addresses review feedback
                        self.run_developer_fixes(session, &review).await?;
                        session.phase = CollaborationPhase::Review { round: round + 1 };
                        session.review_rounds_completed += 1;
                    }
                }
                CollaborationPhase::Testing => {
                    let test_report = self.run_tester(session).await?;

                    if test_report.verdict == "pass" {
                        session.phase = CollaborationPhase::Merging;
                    } else {
                        // Developer fixes test failures, then re-test
                        self.run_developer_fixes(session, &test_report).await?;
                        // Re-run tests (but don't loop indefinitely)
                        if session.test_retry_count < 3 {
                            session.test_retry_count += 1;
                        } else {
                            session.phase = CollaborationPhase::Failed {
                                reason: "Tests failed after 3 fix attempts".into()
                            };
                        }
                    }
                }
                CollaborationPhase::SecurityAudit => {
                    let audit = self.run_security_auditor(session).await?;

                    if audit.verdict != "fail" {
                        session.phase = CollaborationPhase::Testing;
                    } else {
                        // Developer fixes security issues
                        self.run_developer_fixes(session, &audit).await?;
                        // Re-audit (limited retries)
                        if session.security_retry_count < 2 {
                            session.security_retry_count += 1;
                        } else {
                            session.phase = CollaborationPhase::Failed {
                                reason: "Security audit failed after fixes".into()
                            };
                        }
                    }
                }
                CollaborationPhase::Merging => {
                    if session.review_policy.require_owner_approval {
                        // Notify owner and wait
                        self.request_owner_approval(session).await?;
                    }
                    self.merge(session).await?;
                    session.phase = CollaborationPhase::Completed;
                }
                CollaborationPhase::Completed | CollaborationPhase::Failed { .. } => {
                    break;
                }
            }

            // Persist session state after each phase transition
            self.save_session(session).await?;
            self.event_bus.publish(
                EventSource::Brain,
                ArgusEvent::CollaborationPhaseChanged {
                    session_id: session.id,
                    task_id: session.task_id,
                    phase: session.phase.clone(),
                },
            );
        }

        Ok(())
    }
}
```

## 9. Cost Control

Collaboration multiplies Worker invocations. Cost is controlled by tiering:

### 9.1 Expected Distribution

| Mode | % of Tasks | Workers per Task | Cost Multiplier |
|------|-----------|-----------------|-----------------|
| Solo | ~70% | 1 | 1x |
| PairReview | ~25% | 2 | 2x |
| FullTeam | ~5% | 4-7 | 4-7x |

### 9.2 Cost Optimization Strategies

1. **Model tiering within collaboration**: Architect and SecurityAuditor use powerful models (Claude Sonnet, GPT-4o). Reviewer for low-severity issues can use cheaper models (DeepSeek).
2. **Context compression**: Each role receives only the artifacts it needs, not the entire project context.
3. **Early exit**: If the Architect determines a task is simpler than initially assessed, downgrade to PairReview or Solo mid-session.
4. **Caching**: If a review round produces "approve" with no issues, skip subsequent rounds even if `max_review_rounds` allows more.
5. **Parallel execution**: When no dependency exists between roles (e.g., Tester and SecurityAuditor can run in parallel after Review is approved).

### 9.3 Budget Guard

```rust
impl CollaborationOrchestrator {
    /// Check if we have budget for the planned collaboration
    async fn check_budget(&self, session: &CollaborationSession) -> Result<()> {
        let estimated_cost = match session.mode {
            CollaborationMode::Solo => 0.0,  // No extra cost
            CollaborationMode::PairReview => {
                // ~2x normal task cost
                self.estimate_task_cost(&session.task_id).await? * 2.0
            }
            CollaborationMode::FullTeam => {
                // ~5x normal task cost (architect + dev + reviewer + tester + security)
                self.estimate_task_cost(&session.task_id).await? * 5.0
            }
        };

        let remaining_budget = self.budget_tracker.remaining_budget().await?;

        if estimated_cost > remaining_budget * 0.2 {
            // This session would consume >20% of remaining budget — warn owner
            self.notify_owner(format!(
                "协作任务预估消耗 ${:.2}，占剩余预算的 {:.0}%。是否继续？",
                estimated_cost,
                (estimated_cost / remaining_budget) * 100.0,
            )).await?;
        }

        Ok(())
    }
}
```

## 10. Configuration

```toml
# config/collaboration.toml

[collaboration]
# Enable/disable the collaboration system
enabled = true

# Complexity score thresholds for mode selection
solo_threshold = 0.3          # Below this → Solo
pair_review_threshold = 0.6   # Below this → PairReview, above → FullTeam

# Maximum concurrent collaboration sessions
max_concurrent_sessions = 3

# Artifact retention (days) — cleanup old session artifacts
artifact_retention_days = 30

[collaboration.review]
# Default max review rounds (can be overridden by risk level)
default_max_review_rounds = 3

# Force different model for review (default, overridden by risk level)
default_force_different_model = false

# Timeout per role execution (minutes)
role_timeout_minutes = 30

[collaboration.cost]
# Warn owner if collaboration cost exceeds this % of remaining budget
budget_warning_percent = 20

# Maximum cost per collaboration session (USD) — abort if exceeded
max_session_cost_usd = 10.0

[collaboration.roles]
# Which roles are enabled (all by default)
enabled_roles = [
    "Architect",
    "Developer",
    "Reviewer",
    "Tester",
    "SecurityAuditor",
]

# DevOps role is optional, only used for deploy-related tasks
devops_enabled = false
```

## 11. Conversational Management

```
User: "这个任务用什么模式执行的？"
Argus: "任务 T42 (用户认证系统) 的协作评估:
        复杂度评分: 0.72 (高)
        因素: SecuritySensitive(认证/JWT), DatabaseMigration, PublicApiChange
        模式: FullTeam (完整团队协作)
        
        当前进度:
        ✅ Phase 1: 架构设计 (Worker-1, OpenCode/Claude Sonnet)
        ✅ Phase 2: 代码实现 (Worker-2, Claude Code/Claude Sonnet)
        🔄 Phase 3: 代码审查 Round 1 (Worker-3, OpenCode/GPT-4o)
        ⬜ Phase 4: 测试验证
        ⬜ Phase 5: 安全审计
        ⬜ Phase 6: 合并"

User: "以后简单的bug修复不要走协作流程"
Argus: "已更新: 简单bug修复将直接使用 Solo 模式。
        当前协作阈值:
        ├── Solo: 复杂度 < 0.3
        ├── PairReview: 0.3 ≤ 复杂度 < 0.6
        └── FullTeam: 复杂度 ≥ 0.6
        
        bug修复类任务的默认复杂度上限已设为0.2，除非涉及安全或数据库变更。"

User: "协作执行了多少次了？花了多少钱？"
Argus: "协作系统统计 (过去30天):
        
        模式分布:
        ├── Solo: 89次 (71.2%)
        ├── PairReview: 31次 (24.8%)
        └── FullTeam: 5次 (4.0%)
        
        协作成本:
        ├── PairReview 总计: $12.50 (平均 $0.40/次)
        ├── FullTeam 总计: $18.75 (平均 $3.75/次)
        └── 协作总额外成本: $31.25
        
        效果:
        ├── 协作审查发现问题: 47个 (其中8个为严重问题)
        ├── 安全审计发现漏洞: 3个 (全部在合并前修复)
        └── 代码审查一次通过率: 68%"

User: "关闭协作系统"
Argus: "已禁用协作系统。所有任务将使用 Solo 模式执行。
        注意: 这意味着不再有自动代码审查和安全审计。
        如需重新开启，告诉我即可。"
```

## 12. Integration Points

### 12.1 Brain Engine Integration

The Complexity Assessor is called after the First-Principles Decomposer produces the task tree:

```
Decomposer output (task tree)
    │
    ▼
Complexity Assessor (per task)
    │
    ├── score < 0.3 → Solo → directly to Worker Manager
    │
    └── score ≥ 0.3 → Collaboration Orchestrator
                            │
                            ▼
                      Worker Manager (multiple workers with roles)
```

### 12.2 Worker Manager Integration

The Worker Manager gains awareness of collaboration capability:

- When the Collaboration Orchestrator requests a worker for a role, the Worker Selector **only considers** `CollaborationCapable` tools (OpenCode, Claude Code).
- When `force_different_model` is required, the Worker Selector ensures the reviewer's underlying model differs from the developer's.
- Workers spawned for collaboration roles receive role-specific system prompts via their driver's prompt injection mechanism.

### 12.3 Event Bus Integration

New events added to `ArgusEvent`:

```rust
// Collaboration events
CollaborationStarted {
    session_id: Uuid,
    task_id: Uuid,
    mode: String,           // Solo, PairReview, FullTeam
    risk_level: String,
    roles: Vec<String>,
},
CollaborationPhaseChanged {
    session_id: Uuid,
    task_id: Uuid,
    phase: String,
},
CollaborationRoleAssigned {
    session_id: Uuid,
    role: String,
    worker_id: Uuid,
    worker_type: String,
    model: String,
},
CollaborationRoleCompleted {
    session_id: Uuid,
    role: String,
    worker_id: Uuid,
    duration_seconds: i64,
},
CollaborationReviewSubmitted {
    session_id: Uuid,
    round: u32,
    verdict: String,
    issues_count: u32,
},
CollaborationCompleted {
    session_id: Uuid,
    task_id: Uuid,
    mode: String,
    total_duration_seconds: i64,
    total_roles_executed: u32,
    review_rounds: u32,
},
CollaborationFailed {
    session_id: Uuid,
    task_id: Uuid,
    reason: String,
    phase: String,
},
```

### 12.4 Database Integration

Three new tables: `collaboration_sessions`, `collaboration_artifacts`, `collaboration_reviews`. See `DATABASE_SCHEMA.md` for full schema.
