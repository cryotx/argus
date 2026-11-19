# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Argus is a self-governing autonomous AI agent operating system built in Rust. It can decompose project goals, develop software, manage multiple projects, and self-evolve with minimal human intervention.

## Commands

```bash
# Setup (first time)
./scripts/setup.sh

# Database migrations
cargo run -p argus-cli -- migrate

# Run the daemon
cargo run -p argus-cli -- run

# Build
cargo build --workspace
cargo check --workspace

# Lint and format
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings

# Tests
cargo test --workspace
cargo test -p argus-security -- --nocapture
cargo test -p argus-cli test_secured_command_denied_for_viewer
```

## Architecture

### Core Data Flow

```
User Message → Intent Classification → Task Decomposition → Scheduler → Worker Pool → Quality Check → Git Commit
                    ↑                        ↑                                              ↓
                EventBus ← ← ← ← ← ← ← ← ← ← Events ← ← ← ← ← ← ← ← ← ← ← ← ← ← ← ← ← ← ←
```

### Key Components

**argus-core**: Central orchestration engine
- `brain/` — Intent classification, task decomposition, decision engine
- `task_scheduler.rs` — Priority queue with DAG dependency resolution
- `event_bus.rs` — Async pub/sub for cross-module communication
- `types.rs` — Shared domain types (`Task`, `TaskState`, `ArgusError`, etc.)

**argus-llm**: Multi-provider LLM routing
- `router.rs` — Selects best provider based on model class, budget, health
- `providers/` — OpenAI, Anthropic, Ollama, etc. implementations
- `budget.rs` — Token/cost tracking per project/provider
- `key_rotation.rs` — Round-robin API key management with cooldown

**argus-memory**: Four-layer persistent memory
- `embedding.rs` — BGE-M3 via fastembed (1024-dim, local, no API)
- `store.rs` — PostgreSQL + pgvector for semantic search
- `working.rs` — In-memory DashMap per-task scratchpad
- `lifecycle.rs` — Decay, promotion, consolidation jobs

**argus-worker**: Subprocess worker management
- `pool.rs` — Spawns/manages worker processes
- `drivers/` — OpenCode, ClaudeCode, Aider, GenericCli drivers
- `sandbox/` — Directory and Docker isolation

**argus-security**: Production security
- `rbac.rs` — Owner/Admin/Developer/Viewer roles
- `command_guard.rs` — Blocks dangerous commands (`rm -rf /`, etc.)
- `audit.rs` — All security-relevant actions logged

### Event-Driven Communication

All components communicate through `EventBus` with typed events in `argus-core/src/events.rs`. State changes emit events; other modules subscribe and react. This decouples the brain, scheduler, workers, and social adapters.

### Configuration Hierarchy

1. `config/argus.toml` — Main config (database, workers, scheduler, etc.)
2. `config/providers.toml` — LLM provider credentials and model mappings
3. `.env` — Secrets and overrides (ARGUS_DATABASE_URL, API keys)

Environment variables override file config. Use `ArgusConfig::load_auto()` to merge sources.

### Task Lifecycle

1. **Pending** → Created, waiting for dependencies
2. **Ready** → Dependencies satisfied, queued for execution
3. **Running** → Assigned to worker
4. **Completed** / **Failed** → Final state, may trigger retry

States defined in `argus-core/src/task_types.rs`. State transitions emit `TaskStateChanged` events.

### Worker Types

- `OpenCode` — OpenCode CLI driver
- `ClaudeCode` — Claude Code CLI driver
- `Aider` — Aider driver
- `GenericCli` — Arbitrary command execution

Workers are spawned on-demand, recycled when idle, and killed on resource limits.

## Database

PostgreSQL with pgvector extension. Migrations in `migrations/` numbered 0001-0021. Key tables:
- `projects`, `milestones`, `tasks` — Project hierarchy
- `memories` — Vector-embedded memories with importance/decay
- `llm_usage` — Token tracking per provider/project
- `audit_log` — Security audit trail

## Code Patterns

### Adding a new LLM provider

1. Create `crates/argus-llm/src/providers/your_provider.rs`
2. Implement `LlmProvider` trait
3. Register in `router.rs` with priority and model classes
4. Add credentials to `config/providers.toml`

### Adding a new social adapter

1. Create `crates/argus-social/src/adapters/your_platform.rs`
2. Implement async `send()` and `receive()` patterns
3. Register in `argus-cli/src/main.rs` startup wiring

### Adding a new event type

1. Add variant to `ArgusEvent` enum in `argus-core/src/events.rs`
2. Emit from source component via `event_bus.publish()`
3. Subscribe in target component via `event_bus.subscribe_filtered()`