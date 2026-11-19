# Project State

Current implementation status only. For where this is headed, see
[MASTER_ROADMAP.md](MASTER_ROADMAP.md); for why things are built the way they
are, see [adr/README.md](adr/README.md). This file is kept short on purpose —
it answers "what is true right now," not "how we got here." Update it in the
same change that changes what's true; don't let it drift.

## Current

**V0.45 — Evaluation Harness foundation — functionally completed.**
V0.38–V0.44 remain completed within their documented scopes. V0.44 routing
still does not dispatch providers. The daemon selects one executor at startup:
Codex by default, or Claude Code via `ARGUS_DEFAULT_WORKER=claude_code`.

## V0.45 Evaluation Harness (foundation)

Implemented:

- Required artifact/provenance/test-evidence and real Git checkpoint evaluators;
  pluggable additional required evaluators. Missing/skipped evidence never passes.
- Durable reports with input hash, version, check results and artifact references
  (migration 0035); transactionally promote verified WorkItems and record activity
  and project revision. Additional linked tasks invalidate prior completion.
- Production ExecutionLoop hook after checkpoint completion. Explicit evaluation
  can retry failed verification/persistence; task artifacts remain durable.
- Separate, idempotent operator acceptance, requiring a current passing report,
  unchanged evidence and rechecking. Cross-project references are rejected.
- CLI: `project link-task|evaluate|evaluations|accept`; HTTP task linking,
  evaluation, report listing and acceptance endpoints.
- HTTP write authentication, existing RBAC permissions, and persisted
  authorization audit before writes. Plan approvals/application and acceptance
  require admin/owner permissions; no role supplied by a caller is trusted.
- Operational surfaces corrected: actual redacted read-only configuration;
  unsupported updates return 501; logs read persisted audit actions. Docker
  sandbox requests now fail rather than silently running on the host.
- CI adds frontend build/lint, API inventory/generated-client drift checks and
  tracked secret scan; Postgres E2E suites run serially.
- Ten real-Postgres/Git/process E2E regression cases, plus HTTP permission,
  audit-unavailable, CLI permission, configuration redaction and sandbox tests.
  No real model calls are needed.

Limits: v1 verifies persisted test and Git evidence, not prose DoD semantics.
Semantic review, more evaluator kinds, automatic planning, multi-provider
execution and Docker isolation are future work. Historical bootstrap records
retain their trusted note-based verification. See
[EVALUATION_HARNESS.md](EVALUATION_HARNESS.md) for the exact policy and commands.

## V0.44 Provider / Auth / Subscription / Quota (closed)

Maps V0.42 capability requirements onto a concrete execution target:

```text
Role -> Capability Requirement -> Provider Capability -> Auth Profile
  -> Quota / Availability -> Cost / Policy -> Concrete Execution Target
```

`Role != Model != Provider != Auth != Subscription != API billing`.

Implemented and tested against real Postgres + deterministic fakes:

- **Types** (`argus_core::provider`) — `RuntimeProvider`, `ProviderDescriptor`
  with runtime capabilities, `ModelDescriptor` with a **capability tier** and
  an effective-capability intersection (model ∩ runtime), `AuthProfile`
  (`SubscriptionLogin`/`ApiKey`/`OAuth`/`CliCredential`/…), `SecretReference`
  (a locator only), `QuotaSnapshot` (kind + availability + source +
  confidence), `ProviderHealth`, `ExecutionTarget`, `BillingMode`,
  `SelectionPolicy`, `FallbackPolicy`, project policy, and persisted
  selection/attempt records.
- **Registry** — built-in provider/model defaults with config overrides;
  models are data, never hardcoded permanent vendor catalogs.
- **Router** (`provider-router-v1`) — filter by capability → tier → runtime →
  auth → known quota, then subscription-first/cost/health policy with a stable
  tie-break. **Unknown quota is neither zero nor unlimited**; only a known
  exhaustion excludes a target. Provider failover never changes the required
  tier.
- **Budget integration** — every metered target (`ApiMetered` **and**
  `Credits`) passes the V0.37 `BudgetService` preflight; `FallbackPolicy::Ask`
  returns `NeedsApproval` before any spend; `Never` does not fall back to API.
- **Policy resolution** (`ProjectProviderPolicy::resolve`) — the persisted
  project policy is authoritative and applied inside `ProviderService::route`,
  so no entrypoint can skip it. A caller may only **narrow** it: denials are
  unioned, allow-lists intersected, `max_tier` takes the stricter value, and
  `FallbackPolicy` takes the most restrictive. `allowed_providers` is an
  `Option<Vec<_>>` so "permit nothing" (`Some([])`) can never collapse into
  "unrestricted" (`None`) and silently widen. Model-level allow/deny is
  supported without hardcoding any vendor's model ids into the router.
- **Discovery** — allowlisted CLI binaries only (`claude`, `codex`,
  `opencode`), resolved by scanning `PATH` and executed without a shell.
  Installed + `--version`, plus each CLI's **own official non-interactive
  status command** for auth: `claude auth status --json` (machine-readable →
  `High` confidence) and `codex login status` (phrase parse → `Medium`).
  OpenCode has no such command, so its status stays `Unknown` — it is never
  guessed. A parse failure degrades to `Unknown`, never `Unavailable` and
  never a panic. No browser cookies, no session/token files, no login, no
  model call. Parser versions are pinned (`claude-auth-status-parser-v1`,
  `codex-login-status-parser-v1`).
- **API** — `GET /api/providers`, `GET /api/providers/status`,
  `GET /api/providers/:id/models`, `GET/POST /api/auth-profiles`,
  `GET /api/auth-profiles/:id/quota` (+`/refresh`),
  `POST /api/projects/:id/provider-routing/select`, `GET/PUT .../policy`,
  `GET .../selections`. **CLI** — `argus provider list|status|discover`,
  `argus auth list|status|register`, `argus quota show|refresh`,
  `argus route <routing-decision>`.
- **Tests** — `provider_routing_e2e.rs` (13 cases: subscription-first,
  capability vs subscription, budget-blocked API fallback, Ask fallback,
  unknown quota, NeedsLogin, project policy isolation, caller-cannot-widen,
  project fallback policy wins, denied model, secret redaction, restart
  persistence, unavailable-runtime failover, failover-keeps-tier) plus 32 unit
  tests in `argus_core::provider`. The V0.44 suite holds a lock around the
  global `auth_profiles` table so it is safe under parallel `cargo test`.
  Real model cost: `$0`.

> **Running the Postgres E2E suites:** they share one database, and some
> V0.38-era suites assert on globally-scanned state (`restore()` counts active
> tasks across all projects), so run them with `-- --ignored --test-threads=1`.
> See DEF-020. The V0.44 provider suite is the exception — it is parallel-safe.

**Closed during V0.44 security closure:** DEF-010 (the two
`create_task` idempotency integration tests) is fixed and no longer deferred —
both now build their `TaskScheduler` over a real `PostgresTaskRepository`,
matching production wiring, and pass against Postgres.

**Provider credentials:** `config/providers.toml` now declares
`api_keys = ["${DASHSCOPE_API_KEY}"]` instead of a literal, resolved at startup
by `apply_provider_env_overrides`. `tracked_configs_hold_no_literal_credentials`
guards every tracked config file, and
`provider_env_indirection_resolves_without_leaking` proves placeholder → env →
provider resolution without the value reaching any loggable surface. See
[LLM_PROVIDER.md](LLM_PROVIDER.md) §2.

## V0.43 Planner Harness / Protocol (completed)

A provider-neutral planning protocol. A Planner (a web model, a future API
provider, or the test fake) may only *propose* structured changes; ARGUS
binds the request to a context hash, validates, diffs, and applies only on
explicit approval — transactionally and idempotently.

```text
Project State -> Context Compiler -> PlanningRequest -> Planner
  -> PlanningResult -> Validate -> Diff -> Proposal -> Approve -> Apply -> Project State
```

Implemented and tested against real Postgres:

- **Types** (`argus_core::planner`) — `PlanningRequest` (bound to
  `context_hash`/`compiler_version`, revisions, objective, constraints,
  requested outputs), `PlanningResult`, `PlanProposal` with typed operations
  (`create/update/transition/supersede/defer/cancel/no_op`), decision/risk/
  blocker/deferred proposals, assumptions, clarifications, provider-neutral
  `PlannerMetadata`.
- **Protocol** `planner-protocol-v1` with canonical `request_hash` /
  `result_hash`.
- **Validator** — schema/version, project scope, entity existence,
  cross-project rejection, illegal transitions, **no fabricated
  VerifiedComplete/Accepted**, dependency validity (no self/foreign deps),
  duplicate external keys, required DoD on create, and context binding.
  Untrusted JSON uses `deny_unknown_fields` (no arbitrary commands/SQL/paths).
- **PlanDiff** — deterministic, stably ordered create/update/transition
  summary.
- **Lifecycle** Draft/Validated/Rejected/Approved/Applied/Stale/Failed with an
  append-only `proposal_lifecycle_events` audit trail; explicit
  approve/reject; no auto-approval.
- **Transactional apply** — a single PostgreSQL transaction; all-or-nothing;
  idempotent (a second apply returns `AlreadyApplied`); minimal per-project
  `state_revision`/`roadmap_revision` for optimistic concurrency (stale
  proposals never mutate state).
- **PlannerAdapter** trait + deterministic `FakePlannerAdapter`; no provider,
  auth, or quota code.
- **Manual Web Planner** — `render_planner_packet` (Markdown) + CLI
  export/import; imported results are untrusted and validated.
- **API** — `POST /api/projects/:id/planning/requests`,
  `GET /api/planning/requests/:id/packet`, `POST /api/planning/results/import`,
  `GET /api/planning/proposals/:id[/diff]`,
  `POST /api/planning/proposals/:id/{approve,reject,apply}`. **CLI** —
  `argus project plan export|import|show|diff|approve|reject|apply`.
- **Tests** — `planner_harness_e2e.rs` (fake-planner full chain, fabricated
  verification rejected, cross-project injection, stale no-mutation, double
  apply, duplicate import + tamper, manual packet, restart persistence,
  transactional rollback, strict parsing, determinism, project isolation)
  plus unit tests. Real Agent cost: `$0`.

## V0.42 Task Intelligence / Model Routing (completed)

Classify a WorkItem into an explainable `TaskProfile` and route it to a
*minimum capability tier* — `use the cheapest sufficient intelligence;
escalate only when evidence justifies it`. A tier is never a provider or
model; V0.44 maps tiers onto providers/subscriptions/quotas.

Implemented and tested against real Postgres:

- **Types** (`argus_core::intelligence`) — `TaskProfile` (normalized
  complexity/risk/ambiguity/reasoning/scope/verification/context/reversibility
  signals, `TaskKind`, `secondary_flags`, capabilities, tier, confidence,
  reasons, `input_hash`) and `RoutingDecision` (required/preferred tier,
  capabilities, reasoning effort, context/verification requirement, retry +
  escalation policy, `cost_preference`, reasons, `router_version`).
- **Deterministic classifier** (`task-intelligence-v1`) — rule-based over
  structured metadata; never prompt length; every non-trivial tier carries
  reasons. Risk floor and verification floor are independent of the tier.
- **Model tiers** T0–T3 with capability requirements; `RoutingPolicy` is
  role- and purpose-sensitive (e.g. a risky Reviewer/Planning pass raises the
  floor).
- **Failure classification + escalation** — `FailureClass`,
  `EscalationAction`, `EscalationPolicy` (`escalation-v1`). Only
  reasoning/implementation/verification (and context) failures may raise the
  tier; environment/dependency/tool/permission/budget/quota failures never do;
  context-insufficient recompiles context first; T3 is the ceiling.
- **Persistence** — `task_profiles`, `routing_decisions`, `routing_outcomes`,
  `escalation_decisions` (migration 0031), idempotent by `input_hash`.
- **API** — `POST /api/projects/:id/intelligence/classify`,
  `POST /api/routing/:id/outcome`, `POST /api/routing/:id/failure`,
  `GET /api/projects/:id/routing/stats`. **CLI** — `argus project classify …`.
- **Tests** — `task_intelligence_e2e.rs` (ARGUS self-classification, four task
  shapes, prompt-length independence, failure escalation matrix, outcomes +
  statistics, determinism/restart, project isolation) plus unit tests. Real
  Agent cost: `$0`.

## V0.41 Context Compiler (completed)

Compile the structured Project State into a minimal, relevant, traceable
context packet for a specific role and purpose, with no model call:
`Project Memory → Context Compiler → Role-specific Context Packet →
Planner / Agent`.

Implemented and tested against real Postgres:

- **Types** (`argus_core::context`) — `ContextRequest` (role, purpose,
  optional WorkItem/Task/Execution, item/byte/token budgets, requested
  sections) and an immutable `ContextPacket` with `context_hash`,
  `compiler_version` (`context-compiler-v1`), and `source_refs`.
- **Role policies** — Chief (project-level summary only), ProjectManager
  (charter/roadmap/active+blocked work/risks/decisions/deferred/artifacts/
  activity), Developer (its WorkItem, DoD, dependencies, evidence, relevant
  decisions/constraints — not the whole roadmap), Tester, Reviewer,
  Researcher, Integrator. Purpose (planning/execution/review/debugging/
  research/integration/status-review) further biases selection.
- **Deterministic selection** — structural relevance only (same project /
  milestone / phase / work item / evidence / dependency), no embeddings or
  RAG. Provenance is recorded for every fact.
- **Budget + truncation** — deterministic priority-based truncation
  (deferred → activity → milestone/phase → artifacts → dependencies →
  decisions/risks → blockers → constraints; current WorkItem, charter, and
  project identity protected), with `truncated`, `omitted_sections`, and
  per-section `omitted_item_counts`.
- **Renderers** — `render_json` and a template `render_markdown` (no
  summarization).
- **API** — `POST /api/projects/:id/context/compile[?format=markdown]`.
- **CLI** — `argus project context <project> --role … --purpose … --format …`.
- **Tests** — `context_compiler_e2e.rs` (ARGUS self-context, developer
  isolation, project-manager breadth, reviewer artifact/evidence, superseded
  decisions, budget truncation, determinism/restart, project isolation,
  provenance, Markdown-is-not-truth) plus unit tests. Real Agent cost: `$0`.

## V0.40 Roadmap & Progress Engine (completed)

Deterministic, evidence-driven roadmap and progress on top of the V0.39
structured state. `Facts -> Roadmap Structure -> Verification State ->
Deterministic Progress`. No LLM computes progress; nothing auto-mutates state
from a computed number.

Implemented and tested against real Postgres:

- **Roadmap hierarchy** — `Project -> Milestone -> Phase -> WorkItem` (fixed
  four levels, no arbitrary tree). New `phases` table plus
  `external_key`/`weight` on milestones and work items; `work_items.phase_id`.
- **Roadmap dependencies** — `work_item_dependencies` (A before B), kept
  strictly separate from the runtime TaskScheduler DAG.
- **Deterministic Progress Engine** (`argus_core::project_state::progress`) —
  weighted `ProgressSummary` (verified/accepted/active/blocked/deferred/
  cancelled, `completion_ratio`, `ProgressConfidence`). Only
  `VerifiedComplete`/`Accepted` count; `ReportedComplete` never does;
  Deferred/Cancelled are excluded from the denominator but reported.
  Phase/Milestone/Project aggregate upward; `current_milestone`,
  `current_phase`, and `next_candidates` are deterministic.
- **Evidence gate** — a WorkItem with a `definition_of_done` cannot be marked
  `VerifiedComplete`/`Accepted` without at least one evidence row.
- **Progress Diff** — `ProgressDiff::between(before, after)` plus
  `record_progress_snapshot`/`get_progress_diff`, so "what changed" is a
  deterministic fact, not a model summary.
- **API** — `GET /api/projects/:id/roadmap`, `GET .../progress`,
  `GET .../progress/diff`, `POST .../progress/record`; status
  rendering now includes overall verified progress.
- **CLI** — `argus project progress <project>`.
- **Self-roadmap** — ARGUS is bootstrapped to V0.38/V0.39 Completed, V0.40
  Active, V0.41/V0.42 Planned, with the V0.40 phase and work items and
  DEF-010/011/012 deferred. `MASTER_ROADMAP.md` remains human documentation,
  never runtime truth.
- **Tests** — `roadmap_progress_e2e.rs` covers evidence-driven completion,
  Deferred denominator, parent aggregation, weights, dependencies, progress
  diff, per-project isolation, determinism, and restart persistence. Real
  Agent cost: `$0`.

## V0.39 Foundation (completed)

ARGUS's own durable, structured understanding of a project — independent of
any conversation or model context window. The rule is
`Conversation = Evidence, Project State = Source of Truth`. No LLM generates
project state; the database does, and snapshots are derived deterministically.

Implemented and tested against real Postgres:

- **Structured state model** (`argus_core::project_state`) — `Project`,
  `ProjectCharter`, `Milestone`, `WorkItem`, `Decision`, `Risk`, `Blocker`,
  `DeferredImprovement`, `ProjectActivity`, and a machine-readable
  `ProjectSnapshot`. Each status is a finite enum; `ReportedComplete` is
  never `VerifiedComplete`.
- **Persistence** — migration `0028_project_state.sql` adds
  `project_charters`, `work_items`, `work_item_tasks`, `work_item_evidence`,
  `project_decisions`, `project_risks`, `project_blockers`,
  `deferred_improvements`, `project_activities`, and extends `projects`
  (`slug`, `status`) and `milestones` (`parent_id`,
  `definition_of_done`). One `ProjectStateRepository`, not one per entity.
- **`ProjectStateService`** — create/get/snapshot/status plus per-entity
  CRUD and a deterministic `ProjectStatusView` renderer. WorkItems carry
  0..N Tasks and completion evidence (artifact/task/execution), the
  substrate for a later Evaluation Harness.
- **API** — read-mostly: `GET /api/projects/:id/state`,
  `GET /api/projects/:id/snapshot`, plus create endpoints for milestones,
  work items, decisions, risks, blockers, and improvements.
- **Self-bootstrap** — `argus project bootstrap` records ARGUS's own state
  (V0.38 Completed, V0.39 Active, `Project State Foundation` active work
  item, DEF-010/DEF-011 deferred) and `argus project status argus` renders
  it with no model involved.
- **Tests** — `project_state_e2e.rs` covers CRUD/snapshot, restart
  persistence, multi-project isolation, DB-beats-Markdown source of truth,
  ARGUS self-bootstrap, and the artifact→activity hook. Real Agent cost: `$0`.

Git-history ingestion remains deferred. The Context Compiler and planner-facing
packets subsequently landed in V0.41/V0.43, as described above.

## Completed (V0.38 — Reliable Agent Delivery Runtime)

These are implemented, tested, and running against real Postgres and (for
Agent execution) a real Claude Code CLI — not just designed:

- **Recovery** — crash-safe task state restore on daemon restart; interrupted
  tasks resume without duplicate execution.
- **Spend / Budget Safety** — persistent spend ledger, daily/per-task budget
  enforcement, pre-flight checks before any paid Agent execution.
- **Accounting Fail-Closed** — a spend-ledger write failure marks the system
  `Degraded` and blocks further paid execution until explicit operator
  recovery; it never silently continues.
- **Git Worktree Foundation** — real Git worktree creation/verification
  (ownership, ownership-of-branch, path safety, base revision) via a
  `WorkspaceGuard` abstraction, not ad hoc shell calls.
- **Workspace Persistence** — workspace records (`task_workspaces`) survive
  restart and are idempotent per execution.
- **Agent Workspace Execution** — the production `ExecutionLoop` runs each
  Agent in its own verified worktree; the canonical repository is never the
  Agent's cwd and is never modified directly. Verified against a real
  `claude` CLI end-to-end.
- **Runtime Stabilization** — no busy/hot-loop on a stuck admission gate
  (budget exhausted, accounting degraded, workspace not ready); deferred
  retries survive a daemon restart.
- **Git Checkpoint & Task Artifact (Phase 4)** — a quality-passing task's
  worktree changes only reach `Completed` after ARGUS (never the Agent)
  stages and commits them with an explicit identity, and persists a
  `task_artifacts` row linking project/task/execution/workspace to the
  commit SHA, files changed, and Quality Gate evidence
  (`argus_core::checkpoint`, `argus_core::artifact_repository`,
  `argus_git::checkpoint`). No `--allow-empty`: quality passing with zero
  real changes is a distinct failure, not a silent completion. Idempotent
  across all three crash windows (commit-but-no-artifact,
  artifact-persistence-failure, and a full finalize replay) — verified by
  unit tests against real fakes; the successful/quality-failure/no-changes
  paths are additionally verified end-to-end against real Postgres and a
  real `git` binary.

## Next

V0.46 — Environment / Resource State, followed by V0.47 — Portfolio Control
Plane. Live Planner-to-Task orchestration and multi-provider dispatch still
need explicit roadmap placement before implementation; V0.5 composes these
foundations into an autonomous Project Manager.

## Current Rule

Do **not** start Parallel Workers (`max_pool_size > 1`) yet — see
[MASTER_ROADMAP.md's Non-Goals](MASTER_ROADMAP.md#non-goals-current-phase).
Workspace isolation, Task Artifacts, and basic Evaluation land first.

## See also

- [MASTER_ROADMAP.md](MASTER_ROADMAP.md) — V0.38 through V1.0.
- [DEFERRED_IMPROVEMENTS.md](DEFERRED_IMPROVEMENTS.md) — known gaps that don't block the current phase.
- [ARCHITECTURE.md](ARCHITECTURE.md) — the layered model this work fits into (currently: Durable Infrastructure, Execution Isolation, Agent Runtime, Agents).
