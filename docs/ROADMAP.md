# Development Roadmap

This document defines the phased development plan for Argus, from workspace initialization to full autonomous operation. Each phase has clear deliverables and acceptance criteria.

**Estimated total timeline: 21-27 weeks** (single developer, full-time)

---

## Phase Overview

| Phase | Name | Duration | Deliverable |
|-------|------|----------|------------|
| P0 | Foundation | 1 week | Workspace, event bus, CLI skeleton, DB migrations |
| P1 | Memory Engine | 1.5 weeks | Four-layer memory with BGE-M3 embeddings, pgvector search |
| P2 | LLM Provider Engine | 1.5 weeks | Multi-provider routing, key rotation, failover, budget tracking |
| P3 | Brain Engine | 2 weeks | Intent classification, first-principles decomposer, decision engine |
| P4 | Task Scheduler | 1.5 weeks | Priority queue, dependency DAG, state machine, checkpointing |
| P5 | Worker Manager & Collaboration | 3 weeks | Worker pool, OpenCode/ClaudeCode/Aider drivers, sandboxing, collaboration system |
| P6 | **Milestone: Single-Project MVP** | 0.5 weeks | Integration test: decompose → schedule → execute → commit |
| P7 | Git Operations | 1 week | Clone, branch, commit, push, PR, conflict detection |
| P8 | MCP & Skills | 2 weeks | MCP client, server registry, security audit, built-in skills |
| P9 | Deploy Engine | 2 weeks | Pipeline engine, Docker/VPS deployers, health checks, rollback |
| P10 | Social Gateway | 2 weeks | Telegram + Web Dashboard adapters, notification engine, dialog manager |
| P11 | Security | 1.5 weeks | Key vault, RBAC, audit logging, command guard, secret scanner |
| P12 | Conversational Config | 1 week | NL config parsing, validation, undo, hot-reload |
| P13 | Self-Evolution | 2 weeks | Isolated branch dev, test instance, blue-green deploy, guardrails |
| P14 | Polish & Harden | 1-2 weeks | Load testing, edge cases, documentation, packaging |

---

## P0 — Foundation (Week 1)

**Goal**: Runnable Rust workspace with event bus, CLI entry point, and database schema applied.

### Deliverables

1. **Cargo workspace** with all 12 crate stubs
   ```
   argus/
   ├── Cargo.toml (workspace)
   └── crates/
       ├── argus-core/Cargo.toml
       ├── argus-llm/Cargo.toml
       ├── argus-memory/Cargo.toml
       ├── argus-worker/Cargo.toml
       ├── argus-mcp/Cargo.toml
       ├── argus-skills/Cargo.toml
       ├── argus-git/Cargo.toml
       ├── argus-deploy/Cargo.toml
       ├── argus-social/Cargo.toml
       ├── argus-monitor/Cargo.toml
       ├── argus-security/Cargo.toml
       └── argus-cli/Cargo.toml
   ```

2. **Event bus** (`argus-core/src/event_bus.rs`)
   - `EventBus::new()`, `publish()`, `subscribe()`
   - `EventEnvelope`, `EventSource`, `ArgusEvent` (full enum from API_DESIGN.md)
   - Filtered subscription helper
   - Unit tests

3. **CLI entry point** (`argus-cli/src/main.rs`)
   - `clap` argument parsing: `argus run`, `argus status`, `argus version`
   - Tokio runtime initialization
   - Signal handling (SIGINT, SIGTERM → graceful shutdown)
   - Tracing/logging initialization (`tracing-subscriber` with JSON output)

4. **Database migrations** (`migrations/0001-0021`)
    - All 21 migration files from DATABASE_SCHEMA.md
   - `sqlx-cli` integration
   - `argus migrate` CLI subcommand

5. **Configuration loading**
   - TOML config file parsing with serde
   - Default config template at `config/argus.default.toml`
   - Environment variable overrides (`ARGUS_DATABASE_URL`, etc.)

6. **Shared types** (`argus-core/src/types.rs`)
   - Common ID types, error types, result aliases
   - Re-exports used by all crates

### Acceptance Criteria

- `cargo build` compiles the entire workspace with zero warnings
- `cargo test` passes all event bus unit tests
- `argus run` starts, connects to PostgreSQL, applies migrations, and prints "Argus started"
- `argus version` prints version
- `Ctrl+C` triggers graceful shutdown with log message

---

## P1 — Memory Engine (Weeks 2-3)

**Goal**: Fully functional four-layer memory with local embedding generation and semantic search.

### Deliverables

1. **Embedding engine** (`argus-memory/src/embedding.rs`)
   - BGE-M3 via `fastembed-rs` (local, no API calls)
   - `embed_text(text) → Vec<f32>` (1024 dimensions)
   - Batch embedding support
   - Model loaded once at startup, shared via `Arc`

2. **Memory store** (`argus-memory/src/store.rs`)
   - `store_memory(content, layer, memory_type, project_id, tags)` → embed → insert
   - Near-duplicate detection (cosine similarity > 0.95 → update instead of insert)
   - Layer-aware insertion (working = RAM-only, others = PostgreSQL)

3. **Memory recall** (`argus-memory/src/recall.rs`)
   - `recall(query, options)` → embed query → pgvector cosine search
   - `RecallOptions`: layers, types, project_id, min_similarity, limit
   - Results sorted by `similarity * importance`

4. **Memory lifecycle** (`argus-memory/src/lifecycle.rs`)
   - Decay: daily job reduces importance based on `decay_rate`
   - Promotion: short-term → long-term when importance > 0.7 and access_count >= 3
   - Consolidation: LLM summarizes related memories into one
   - Expiration: delete memories past `expires_at`

5. **Context builder** (`argus-memory/src/context.rs`)
   - `build_context(query, token_budget, project_id)` → ranked memories that fit budget
   - Priority: permanent → project long-term → cross-project → short-term

6. **Working memory** (`argus-memory/src/working.rs`)
   - In-memory `DashMap<Uuid, WorkingMemory>` per-task
   - Auto-flush to short-term on task completion

### Acceptance Criteria

- Store 1000 memories, recall top-10 by semantic similarity in < 500ms
- Embedding generation works offline (no API calls)
- Decay function reduces importance correctly over simulated time
- Context builder respects token budget

---

## P2 — LLM Provider Engine (Weeks 3-4.5)

**Goal**: Multi-provider LLM access with smart routing, failover, and cost tracking.

### Deliverables

1. **Provider trait** (`argus-llm/src/provider.rs`)
   - `LlmProvider` trait: `complete(messages, options) → LlmResponse`
   - Streaming support: `complete_stream(messages, options) → Stream<LlmChunk>`

2. **Provider implementations** (`argus-llm/src/providers/`)
   - OpenAI, Anthropic, DeepSeek, Gemini, Qwen, Zhipu, Moonshot
   - All via `reqwest` HTTP client
   - Ollama (local model) via HTTP API

3. **Model router** (`argus-llm/src/router.rs`)
   - `ModelClass` enum: Reasoning, Coding, Fast, LongContext, Embedding
   - Route by task type → model class → best available provider
   - Respect per-provider budget limits

4. **Key rotation** (`argus-llm/src/key_rotation.rs`)
   - Round-robin across multiple API keys per provider
   - Mark key as exhausted on 429/rate-limit, rotate to next
   - Reset keys on timer (monthly/daily based on provider)

5. **Auto-failover** (`argus-llm/src/failover.rs`)
   - Provider priority chain: primary → secondary → ... → web scraper → local
   - Automatic switch on provider failure, emit `LlmProviderSwitched` event
   - Circuit breaker: 3 consecutive failures → mark provider as down for 5 minutes

6. **Token budget tracker** (`argus-llm/src/budget.rs`)
   - Track per-provider, per-project, per-task usage
   - Insert into `llm_usage` table
   - Budget warnings at 80%, hard stop at 100% (configurable)

7. **Web scraper fallback** (`argus-llm/src/web_scraper.rs`)
   - `WebScraperProvider` trait (stubbed for P2, implemented later)
   - Placeholder for Qwen free web, Poe, etc.

### Acceptance Criteria

- Send a prompt to Anthropic, receive response, track tokens
- When Anthropic key is exhausted, auto-switch to DeepSeek
- Budget tracker prevents usage beyond monthly limit
- Concurrent requests to different providers work correctly

---

## P3 — Brain Engine (Weeks 4.5-6.5)

**Goal**: Argus can understand natural language commands, decompose them into task trees, and make autonomous decisions.

### Deliverables

1. **Intent classifier** (`argus-core/src/brain/intent.rs`)
   - LLM-based classification: message → `Intent` enum (~40 variants)
   - Uses Fast model class for speed
   - Confidence threshold: > 0.7 auto-classify, < 0.7 ask for clarification

2. **First-principles decomposer** (`argus-core/src/brain/decomposer.rs`)
   - 4-phase pipeline: goal clarification → technical decisions → milestones → atomic tasks
   - Each phase uses LLM with structured output (JSON schema)
   - Produces `Vec<Milestone>` containing `Vec<Task>` with dependency edges

3. **Decision engine** (`argus-core/src/brain/decision.rs`)
   - 3 decision levels: Auto (low-risk), TimedAuto (medium-risk, 30min timeout), WaitForApproval (high-risk)
   - Decision records stored in memory for learning
   - Cross-model code review matrix (task done by model A, reviewed by model B)

4. **Quality check loop** (`argus-core/src/brain/quality.rs`)
   - After task completion: run tests → AI code review → security scan
   - If any check fails: generate fix task and re-queue
   - Max iteration count (default 3) before escalating to owner

5. **Learning from outcomes** (`argus-core/src/brain/learning.rs`)
   - On task success/failure: store experience memory with tags
   - Recall similar experiences when decomposing similar tasks
   - Track which approaches work for which types of tasks

### Acceptance Criteria

- Classify "帮我用Next.js做一个博客" correctly as ProjectCreate
- Decompose it into milestones and atomic tasks with correct dependency graph
- Decision engine correctly identifies deploy-to-production as WaitForApproval
- Quality loop detects test failure and generates fix task

---

## P4 — Task Scheduler (Weeks 6.5-8)

**Goal**: Priority-based task scheduling with dependency resolution, crash recovery, and progress tracking.

### Deliverables

1. **Task state machine** (`argus-core/src/scheduler/state.rs`)
   - 10 states with validated transitions
   - State change emits event to event bus

2. **Dependency graph** (`argus-core/src/scheduler/dag.rs`)
   - DAG implementation with topological sort
   - Cycle detection on task creation
   - Auto-promote to `ready` when all dependencies completed

3. **Priority queue** (`argus-core/src/scheduler/queue.rs`)
   - Effective priority = base + age_boost + dependency_boost - retry_penalty
   - Re-computed periodically (every priority update cycle)

4. **Scheduling loop** (`argus-core/src/scheduler/loop.rs`)
   - 500ms tick: promote_ready → check_timeouts → assign_tasks → update_priorities
   - Assign tasks to workers via WorkerManager

5. **Checkpointing** (`argus-core/src/scheduler/checkpoint.rs`)
   - Save task state to `task_checkpoints` table periodically
   - On crash recovery: restore from latest checkpoint

6. **Cross-project conflict detection** (`argus-core/src/scheduler/conflicts.rs`)
   - Detect port conflicts, shared file locks, container name collisions
   - Serialize conflicting tasks

### Acceptance Criteria

- 50 tasks with complex dependency graph scheduled correctly
- High-priority task preempts low-priority task assignment
- After simulated crash, tasks resume from checkpoint
- Timed-out task detected and re-queued

---

## P5 — Worker Manager & Collaboration (Weeks 8-11)

**Goal**: Spawn and manage multiple AI coding tool subprocesses with sandboxing and role-based collaboration.

### Deliverables

1. **Worker driver trait** (`argus-worker/src/driver.rs`)
   - `WorkerDriver`: spawn, send_task, read_output, health_check, kill

2. **Driver implementations** (`argus-worker/src/drivers/`)
   - OpenCode driver (stdin/stdout communication)
   - Claude Code driver
   - Aider driver
   - Generic CLI driver (arbitrary commands)

3. **Worker pool** (`argus-worker/src/pool.rs`)
   - Configurable max pool size
   - Smart worker selection: match task type to best worker type
   - Idle worker recycling (configurable timeout)

4. **Sandbox isolation** (`argus-worker/src/sandbox.rs`)
   - Directory sandbox: chroot-like path restriction
   - Docker sandbox: via `bollard` crate, per-worker container
   - Resource limits: memory, CPU, disk, timeout

5. **Resource monitor** (`argus-worker/src/monitor.rs`)
   - Poll `/proc/<pid>/status` for memory/CPU
   - Kill workers exceeding resource limits
   - Emit `WorkerResourceAlert` events

6. **Communication protocol** (`argus-worker/src/protocol.rs`)
    - TaskPayload → worker stdin (JSON)
    - Worker stdout → WorkerOutput (streaming)
    - Timeout enforcement

7. **Collaboration system** (`argus-worker/src/collaboration/`)
    - Role-based collaboration sessions (Solo, PairReview, FullTeam modes)
    - Complexity assessment and automatic mode selection
    - Role assignment: architect, developer, reviewer, tester, security auditor
    - Multi-phase pipeline: architecture → implementation → review → testing → merging
    - Review policy enforcement with configurable approval thresholds
    - Collaboration artifacts and review round tracking
    - See COLLABORATION.md for full design

### Acceptance Criteria

- Spawn OpenCode worker, send coding task, receive result
- Docker sandbox enforces memory limit (kills on exceed)
- Worker crash detected within 5 seconds, task re-queued
- 5 workers running concurrently without interference
- Collaboration session assigns roles and completes multi-phase pipeline
- Review round correctly blocks merge on request_changes verdict

---

## P6 — Single-Project MVP (Week 11.5)

**Goal**: End-to-end integration test proving the core loop works.

### Test Scenario

1. User sends (via CLI): "创建一个Rust的Hello World项目，加上单元测试"
2. Brain classifies intent → ProjectCreate
3. Brain decomposes into tasks:
   - Create project directory
   - Initialize Cargo project
   - Write main.rs with hello world
   - Write lib.rs with function + test
   - Run `cargo test`
   - Commit to git
4. Scheduler orders tasks by dependency
5. Worker (OpenCode) executes each task
6. Quality check runs `cargo test` → passes
7. Git commit created
8. Memory stores: project context, decisions, experience

### Acceptance Criteria

- Entire flow completes autonomously from single NL command
- All tasks complete without human intervention
- Tests pass
- Git history shows meaningful commits
- Memories retrievable by semantic search

---

## P7 — Git Operations (Weeks 11.5-12.5)

**Goal**: Full git workflow management.

### Deliverables

1. **Repository manager** (`argus-git/src/repo.rs`)
   - Clone, init, status, diff via `gitoxide`/`gix`
   - Branch management: create, switch, delete, list

2. **Commit workflow** (`argus-git/src/commit.rs`)
   - Stage changes, create commit with LLM-generated message
   - Conventional commit format (feat:, fix:, refactor:, etc.)

3. **Remote operations** (`argus-git/src/remote.rs`)
   - Push, pull, fetch
   - SSH key authentication (keys from KeyVault)
   - HTTPS token authentication

4. **PR workflow** (`argus-git/src/pr.rs`)
   - Create PR via GitHub/GitLab API
   - PR description generated by LLM
   - Handle review comments

5. **Conflict resolution** (`argus-git/src/conflict.rs`)
   - Detect merge conflicts
   - Auto-resolve simple conflicts (non-overlapping)
   - Escalate complex conflicts to owner

### Acceptance Criteria

- Create branch, make changes, commit, push, create PR — all automated
- SSH auth works with key from KeyVault
- Merge conflict detected and escalated correctly

---

## P8 — MCP & Skills (Weeks 12.5-14.5)

**Goal**: MCP client with server management and extensible skills system.

### Deliverables

1. **MCP client** (`argus-mcp/src/client.rs`)
   - Protocol support: stdio, SSE, Streamable HTTP
   - `connect(server_config) → McpClient`
   - `call_tool(server, tool_name, args) → ToolResult`

2. **Server registry** (`argus-mcp/src/registry.rs`)
   - Install/remove servers, persist to DB
   - Trust levels: Builtin, Verified, ThirdParty, Untrusted
   - Auto-discovery from community registries (npm, GitHub)

3. **Security auditor** (`argus-mcp/src/audit.rs`)
   - 4-stage pipeline: static scan → behavioral analysis → LLM review → verdict
   - Auto-assign trust level based on audit result

4. **Built-in servers** (`argus-mcp/src/builtin/`)
   - Filesystem server (read/write/search with path restrictions)
   - Shell server (execute commands with CommandGuard)
   - Database server (query PostgreSQL with read-only default)

5. **Tool router** (`argus-mcp/src/router.rs`)
   - Brain requests tool → router finds appropriate MCP server or built-in skill
   - Permission checking before execution

6. **Skills engine** (`argus-skills/src/`)
   - SkillManifest, permissions, execution runtime
   - ~10 built-in skills (scaffold, lint, test, format, search, analyze, etc.)
   - Self-generated skills (LLM writes Python scripts, audits, saves to DB)

### Acceptance Criteria

- Install a third-party MCP server via conversation
- Security audit runs and assigns trust level
- Built-in filesystem server reads/writes files correctly
- Skill execution respects permission boundaries

---

## P9 — Deploy Engine (Weeks 14.5-16.5)

**Goal**: Automated deployment pipeline with Docker and VPS support.

### Deliverables

1. **Pipeline engine** (`argus-deploy/src/pipeline.rs`)
   - 6-stage pipeline: Build → Test → Package → PreCheck → Deploy → HealthCheck
   - Stage execution with timeout, retry, approval gates

2. **Docker deployer** (`argus-deploy/src/deployers/docker.rs`)
   - Build image, push to registry, deploy container
   - Docker Compose support
   - Via `bollard` crate

3. **VPS deployer** (`argus-deploy/src/deployers/vps.rs`)
   - SSH connection via `russh`
   - Rsync file transfer
   - systemd/pm2 service management

4. **Health checker** (`argus-deploy/src/health.rs`)
   - HTTP probe, TCP probe, command probe
   - Success/failure thresholds
   - Auto-rollback on health check failure

5. **Rollback manager** (`argus-deploy/src/rollback.rs`)
   - Pre-deploy snapshots
   - One-command rollback to previous version

6. **Environment manager** (`argus-deploy/src/environment.rs`)
   - Dev, staging, production per project
   - Promote between environments

### Acceptance Criteria

- Deploy a Docker container, health check passes
- Deploy to VPS via SSH, systemd service starts
- Health check failure triggers automatic rollback
- Rollback restores previous version correctly

---

## P10 — Social Gateway (Weeks 16.5-18.5)

**Goal**: Communicate with owner via Telegram and Web Dashboard.

### Deliverables

1. **Telegram adapter** (`argus-social/src/adapters/telegram.rs`)
   - Via `teloxide` crate
   - Receive messages, send responses
   - Inline keyboard buttons for approvals
   - File/image sending for reports

2. **Web Dashboard adapter** (`argus-social/src/adapters/web.rs`)
   - Axum HTTP server for REST API (from API_DESIGN.md)
   - WebSocket for real-time updates
   - Serve React dashboard static files

3. **Notification engine** (`argus-social/src/notification.rs`)
   - Priority-based routing (critical → immediate push, low → digest)
   - Disturbance-level awareness (respect owner's active hours)
   - Message buffering during inactive hours

4. **Multi-turn dialog manager** (`argus-social/src/dialog.rs`)
   - State machine for complex conversations
   - Project creation flow, config flow, approval flow
   - Context preserved across messages

5. **Owner profile** (`argus-social/src/owner.rs`)
   - Preferences: language, timezone, communication style
   - Active hours, disturbance level
   - Learned preferences from interaction patterns

### Acceptance Criteria

- Send message via Telegram, receive response
- Web Dashboard shows real-time task updates via WebSocket
- Notification respects owner's active hours (buffered outside)
- Multi-turn dialog completes project creation flow

---

## P11 — Security (Weeks 18.5-20)

**Goal**: Production-grade security for all operations.

### Deliverables

1. **Key vault** (`argus-security/src/vault.rs`)
   - AES-256-GCM encryption, Argon2id key derivation
   - Store/get/delete/list/rotate secrets
   - Master password never stored

2. **RBAC** (`argus-security/src/rbac.rs`)
   - 4 roles: Owner, Admin, Developer, Viewer
   - Permission matrix enforcement
   - Identity resolution from platform credentials

3. **Audit logging** (`argus-security/src/audit.rs`)
   - All security-relevant actions logged
   - Buffered batch insert with immediate flush for critical events

4. **Command guard** (`argus-security/src/guard.rs`)
   - Blocked patterns: `rm -rf /`, fork bombs, `mkfs`
   - Warning patterns: `sudo`, `rm -rf`
   - Approval-required patterns: force push, DROP DATABASE

5. **Secret scanner** (`argus-security/src/scanner.rs`)
   - Regex patterns for API keys, passwords, private keys
   - Scan worker output, git diffs, logs
   - Auto-redact detected secrets

### Acceptance Criteria

- Secrets encrypted at rest, decrypted only with master password
- Viewer role cannot trigger deployments (403)
- `rm -rf /` blocked with audit log entry
- API key in worker output is redacted before storage

---

## P12 — Conversational Config (Weeks 20-21)

**Goal**: All configuration changes via natural language.

### Deliverables

1. **Config resolver** (`argus-core/src/config/resolver.rs`)
   - NL → ConfigIntent → ConfigChange pipeline
   - LLM-powered parsing with domain map
   - Confidence threshold for confirmation

2. **Config validator** (`argus-core/src/config/validator.rs`)
   - Per-domain validation rules
   - Warnings for potentially dangerous changes

3. **Config applier** (`argus-core/src/config/applier.rs`)
   - Validate → store undo → write → audit → notify
   - Hot-reload for applicable modules

4. **Undo manager** (`argus-core/src/config/undo.rs`)
   - In-memory stack of recent changes
   - "撤销" (undo) command support

### Acceptance Criteria

- "把最大worker数改成10" correctly changes config
- "撤销" reverts the change
- Invalid value rejected with explanation
- Config change hot-reloads worker pool size

---

## P13 — Self-Evolution (Weeks 21-23)

**Goal**: Argus can modify its own source code safely.

### Deliverables

1. **Evolution engine** (`argus-core/src/evolution/engine.rs`)
   - 10-step pipeline from request to completion
   - Safety checks and guardrails enforced

2. **Test runner** (`argus-core/src/evolution/test.rs`)
   - Run `cargo fmt`, `clippy`, `build`, `test` on modified code
   - Fail-fast on any check failure

3. **Deployment strategies** (`argus-core/src/evolution/deploy.rs`)
   - Hot reload (skills/config — no restart)
   - Graceful restart (non-critical — finish tasks, swap binary)
   - Blue-green deploy (core changes — parallel instances)

4. **Guardrail enforcer** (`argus-core/src/evolution/guardrails.rs`)
   - Immutable rules: cannot modify security core, cannot remove guardrails
   - Approval-required rules: test coverage threshold, max file count

5. **Conflict detector** (`argus-core/src/evolution/conflicts.rs`)
   - Detect concurrent evolutions
   - Detect conflicts with active tasks

### Acceptance Criteria

- Simple skill addition: hot-reloaded without restart
- Core change: blue-green deployed with health check
- Attempt to modify guardrail file: rejected with explanation
- Test failure: evolution rolled back automatically

---

## P14 — Polish & Harden (Weeks 23-25)

**Goal**: Production readiness.

### Deliverables

1. **Additional social adapters**
   - Discord adapter (if not done in P10)
   - Slack adapter (basic)
   - WeChat Enterprise adapter (basic)

2. **Additional deployers**
   - Vercel deployer
   - K8s deployer (basic)

3. **Monitoring & observability**
   - Prometheus metrics export (`/metrics`)
   - Health check endpoint refinement
   - System watchdog (auto-restart on crash)

4. **Load testing**
   - 50+ concurrent tasks
   - 100+ simultaneous projects in DB
   - WebSocket with 50+ concurrent connections

5. **Edge case handling**
   - Network partitions (DB disconnect, LLM timeout)
   - Disk full scenarios
   - OOM scenarios
   - Corrupted checkpoint recovery

6. **Packaging**
   - Single binary distribution (`cargo build --release`)
   - Systemd service file
   - Docker image for Argus itself
   - Install script

7. **Web Dashboard** (React)
   - Project list/detail views
   - Task board (Kanban-style)
   - Deployment history
   - Real-time log viewer
   - Config editor
   - LLM usage dashboard

### Acceptance Criteria

- 24-hour stability test with continuous task execution
- All social platform adapters connect and exchange messages
- Docker/VPS deploys succeed end-to-end
- Web Dashboard renders all views correctly
- Single binary + `argus run` on a fresh VPS = working system

---

## Milestone Summary

```
Week 0   ────────────────── P0: Foundation
Week 1   ─────────┐
Week 2   ─────────┤ P1: Memory Engine
Week 3   ─────────┤
         ─────────┤ P2: LLM Provider
Week 4.5 ─────────┘
         ─────────┐
Week 6.5 ─────────┤ P3: Brain Engine
         ─────────┤
Week 8   ─────────┤ P4: Task Scheduler
         ─────────┤
Week 11  ─────────┤ P5: Worker Manager & Collaboration
Week 11.5─────────┤ P6: ★ SINGLE-PROJECT MVP ★
         ─────────┤
Week 12.5─────────┤ P7: Git Operations
         ─────────┤
Week 14.5─────────┤ P8: MCP & Skills
         ─────────┤
Week 16.5─────────┤ P9: Deploy Engine
         ─────────┤
Week 18.5─────────┤ P10: Social Gateway
         ─────────┤
Week 20  ─────────┤ P11: Security
         ─────────┤
Week 21  ─────────┤ P12: Conversational Config
         ─────────┤
Week 23  ─────────┤ P13: Self-Evolution
         ─────────┤
Week 25  ─────────┘ P14: Polish & Harden
                    ★ FULL RELEASE ★
```

---

## Critical Path

The critical path determines the minimum time to MVP (P6):

```
P0 (Foundation) → P1 (Memory) → P2 (LLM) → P3 (Brain) → P4 (Scheduler) → P5 (Worker+Collab) → P6 (MVP)
     1 week        1.5 weeks     1.5 weeks    2 weeks      1.5 weeks        3 weeks             0.5 weeks
                                                                                        Total: ~11 weeks
```

Some phases can be partially parallelized:
- P1 (Memory) and P2 (LLM) can be developed simultaneously (only share DB)
- P7 (Git) and P8 (MCP) can overlap
- P11 (Security) can start alongside P10 (Social)

With parallelization, **MVP in ~9 weeks** and **full release in ~21 weeks** is achievable.

---

## Risk Mitigation

| Risk | Impact | Mitigation |
|------|--------|-----------|
| fastembed-rs BGE-M3 performance on CPU | Slow embedding | Pre-compute embeddings in batch; accept ~100ms per embed |
| Worker subprocess communication reliability | Lost tasks | Checkpoint every 30s; timeout detection; retry with backoff |
| LLM costs during development | Budget overrun | Use local Ollama for development; only test paid APIs periodically |
| Self-evolution safety | System corruption | Immutable guardrails file; always require owner approval for core changes |
| PostgreSQL pgvector at scale (100K+ memories) | Slow recall | HNSW index tuning; partition by project; prune low-importance memories |
| Web scraper API changes | Fallback breaks | Scraper trait with multiple implementations; graceful degradation |
| Complex multi-turn NL parsing | Misclassification | Confidence thresholds; always ask for confirmation on low confidence |

---

## Development Environment

- **Rust toolchain**: stable (latest)
- **Database**: PostgreSQL 16+ with pgvector extension
- **Container runtime**: Docker (for worker sandboxing and testing deployers)
- **AI tools for development**: Use Argus's own worker pool concept — develop with OpenCode/Claude Code
- **Testing**: `cargo test` for unit tests, custom integration test harness for end-to-end
- **CI**: GitHub Actions (build + test + clippy + fmt on every push)
