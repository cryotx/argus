# Brain Engine — Detailed Design

The Brain Engine is the central intelligence of Argus. It is responsible for understanding user intent, decomposing goals into executable plans, making autonomous decisions, and continuously improving its strategies.

## 1. Components

```
┌─────────────────────────────────────────────────────────────────┐
│                         Brain Engine                              │
│                                                                    │
│  ┌──────────────┐  ┌──────────────┐  ┌───────────────┐           │
│  │ Intent       │  │ First-       │  │ Decision      │           │
│  │ Classifier   │  │ Principles   │  │ Engine        │           │
│  │              │  │ Decomposer   │  │               │           │
│  └──────┬───────┘  └──────┬───────┘  └───────┬───────┘           │
│         │                 │                   │                   │
│         │          ┌──────┴───────┐           │                   │
│         │          │ Complexity   │           │                   │
│         │          │ Assessor     │           │                   │
│         │          └──────┬───────┘           │                   │
│         │                 │                   │                   │
│  ┌──────┴─────────────────┴───────────────────┴───────┐          │
│  │                Prompt Templates                     │          │
│  └────────────────────────┬────────────────────────────┘          │
│                           │                                       │
│  ┌────────────────────────┴────────────────────────────┐          │
│  │                Context Builder                      │          │
│  │  (assembles memory + task info into optimal prompt)  │          │
│  └─────────────────────────────────────────────────────┘          │
└─────────────────────────────────────────────────────────────────┘
```

## 2. Intent Classifier

The first stage of processing any incoming message. Uses a cheap, fast model to quickly categorize intent.

### 2.1 Classification Categories

```rust
enum Intent {
    // === Project Management ===
    CreateProject { description: String },
    ModifyProject { project_hint: String, changes: String },
    PauseProject { project_hint: String },
    ResumeProject { project_hint: String },
    DeleteProject { project_hint: String },
    ProjectStatus { project_hint: Option<String> },
    ListProjects,
    SetProjectPriority { project_hint: String, priority: Priority },

    // === Feature / Task ===
    SuggestFeature { project_hint: String, feature: String },
    PrioritizeTask { task_hint: String, priority: Priority },
    CancelTask { task_hint: String },
    RetryTask { task_hint: String },

    // === Configuration ===
    ChangeConfig { domain: String, natural_description: String },

    // === Query ===
    QueryStatus { scope: QueryScope },
    QueryLogs { project_hint: Option<String>, filter: Option<String> },
    QueryBudget,
    QueryWorkers,
    QueryMemory { query: String },

    // === Direct Operations ===
    RunCommand { target: String, command: String },
    GitOperation { project_hint: String, operation: String },
    DeployNow { project_hint: String, environment: String },
    Rollback { project_hint: String },

    // === Memory ===
    RememberPermanent { content: String },
    Forget { content: String },
    Recall { query: String },

    // === Tools ===
    InstallMcp { description: String },
    InstallSkill { description: String },
    RemoveTool { name: String },
    ListTools,

    // === Social / Platform ===
    ConnectPlatform { platform: String, config: String },
    DisconnectPlatform { platform: String },

    // === Environment ===
    SetupServer { description: String },
    DiagnoseServer,

    // === Self-Evolution ===
    SelfImprove { description: String },

    // === Multi-Project ===
    CrossProjectAction { description: String },

    // === Conversational ===
    Greeting,
    Help,
    Feedback { content: String },
    Clarification { context: String },
    Approval { action_id: String },
    Rejection { action_id: String, reason: Option<String> },

    // === Unknown ===
    Unclear { original: String },
}
```

### 2.2 Classification Prompt

```
SYSTEM: You are an intent classifier for the Argus autonomous agent system.
Classify the user's message into exactly one intent category.
Return JSON: { "intent": "...", "params": { ... }, "confidence": 0.0-1.0 }

Available intents: [full list above]

Current context:
- Active projects: {project_list}
- Pending approvals: {pending_list}
- Recent conversation summary: {summary}

USER: {message}
```

### 2.3 Model Selection for Classification

- **Primary**: Fast model (gpt-4o-mini, qwen-turbo, glm-4-flash) — ~$0.0001 per classification
- **Fallback**: Local model (qwen2.5:7b via Ollama) — free

## 3. First-Principles Decomposer

### 3.1 Decomposition Pipeline

```
Input: "做一个电商小程序"
         │
         ▼
┌────────────────────────────────────────────┐
│ Phase 1: Goal Clarification                │
│                                            │
│ Questions the LLM asks itself:             │
│ • What is the essential purpose?           │
│   → Enable buying and selling goods        │
│ • Who is the target user?                  │
│   → General consumers, mobile-first        │
│ • What is the platform constraint?         │
│   → WeChat Mini Program ecosystem          │
│ • What is the minimum viable product?      │
│   → Browse products + Cart + Checkout      │
│                                            │
│ Output: Refined goal statement + constraints│
└────────────────────┬───────────────────────┘
                     ▼
┌────────────────────────────────────────────┐
│ Phase 2: Technical Decision                │
│                                            │
│ Decisions to make:                         │
│ • Tech stack selection                     │
│   → Taro/uni-app for cross-platform?      │
│   → Or native WeChat framework?           │
│ • Backend technology                       │
│   → Node.js + Express/Koa/Nest?           │
│   → Or Go/Python/Rust?                    │
│ • Database                                 │
│   → PostgreSQL for relational data        │
│   → Redis for sessions and caching        │
│ • Third-party integrations                 │
│   → WeChat Pay SDK                        │
│   → Image hosting (OSS/S3)               │
│                                            │
│ Output: Technical architecture document    │
└────────────────────┬───────────────────────┘
                     ▼
┌────────────────────────────────────────────┐
│ Phase 3: Milestone Decomposition           │
│                                            │
│ M1: Project Setup                          │
│   - Initialize repo and project structure  │
│   - Configure build tools                  │
│   - Set up database schema                 │
│   - Basic project skeleton                 │
│                                            │
│ M2: Core Features                          │
│   - Product catalog (list + detail)        │
│   - Shopping cart                          │
│   - User authentication                   │
│                                            │
│ M3: Transaction                            │
│   - Order management                      │
│   - Payment integration                   │
│   - Order status tracking                 │
│                                            │
│ M4: Polish                                 │
│   - Search and filtering                  │
│   - User profile and order history        │
│   - Error handling and edge cases         │
│                                            │
│ M5: Launch                                 │
│   - Integration testing                   │
│   - Performance optimization              │
│   - Deployment and go-live                │
│                                            │
│ Output: Ordered milestones with descriptions│
└────────────────────┬───────────────────────┘
                     ▼
┌────────────────────────────────────────────┐
│ Phase 4: Task Atomization                  │
│                                            │
│ Each milestone → N atomic tasks            │
│                                            │
│ Example for M1:                            │
│ T1.1: Initialize Git repository            │
│   - acceptance: repo exists, .gitignore    │
│   - depends_on: none                       │
│   - worker_type: opencode                  │
│   - estimated: 5 minutes                   │
│                                            │
│ T1.2: Create Taro project with TS template │
│   - acceptance: project builds, dev server │
│   - depends_on: T1.1                       │
│   - worker_type: opencode                  │
│   - estimated: 10 minutes                  │
│                                            │
│ T1.3: Design and create database schema    │
│   - acceptance: migration runs, tables ok  │
│   - depends_on: T1.1                       │
│   - worker_type: opencode                  │
│   - estimated: 20 minutes                  │
│                                            │
│ Output: Full task tree with dependencies   │
└────────────────────────────────────────────┘
```

### 3.2 Decomposition Prompt Template

```
SYSTEM: You are the brain of Argus, an autonomous development agent.
Use first-principles thinking to decompose the following project goal.

## Method
1. What is the fundamental purpose of this project? (Strip away all assumptions)
2. What are the absolute minimum components needed?
3. What is the simplest implementation of each component?
4. What are the dependencies between components?
5. What is the optimal build order?

## Constraints
- Target platform: {platform}
- Owner preferences: {preferences_from_permanent_memory}
- Past experience with similar projects: {relevant_memories}

## Output Format
Return a JSON structure:
{
  "refined_goal": "...",
  "constraints": ["..."],
  "tech_decisions": {
    "frontend": "...",
    "backend": "...",
    "database": "...",
    "third_party": ["..."]
  },
  "milestones": [
    {
      "id": "M1",
      "name": "...",
      "description": "...",
      "tasks": [
        {
          "id": "T1.1",
          "title": "...",
          "description": "...",
          "acceptance_criteria": ["..."],
          "depends_on": [],
          "worker_type": "opencode",
          "test_commands": ["..."],
          "estimated_minutes": 10
        }
      ]
    }
  ]
}

## Project Goal
{goal}
```

### 3.3 Iterative Refinement

After initial decomposition, the Brain checks:
1. Are all tasks truly atomic? (Can be completed in a single worker session)
2. Are dependencies correct? (No circular dependencies)
3. Are acceptance criteria testable? (Has concrete commands to verify)
4. Is the estimate reasonable? (Based on historical data from memory)

If any check fails, the Brain re-decomposes the problematic area.

### 3.4 Complexity Assessment

After decomposition, each task is assessed for complexity to determine the appropriate collaboration mode. The Complexity Assessor sits between the Decomposer output and the Collaboration Orchestrator.

```
Decomposer output (task tree)
    │
    ▼
Complexity Assessor (per task)
    │
    ├── score < 0.3 → Solo → directly to Worker Manager (single worker)
    │
    ├── 0.3 ≤ score < 0.6 → PairReview → one writes + one reviews
    │
    └── score ≥ 0.6 → FullTeam → Architect → Developer(s) → Reviewer → Tester → Security Auditor
```

**Key types:**

- `ComplexityFactor` — individual risk/complexity signals (e.g., `MultiModule`, `SecuritySensitive`, `DatabaseMigration`, `ConcurrencyHazard`, `FinancialLogic`)
- `ComplexityAssessment` — aggregated result containing `score: f32` (0.0–1.0), detected `factors`, determined `CollaborationMode`, and `RiskLevel`
- `RiskLevel` — `Low | Medium | High | Critical` — drives the `ReviewPolicy` (review rounds, cross-model enforcement, security audit, owner approval)

The assessor combines static analysis of task metadata with an LLM-assisted evaluation prompt that considers project context and historical outcomes from memory.

> See [COLLABORATION.md](./COLLABORATION.md) Section 3 for full `ComplexityFactor` enum, `ComplexityAssessment` struct, scoring algorithm, risk level determination, and the LLM assessment prompt template.

## 4. Decision Engine

### 4.1 Decision Levels

```rust
enum DecisionLevel {
    /// Execute immediately, report after
    Auto,

    /// Notify owner, execute after delay if no objection
    /// Default delay: 10 minutes (configurable)
    TimedAuto { delay: Duration },

    /// Wait for explicit owner approval
    WaitForApproval,
}

/// Rules for decision level assignment
fn decision_level(action: &Action, owner: &OwnerProfile) -> DecisionLevel {
    // Owner's global preference takes precedence
    match owner.interaction.approval_mode {
        ApprovalMode::FullAuto => return Auto,
        ApprovalMode::ManualAll => return WaitForApproval,
        _ => {} // Continue to risk-based assessment
    }

    match action {
        // === Always Auto ===
        Action::CreateFile { .. } => Auto,
        Action::ModifyCode { .. } => Auto,
        Action::RunTests { .. } => Auto,
        Action::GitCommit { .. } => Auto,
        Action::GitPush { branch } if !branch.is_protected() => Auto,
        Action::InstallDependency { .. } => Auto,
        Action::RunLint { .. } => Auto,

        // === Timed Auto (notify, proceed if no objection) ===
        Action::CreateProject { .. } => TimedAuto { delay: Duration::from_secs(600) },
        Action::ChangeTechStack { .. } => TimedAuto { delay: Duration::from_secs(600) },
        Action::GitPush { branch } if branch.is_protected() => TimedAuto { delay: Duration::from_secs(300) },
        Action::DeployStaging { .. } => TimedAuto { delay: Duration::from_secs(300) },
        Action::InstallMcpServer { .. } => TimedAuto { delay: Duration::from_secs(600) },

        // === Must Approve ===
        Action::DeployProduction { .. } => WaitForApproval,
        Action::DeleteProject { .. } => WaitForApproval,
        Action::DeleteData { .. } => WaitForApproval,
        Action::SpendMoney { amount } if *amount > owner_budget_threshold => WaitForApproval,
        Action::ModifySecurityConfig { .. } => WaitForApproval,
        Action::AccessSensitiveData { .. } => WaitForApproval,
        Action::SelfCoreModification { .. } => WaitForApproval,

        // === Default: based on risk assessment ===
        other => {
            let risk = assess_risk(other);
            match risk {
                Risk::Low => Auto,
                Risk::Medium => TimedAuto { delay: Duration::from_secs(600) },
                Risk::High => WaitForApproval,
            }
        }
    }
}
```

### 4.2 Decision Record

Every significant decision is recorded for learning and auditing:

```rust
struct DecisionRecord {
    id: Uuid,
    timestamp: DateTime<Utc>,

    // Context
    project_id: Option<Uuid>,
    task_id: Option<Uuid>,
    trigger: String,           // What prompted this decision

    // Decision
    options_considered: Vec<String>,
    chosen_option: usize,
    reasoning: String,
    confidence: f32,           // 0.0 ~ 1.0

    // Outcome (filled later)
    outcome: Option<DecisionOutcome>,
    outcome_recorded_at: Option<DateTime<Utc>>,
}

enum DecisionOutcome {
    Success { details: String },
    Failure { error: String, lesson: String },
    Overridden { by: String, reason: String },
}
```

## 5. Quality Check Loop

After every task completion, the Brain reviews the output:

```
Task Output
    │
    ▼
┌─────────────────────┐
│ 1. Run Tests         │ → test_commands from acceptance_criteria
└──────────┬──────────┘
           │
     Pass? ├── No → Analyze failure → Generate fix task → Re-queue
           │
           Yes
           │
           ▼
┌─────────────────────┐
│ 2. AI Code Review    │ → Use different model than the one that wrote the code
└──────────┬──────────┘
           │
     Quality OK?
           ├── No → Generate improvement task (low priority)
           │
           Yes
           │
           ▼
┌─────────────────────┐
│ 3. Security Scan     │ → Check for hardcoded secrets, SQL injection, XSS, etc.
└──────────┬──────────┘
           │
     Safe?
           ├── No → Generate security fix task (high priority)
           │
           Yes
           │
           ▼
┌─────────────────────┐
│ 4. Commit & Continue │ → Git commit, update progress, notify if needed
└─────────────────────┘
```

## 6. Cross-Model Code Review

To avoid blind spots from any single model, the Brain uses a different model for review than the one that generated the code:

| Code Written By | Reviewed By |
|-----------------|------------|
| Claude Sonnet (via OpenCode) | GPT-4o or DeepSeek |
| GPT-4o (via worker) | Claude Sonnet or DeepSeek |
| DeepSeek (via worker) | Claude Sonnet or GPT-4o |
| Any model | Local model for basic lint/security |

## 7. Learning from Outcomes

```rust
impl BrainEngine {
    /// Called when a decision outcome is known
    async fn learn_from_outcome(&self, decision: &DecisionRecord, outcome: &DecisionOutcome) {
        match outcome {
            DecisionOutcome::Success { details } => {
                // Store as positive experience
                self.memory.store(NewMemory {
                    layer: MemoryLayer::LongTerm,
                    memory_type: "successful_decision",
                    content: format!(
                        "Decision: {} → Option: {} → Result: Success ({})",
                        decision.trigger, decision.options_considered[decision.chosen_option], details
                    ),
                    importance: 0.7,
                    tags: vec!["decision", "success"],
                    ..Default::default()
                }).await;
            }
            DecisionOutcome::Failure { error, lesson } => {
                // Store as negative experience with higher importance
                self.memory.store(NewMemory {
                    layer: MemoryLayer::LongTerm,
                    memory_type: "failed_decision",
                    content: format!(
                        "Decision: {} → Option: {} → Result: FAILED ({}). Lesson: {}",
                        decision.trigger, decision.options_considered[decision.chosen_option], error, lesson
                    ),
                    importance: 0.9,  // Failures are more important to remember
                    tags: vec!["decision", "failure", "lesson"],
                    ..Default::default()
                }).await;
            }
            _ => {}
        }
    }
}
```
