# Architecture

ARGUS is organized as eight layers, from durable infrastructure at the
bottom to the human/portfolio boundary at the top. Each layer only depends on
the layers below it. This document maps the model to what exists in the
repository today; it is a planning and orientation document, not API
reference — see `crates/*/src` and [DATABASE_SCHEMA.md](DATABASE_SCHEMA.md)
for that.

Status tags follow [MASTER_ROADMAP.md](MASTER_ROADMAP.md)'s legend:
**Implemented**, **Planned**, **Deferred**. Where a layer is partially built,
the tag is **Implemented** with the gap stated explicitly — never rounded up.

## The Eight Layers

```
8. Human / Portfolio          ← a person, across all projects
7. Chief / Manager / Planner  ← decides what to do
6. Project Intelligence       ← understands one project's state
5. Scheduling / Policy        ← decides when/who/under what budget
4. Agents                     ← named roles doing work (Planner, Developer, ...)
3. Agent Runtime               ← spawns and drives a provider process
2. Execution Isolation        ← where work physically happens, safely
1. Durable Infrastructure     ← the facts nothing above can contradict
```

### 1. Durable Infrastructure — **Implemented**

The facts every other layer must agree with: PostgreSQL (tasks, workspaces,
spend ledger, audit log), the Git repositories themselves, and persisted
artifacts. This is what Architecture Principle 5 ("Project State / Git / DB
/ Artifacts are source of truth") refers to concretely.

- `argus-core::task_repository`, `workspace_repository`, `spend_repository` —
  Postgres-backed, crash-recoverable state for tasks, workspaces, and spend.
- `migrations/` — schema, numbered and forward-only.
- Git repositories on disk (canonical, per project) — see Layer 2 for how
  they're protected from direct Agent access.

### 2. Execution Isolation — **Implemented**

Where an Agent is actually allowed to touch files, and the guarantees that
make it safe to let it. This is Architecture Principle 11 ("canonical
repository is never the Agent workspace") made concrete.

- `argus-core::workspace_manager` — path safety (deterministic worktree
  paths, symlink-escape checks), the `WorkspaceGuard` trait.
- `argus-git::worktree_guard::GitWorkspaceGuard` — real Git worktree
  creation and verification (ownership, branch, base revision, HEAD
  resolvability) backed by `argus-git::repo::RepoManager`.
- `argus-core::workspace_repository` — the persistent record: one workspace
  per execution, idempotent, survives restart, retained on failure.
- Guarantee: an Agent's cwd is always a verified worktree; a workspace that
  fails verification is never silently recreated (it becomes
  `recovery_needed` and a human decides).

### 3. Agent Runtime — **Implemented (single provider)**

Spawns and drives whatever process actually does the work, and normalizes
its result. This is the layer Architecture Principles 1, 2, and 7 (Agent
≠ Model, Role ≠ Provider, CLIs are runtime providers) describe — currently
with one provider wired into production.

- `argus-worker::agent` — the `AgentDriver` trait and drivers for
  `ClaudeCodeDriver`, `CodexDriver`, `OpenCodeDriver`, `KimiDriver`
  (occupying the `Aider` slot — see `driver_for`'s doc comment), plus
  `StubAgentDriver` for testing.
- `argus-worker::agent_executor::AgentTaskExecutor` — bridges a `Task` +
  `WorkspaceContext` to a driver invocation and back to a normalized result.
- The daemon selects Codex (default) or Claude Code at startup using
  `execution.default_worker` / `ARGUS_DEFAULT_WORKER`. V0.42/V0.44 provider
  selection is not yet connected to per-task dispatch. OpenCode/Kimi drivers
  exist but are not selectable through this daemon configuration interface.
  See [DEFERRED_IMPROVEMENTS.md](DEFERRED_IMPROVEMENTS.md) for validation boundaries.

### 4. Agents — **Implemented (one role)**

Named roles that do work — not the same thing as the provider running them
(Principle 2). Today there is one role in production.

- **Developer** (implicit role, not yet named as such in code): the role
  `AgentTaskExecutor` fills — takes a task, produces a verified change in an
  isolated worktree.
- **Planner**, **Reviewer**, and other roles: **Planned** — see V0.43
  (Planner Harness / Protocol) and V0.6 (Multi-Agent Team). Principle 10 (Planner and
  Developer permissions must be separate) is a constraint on this layer for
  when those roles exist, not something enforced yet because there is only
  one role.

### 5. Scheduling / Policy — **Implemented (single-project, sequential)**

Decides when a task runs, whether it's allowed to spend money right now, and
what happens when it can't run yet.

- `argus-core::task_scheduler::TaskScheduler` — dependency-resolved priority
  queue, crash-safe state machine, idempotent claim per execution.
- `argus-core::execution_loop::ExecutionLoop` — the driver: dequeue → prepare
  workspace → budget preflight → verify → execute → finalize.
- `argus-core::budget_service::BudgetService` — pre-flight budget
  enforcement (Principle 9); fail-closed on ledger failure
  (`argus-core::accounting_state`).
- Deferred-retry scheduling (`Task::retry_at`/`defer_count`,
  `TaskScheduler::defer_task`) — an admission-gate block defers with
  category-appropriate backoff instead of busy-looping, and survives a
  restart.
- **Explicitly single-project, sequential** — `max_pool_size > 1` (parallel
  workers within a project) is a later stage beyond V0.47; running many
  projects at once is V0.7.
  Neither exists yet.

### 6. Project Intelligence — **Implemented (foundation)**

- V0.39 structured project charter, milestones, WorkItems, decisions, risks,
  blockers and snapshots persist independently of conversations.
- V0.40 roadmap/progress aggregates verified evidence deterministically.
- V0.41 role/purpose-specific Context Compiler records provenance and hashes.
- V0.45 evaluation records independently checked Git/test evidence and
  promotes WorkItems; operator acceptance is separate.
- Git-history ingestion and semantic retrieval remain future work.

### 7. Chief / Manager / Planner — **Implemented (protocol), Planned (autonomy)**

- V0.43 provider-neutral requests/results, validation, proposal diff,
  explicit approval and transactional/idempotent application.
- Manual/Web Planner export/import is implemented. The automatic planner
  adapter, Manager-to-Task loop and Chief allocation remain planned.
- V0.44 selects concrete providers/auth/quota under policy; production
  runtime dispatch still uses one startup-selected executor.

### 8. Human / Portfolio — **Planned (partial surfaces exist)**

Where a person interacts with ARGUS across all of its projects, not just one.

- Social/chat surfaces (`argus-social`: Telegram, Feishu, Google Chat, QQ,
  WhatsApp, web dashboard, others in varying stages of completeness) exist
  today as *per-conversation* control channels, not a portfolio view.
- A genuine Portfolio Control Plane (cross-project status/budget/risk in one
  place): **Planned**, V0.47.

## Cross-Cutting Capabilities

These aren't a layer — each spans several of the layers above. All are
**Planned** unless marked otherwise; where something partial already exists,
that's stated explicitly rather than rounded up to Implemented.

| Capability | Status | Note |
|---|---|---|
| Context Compiler | Implemented | V0.41 deterministic role/purpose packets with provenance, hashes and budget truncation. |
| Execution Harness | Implemented (single provider) | Today's `ExecutionLoop`/`AgentTaskExecutor` (Layers 3/5) is the first instance; generalizing it into a named, provider-agnostic harness is V0.43 — see [MASTER_ROADMAP.md](MASTER_ROADMAP.md#execution-harness). |
| Evaluation / Evidence | Implemented (partial) | The per-task Quality Gate (`argus-core::quality`) verifies a task's output before completion (ADR #9); V0.45 records WorkItem verification and separate operator acceptance; additional evaluator kinds remain planned — see [MASTER_ROADMAP.md](MASTER_ROADMAP.md#evaluation-harness). |
| Plan Versioning | Implemented | V0.43 requests, proposals, state/roadmap revisions, approval and stale checks. |
| Environment / Resource Management | Planned | No version slot yet — provisioning, port/database allocation, and a tracked `ProjectResource` registry. See [MASTER_ROADMAP.md](MASTER_ROADMAP.md#environment--resource-layer). |
| Integration / Merge | Planned | Deliberately not automatic (ADR: "No automatic merge from task worktree"). V0.38 Phase 4 (Implemented) produces a verified checkpoint; merging it into a canonical branch is a separate, later capability. |
| Approval Gates | Implemented (partial) | Explicit Planner approval/application and WorkItem acceptance, protected by HTTP/CLI RBAC; broader autonomous-action policy remains planned. |
| Capability / Permission | Implemented | `argus-security::rbac` — roles (Owner/Admin/Developer/Viewer) and permissions (ReadProject, WriteCode, ExecuteCommand, ManageUsers, ManageSecrets, ConfigureSystem, ReadAudit), enforced for CLI and HTTP write operations; anonymous GET uses Viewer permissions. |
| Secrets Management | Implemented (partial) | `argus-security::key_vault` stores/retrieves secrets; V0.44 adds locator-only `SecretReference`; a complete scoped secrets platform remains Planned — see [MASTER_ROADMAP.md](MASTER_ROADMAP.md#secrets-management). |
| Provider Capability Registry | Implemented | V0.44 capability, model, auth, quota and policy-based selection; dispatch is still single-provider. |
| Provenance | Planned | `argus-security::audit` records security-relevant actions today; WorkItem→Task→Execution→Artifact→Commit and evaluation references exist;
  a complete portfolio-wide provenance view remains future work — see [MASTER_ROADMAP.md](MASTER_ROADMAP.md#provenance). |
| Planner Evaluation | Planned | Judging plan quality itself, not just task output — depends on the Planner Protocol existing first. |
| Human Inbox | Planned | `argus-social`'s notification engine delivers messages per-conversation; a unified cross-project "Needs Your Attention" inbox is a different, not-yet-built thing. |
| Notifications | Implemented (partial) | Per-platform notification delivery exists (`argus-social`); portfolio-level notification policy is Planned alongside V0.47. |
| Reconciliation | Implemented (narrow) / Planned (general) | Crash reconciliation between "Agent finished" and "checkpoint committed" is Implemented (V0.38 Phase 4: `argus_core::checkpoint`, idempotent across a commit-but-no-artifact or artifact-but-no-completion crash); the general desired-vs-observed-state model for arbitrary resources (containers, processes, ports) is Planned — see [MASTER_ROADMAP.md](MASTER_ROADMAP.md#environment--resource-layer). |
| Backup / Disaster Recovery | Implemented (partial) | A backup schema/migration exists; task/scheduler crash recovery is implemented and tested (V0.35+). Full disaster-recovery procedure is Planned — see [RUNBOOK.md](RUNBOOK.md) for current operational guidance. |

## See also

- [MASTER_ROADMAP.md](MASTER_ROADMAP.md) — which version introduces each Planned item above.
- [PROJECT_STATE.md](PROJECT_STATE.md) — current work in flight.
- [adr/README.md](adr/README.md) — the decisions this architecture encodes.
- [DATABASE_SCHEMA.md](DATABASE_SCHEMA.md), [TASK_SCHEDULER.md](TASK_SCHEDULER.md), [WORKER_MANAGER.md](WORKER_MANAGER.md), [SECURITY.md](SECURITY.md) — implementation-level detail for Layers 1–5.
