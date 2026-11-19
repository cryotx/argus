# Argus Requirements Document

## 1. Project Identity

- **Name**: Argus (after Argus Panoptes, the hundred-eyed giant)
- **Type**: Autonomous AI Agent Operating System
- **Language**: Rust (core), React (Web Dashboard)
- **Database**: PostgreSQL + pgvector
- **Deployment**: Self-hosted VPS/Server

## 2. Stakeholder Requirements

### 2.1 Primary User (Owner)

The owner interacts with Argus primarily through social platforms (Telegram, Discord, WeChat, Slack) and a Web Dashboard. The owner's role is to:

- Define project goals and high-level requirements
- Approve critical decisions (production deployment, budget overruns)
- Provide feedback and new feature suggestions
- Monitor progress through periodic reports

### 2.2 Key Owner Preferences (Captured During Design)

| Preference | Decision |
|-----------|----------|
| Naming style | Mythological / Sci-fi |
| Core language | Rust |
| Social platforms | All five: Telegram, Discord, WeChat, Slack, Web Dashboard |
| Deployment target | Self-hosted VPS |
| MCP role | Host (call MCP servers, not be one) |
| Skills model | Built-in + third-party (with security audit) + self-generated |
| Storage | PostgreSQL + pgvector |
| Interaction model | Conversational — everything via dialog |
| Notification preference | Per-owner configurable, minimal by default |

## 3. Functional Requirements

### FR-01: First-Principles Project Decomposition

- **FR-01.1**: Given a natural language project description, Argus shall decompose it into milestones.
- **FR-01.2**: Each milestone shall be further decomposed into atomic, executable tasks.
- **FR-01.3**: Tasks shall have defined acceptance criteria, dependencies, and estimated duration.
- **FR-01.4**: Decomposition shall consider technical constraints, dependencies, and optimal execution order.
- **FR-01.5**: Decomposition shall leverage past experience from the memory system.

### FR-02: Multi-Project Management

- **FR-02.1**: Support managing 1-500 projects simultaneously.
- **FR-02.2**: Each project shall have independent workspace, Git repository, and deployment config.
- **FR-02.3**: Projects shall be type-agnostic: web apps, mobile apps, mini programs, APIs, CLI tools, scripts, documentation, etc.
- **FR-02.4**: Cross-project conflict detection shall prevent resource conflicts (ports, file paths, dependencies).
- **FR-02.5**: Per-project resource quotas shall prevent any single project from monopolizing resources.

### FR-03: Task Scheduling & Execution

- **FR-03.1**: Priority-based task queue with dependency graph resolution.
- **FR-03.2**: Task states: Pending → Ready → Queued → Assigned → InProgress → Testing → Completed/Failed.
- **FR-03.3**: Automatic retry on failure (configurable max attempts, default 3).
- **FR-03.4**: Task timeout enforcement with configurable per-task limits.
- **FR-03.5**: Parallel execution of independent tasks.
- **FR-03.6**: Task checkpointing for crash recovery.

### FR-04: Worker Management

- **FR-04.1**: Support multiple AI coding CLI tools: OpenCode, Claude Code, Aider, generic CLI.
- **FR-04.2**: Worker pool with configurable concurrency limit.
- **FR-04.3**: Sandbox isolation: directory-level (default) or Docker container.
- **FR-04.4**: Worker health monitoring and automatic recovery.
- **FR-04.5**: Smart worker selection based on task type, past performance, and project affinity.
- **FR-04.6**: Resource limits per worker (CPU, memory, disk, timeout).

### FR-05: Multi-Source LLM Engine

- **FR-05.1**: Support 10+ LLM providers: OpenAI, Anthropic, DeepSeek, Gemini, Qwen, Zhipu, Moonshot, local (Ollama), web scrapers.
- **FR-05.2**: Multiple API keys per provider with round-robin rotation.
- **FR-05.3**: Automatic failover: key exhausted → next key → next provider → web scraper → local model.
- **FR-05.4**: Smart routing: ModelClass (Reasoning, Coding, Fast, LongContext, Embedding) → best available model.
- **FR-05.5**: Token budget management: global, per-provider, per-project, per-task.
- **FR-05.6**: Cost attribution: track spending per project and per task type.
- **FR-05.7**: Usage prediction and budget warning (80% → warning, 90% → auto-downgrade).
- **FR-05.8**: Web scraper fallback: access free web-based AI interfaces (chat.qwen.ai, etc.) as last resort.

### FR-06: Four-Layer Memory System

- **FR-06.1**: **Working Memory** — Current task context, in-memory, cleared after task completion.
- **FR-06.2**: **Short-Term Memory** — Recent interactions (24-72h), auto-decay, PostgreSQL.
- **FR-06.3**: **Long-Term Memory** — Project knowledge and experience, pgvector semantic search, importance-weighted decay.
- **FR-06.4**: **Permanent Memory** — Core knowledge that never fades (owner preferences, critical decisions, safety rules).
- **FR-06.5**: Memory shall persist across LLM model switches — stored in database, not in LLM context.
- **FR-06.6**: Fixed embedding model (local, e.g. BGE-M3) independent of LLM selection for consistent vector search.
- **FR-06.7**: Memory consolidation: merge similar memories, extract general knowledge.
- **FR-06.8**: Memory promotion: frequently accessed short-term → long-term → permanent.
- **FR-06.9**: Conversation context rebuild: when switching models, reconstruct context from database-stored memory + summaries.
- **FR-06.10**: Token-budget-aware context building: total memory context ≤ 2000 tokens by default.

### FR-07: MCP Host

- **FR-07.1**: Implement MCP client supporting stdio, SSE, and streamable HTTP transports.
- **FR-07.2**: Auto-discover MCP servers from community registries.
- **FR-07.3**: Security audit before installing third-party MCP servers: static scan + behavioral analysis + LLM review.
- **FR-07.4**: Built-in MCP servers: filesystem, shell, database.
- **FR-07.5**: Conversational installation: "Install a browser automation tool" → find, audit, install, configure.
- **FR-07.6**: Tool trust levels: Builtin > Verified > ThirdParty > Untrusted.

### FR-08: Skills System

- **FR-08.1**: ~20 built-in core skills (project scaffolding, code review, testing, deployment, server setup, etc.).
- **FR-08.2**: Third-party skill installation with mandatory security scanning.
- **FR-08.3**: Self-generated skills: Argus writes new skills when existing ones can't satisfy a requirement.
- **FR-08.4**: Skill permission model: each skill declares required permissions (filesystem, network, shell, database, etc.).
- **FR-08.5**: Skills installable/removable/configurable via conversation.

### FR-09: Social Platform Integration

- **FR-09.1**: Telegram Bot with full feature support (Markdown, inline keyboards, file upload).
- **FR-09.2**: Discord Bot with dedicated channels (alerts, reports, control, per-project).
- **FR-09.3**: WeChat integration via Enterprise WeChat API or bridge tools (WeChatFerry).
- **FR-09.4**: Slack App with slash commands and interactive messages.
- **FR-09.5**: Web Dashboard with real-time WebSocket updates, full control panel.
- **FR-09.6**: Unified message protocol: all platforms normalized to common format.
- **FR-09.7**: Bidirectional communication: receive commands + proactive reporting.

### FR-10: Conversational Configuration

- **FR-10.1**: All system configuration modifiable through natural language conversation.
- **FR-10.2**: No manual config file editing required for normal operation.
- **FR-10.3**: Multi-turn dialog for complex operations requiring multiple inputs.
- **FR-10.4**: Configurable items include: notification preferences, LLM settings, worker settings, deploy settings, security rules, MCP/skills management, project settings.
- **FR-10.5**: Configuration changes logged in audit trail.
- **FR-10.6**: Undo capability for recent config changes via conversation ("undo that change").

### FR-11: Personalized Owner Profile

- **FR-11.1**: Per-owner notification preferences: Minimal / Low / Medium / High / RealTime.
- **FR-11.2**: Per-owner approval mode: FullAuto / TimedAuto / RiskBased / ManualAll.
- **FR-11.3**: Active hours detection: buffer non-urgent messages during sleep/inactive hours.
- **FR-11.4**: Communication style: Minimal / Concise / Detailed / Technical.
- **FR-11.5**: Automatic preference learning from interaction patterns (response time, approval tendency, etc.).
- **FR-11.6**: Multi-owner support with role-based permissions (Owner/Admin/Developer/Viewer).

### FR-12: Git Operations

- **FR-12.1**: Automatic branch creation, commit, push for every task.
- **FR-12.2**: PR creation with AI-generated descriptions.
- **FR-12.3**: Branch strategy management (main, develop, feature branches).
- **FR-12.4**: Semantic versioning automation.
- **FR-12.5**: Multi-repository management.
- **FR-12.6**: Conflict detection and resolution (auto-resolve or escalate).

### FR-13: Deployment Engine

- **FR-13.1**: Multi-stage pipeline: build → test → package → pre-check → deploy → health check.
- **FR-13.2**: Multiple deployment targets: Docker, VPS (SSH), Vercel, K8s, WeChat Mini Program, App Store.
- **FR-13.3**: Automatic rollback on health check failure.
- **FR-13.4**: Configurable approval gates for production deployments.
- **FR-13.5**: Environment management: dev, staging, production.
- **FR-13.6**: Deployment history with snapshot-based rollback.

### FR-14: Automatic Environment Setup

- **FR-14.1**: Detect missing dependencies when setting up a project.
- **FR-14.2**: Auto-install required runtimes, tools, services (Node.js, Python, Docker, Nginx, etc.).
- **FR-14.3**: Use version managers where appropriate (nvm, pyenv, rustup).
- **FR-14.4**: Respect owner preferences for auto-install vs. ask-for-approval.

### FR-15: Self-Evolution

- **FR-15.1**: Argus can develop new features for itself using its own workers.
- **FR-15.2**: Self-modification happens on isolated branches with full test suite.
- **FR-15.3**: Hot reload for Skills and config changes (no restart needed).
- **FR-15.4**: Graceful restart or blue-green deployment for core changes.
- **FR-15.5**: Automatic experience learning: analyze success/failure patterns, adjust strategies.
- **FR-15.6**: Cross-project knowledge transfer.

### FR-16: General-Purpose Operations (Beyond Code)

- **FR-16.1**: Browser automation via MCP (post on social media, scrape websites, fill forms).
- **FR-16.2**: File management and server administration.
- **FR-16.3**: Email sending/receiving via MCP.
- **FR-16.4**: Data analysis and report generation.
- **FR-16.5**: Any operation achievable through available MCP servers and skills.
- **FR-16.6**: Self-identification of missing capabilities with proactive tool suggestion.

### FR-17: Token Optimization

- **FR-17.1**: Tiered model selection: cheap models for simple tasks, powerful for complex.
- **FR-17.2**: Context compression: summarize long contexts, send only relevant code snippets.
- **FR-17.3**: Response caching: cache similar requests.
- **FR-17.4**: Local model priority: use Ollama for tasks that don't need cloud models.
- **FR-17.5**: Batch processing: merge similar tasks into single LLM calls.
- **FR-17.6**: Progressive precision: start cheap, escalate only on failure.

### FR-18: Role-Based Collaboration

- **FR-18.1**: For every task, Argus shall perform an automatic complexity assessment based on factors such as multi-module impact, file count, security sensitivity, database migration, public API change, concurrency, and new architectural patterns. The assessment produces a numeric score (0.0–1.0) and a risk level (Low / Medium / High / Critical).
- **FR-18.2**: Based on the complexity score, Argus shall select one of three collaboration modes: **Solo** (score < 0.3, single worker, no collaboration), **PairReview** (0.3 ≤ score < 0.6, one writer + one reviewer), or **FullTeam** (score ≥ 0.6, multi-role pipeline). See `COLLABORATION.md` for full details.
- **FR-18.3**: Argus shall support six collaboration roles — Architect, Developer, Reviewer, Tester, SecurityAuditor, DevOps — with the Brain Engine acting as TechLead/project manager. The Brain auto-determines which roles are needed based on the task type and complexity.
- **FR-18.4**: Worker tools shall be classified into capability tiers: **CollaborationCapable** (OpenCode, Claude Code — support structured output, system prompt injection, session persistence) and **SoloOnly** (Aider, generic CLI — usable only in Solo mode). Complex collaborative tasks shall only be assigned to CollaborationCapable workers.
- **FR-18.5**: Review policies shall be enforced per risk level: Low risk requires no review; Medium risk requires PairReview (same model OK, 1 round); High risk requires FullTeam with forced different-model review (2 rounds + Security Auditor); Critical risk adds mandatory owner approval. See `COLLABORATION.md` Section 5 for the full policy matrix.
- **FR-18.6**: Inter-role communication shall use the file system (`.argus/collab/<session_id>/`) with structured artifacts (architecture docs, implementation summaries, review reports, test reports). The Brain coordinates handoffs between roles.
- **FR-18.7**: Collaboration sessions shall enforce cost controls including per-session token budgets, per-role token limits, and automatic escalation to the owner when budget thresholds are exceeded. Cost attribution is tracked per session, per role, and per review round.
- **FR-18.8**: Collaboration settings (mode thresholds, role assignments, review policies, cost limits) shall be manageable through conversational configuration, consistent with FR-10.

## 4. Non-Functional Requirements

### NFR-01: Performance

- System event processing latency < 1ms.
- Intent classification < 2 seconds.
- Task scheduling decision < 100ms.
- Memory semantic search < 500ms.
- Support 100+ concurrent projects.
- Support 5-10 concurrent workers (configurable by resources).

### NFR-02: Reliability

- System uptime target: 99.9%.
- Automatic crash recovery via systemd watchdog.
- Task checkpoint/resume after restart.
- Graceful degradation when LLM APIs unavailable.
- Message queue persistence for social platform disconnects.

### NFR-03: Security

- All API keys encrypted at rest (AES-256-GCM).
- Complete audit log of all operations.
- Worker processes run as non-root users.
- Third-party code (MCP servers, skills) run in sandboxes.
- Dangerous commands (rm -rf, force push main) are blocked.
- Production deployments require configurable approval.
- Communication over TLS.

### NFR-04: Maintainability

- Modular crate structure with clear boundaries.
- Comprehensive logging via tracing.
- All configuration via TOML with conversational override.
- Database migrations via sqlx-cli.
- Version-controlled schema evolution.

### NFR-05: Scalability

- Horizontal worker scaling (more workers as resources allow).
- Vertical scaling through resource limits and quotas.
- Efficient memory system with decay and consolidation (no unbounded growth).

### NFR-06: Data Portability

- All project code in Git (standard, portable).
- Memory exportable as JSON/SQL dump.
- Configuration exportable as TOML.
- No vendor lock-in to any specific LLM provider.

## 5. Constraints

- **Single-binary deployment**: Argus compiles to a single Rust binary plus Web Dashboard assets.
- **PostgreSQL required**: The system requires a PostgreSQL instance with pgvector extension.
- **Internet required**: For LLM API access (but degrades gracefully to local models when offline).
- **Unix-based**: Primary target is Linux; macOS supported for development.

## 6. Acceptance Criteria Summary

| Feature | Acceptance Criteria |
|---------|-------------------|
| Project decomposition | Given a 2-sentence project description, produces valid milestone/task tree in < 30s |
| Multi-project | 50 projects active simultaneously without resource conflicts |
| LLM failover | When primary provider key is exhausted, switches to next within 1 second |
| Memory persistence | After model switch, can recall relevant past decisions and experiences |
| Conversational config | "Change notification to minimal" correctly updates config |
| Self-evolution | "Add feature X" → develops, tests, and deploys the feature autonomously |
| Social integration | Command sent via Telegram results in correct action within 10 seconds |
| Deployment | Auto-deploy to staging after tests pass; rollback within 60 seconds on failure |
| Token optimization | For simple tasks, uses models costing < $0.001 per task |
| Security | Third-party MCP server with suspicious code is rejected during audit |
| Role-based collaboration | High-risk task triggers FullTeam mode with different-model cross-review; Solo mode for simple tasks completes without unnecessary overhead |
