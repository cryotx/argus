# Deferred Improvements

Known, non-blocking gaps — things we know about and have deliberately chosen
not to fix yet, as opposed to things we haven't discovered. Deferred means
"not on the critical path right now," not "unimportant" and not "forgotten."
When one of these gets picked up, move it out of this file and into the
relevant ADR/roadmap entry rather than just deleting the line.

Each entry records: ID, Title, Status, Priority, Source, Why Deferred, and
Required Before (what has to be true for it to stop being deferred).

## Runtime & Scheduling

### DEF-001 — Configurable retry backoff

- **Status:** Deferred
- **Priority:** Low
- **Source:** V0.38.1 runtime stabilization work
- **Why Deferred:** The admission-gate backoff constants (2s/5s/60s/15min/
  next-midnight) are fixed in code, not configuration. The values were
  chosen conservatively and no operator need for tuning has shown up yet.
- **Required Before:** A second real deployment profile needs different
  backoff values than the current defaults.

### DEF-002 — Dedicated accounting-degraded + workspace integration test

- **Status:** Deferred
- **Priority:** Medium
- **Source:** V0.37.2 accounting state work
- **Why Deferred:** The "accounting degraded defers a task without
  disturbing its workspace" property is currently verified by two separate
  tests (budget-blocked workspace reuse, and accounting-degraded blocking)
  whose combination is argued from a shared code path rather than exercised
  directly.
- **Required Before:** That shared code path is refactored — a dedicated
  test should land ahead of or alongside that refactor, not after.

## Agent Runtime Providers

### DEF-003 — Codex CLI real-workspace validation

- **Status:** Deferred
- **Priority:** Medium
- **Source:** V0.38 worktree isolation work
- **Why Deferred:** Workspace isolation has a real end-to-end smoke test
  against Claude Code only. Codex CLI is wired as a driver but has not been
  run through the same real-worktree smoke test. Claude Code is the only
  provider exercised in production today (`main.rs` hardcodes
  `WorkerType::ClaudeCode`), so validating the others doesn't block current
  work.
- **Required Before:** V0.43 (Planner/Runtime Providers) makes provider
  choice a real runtime decision rather than a hardcoded default.

### DEF-004 — OpenCode real-workspace validation

- **Status:** Deferred
- **Priority:** Medium
- **Source:** V0.38 worktree isolation work
- **Why Deferred:** Same gap as DEF-003: driver exists, no real-worktree
  smoke test yet.
- **Required Before:** Same as DEF-003 — V0.43.

### DEF-005 — Kimi real-workspace validation

- **Status:** Deferred
- **Priority:** Low
- **Source:** V0.38 worktree isolation work
- **Why Deferred:** Same gap as DEF-003/004, and additionally contingent on
  Kimi remaining the intended occupant of the `Aider` driver slot (see
  `argus-worker/src/agent.rs`'s `driver_for` comment) rather than a real
  Aider integration being built instead.
- **Required Before:** Same as DEF-003, plus confirming the `Aider` slot's
  intended provider.

## Operations

### DEF-006 — Smoke-test database cleanup

- **Status:** Deferred
- **Priority:** Low
- **Source:** Phase 3 worktree smoke test, repaired E2E suites
- **Why Deferred:** Real end-to-end smoke tests write real rows into
  whatever Postgres they're pointed at and don't clean them up afterward, on
  the theory that retained evidence is more valuable than a tidy database
  during active development.
- **Required Before:** These tests run against a shared or long-lived
  environment rather than disposable local/CI databases.

## Provider Abstraction

### DEF-007 — Provider capability normalization

- **Status:** Deferred
- **Priority:** Medium
- **Source:** Agent Runtime layer (drivers)
- **Why Deferred:** Different providers/drivers report different subsets of
  usage data (tokens, cost, session id) in different shapes; today each
  driver's parsing code absorbs this individually (e.g.
  `ClaudeCodeDriver::parse`). A normalized capability/result schema is the
  right fix, but belongs with the Provider Capability Registry (V0.43), not
  as a standalone refactor now.
- **Required Before:** V0.43 (Planner/Runtime Providers).

### DEF-008 — Advanced quota introspection

- **Status:** Deferred
- **Priority:** Low
- **Source:** BudgetService / spend ledger (V0.36–V0.37.2)
- **Why Deferred:** Budget enforcement today is ledger-based (realized
  spend against a configured limit); there is no `QuotaSnapshot` that
  cross-checks provider-reported quota, local usage estimates, and
  rate-limit observations against each other. Not needed while a single
  provider (Claude Code) and a single auth path are in production.
- **Required Before:** V0.44 (Authentication/Subscription/OAuth/API/Quota)
  introduces multiple auth profiles per provider, at which point
  best-effort provider-reported quota becomes load-bearing for routing.

### DEF-009 — Environment / resource management

- **Status:** Deferred
- **Priority:** Low
- **Source:** Workspace/worktree isolation work (V0.38)
- **Why Deferred:** ARGUS tracks Git worktrees as owned, persistent
  resources (`task_workspaces`), but nothing else an execution might need —
  ports, databases, containers — is tracked as an owned resource today;
  where those exist they're set up manually or by ad hoc agent commands.
  Not a problem while ARGUS runs one project sequentially against one
  configured database.
- **Required Before:** The Environment / Resource Layer (see
  [MASTER_ROADMAP.md](MASTER_ROADMAP.md#environment--resource-layer)) is
  designed and given a roadmap version.

## Operations (found during V0.38 Phase 4 regression testing)

### DEF-011 — Unknown Agent Cost Semantics

- **Status:** Partially addressed — Provider layer closed, legacy runtime
  spend semantics still deferred
- **Priority:** Medium
- **Closed by V0.44:** the provider/auth/quota layer distinguishes reported,
  inferred and unknown throughout — `QuotaSnapshot::remaining` is `Option<f64>`
  (never defaulted to `0`), `BillingMode::SubscriptionIncluded` is modelled as
  "included allowance" rather than free, and unknown cost is never asserted as
  `$0`. Routing decisions are made against unknown-safe values.
- **Still open:** the legacy V0.38 ledger. `spend_repository.rs` still binds
  `record.cost_usd.unwrap_or(0.0)`, so a driver that reported no cost writes a
  genuine-looking `0.0` into `llm_usage.cost_usd`. This entry keeps that half
  open.
- **Source:** `crates/argus-core/src/spend_repository.rs` (found during V0.38
  Phase 4 acceptance: a cost-less driver wrote `NULL` into the `NOT NULL`
  `llm_usage.cost_usd` column and permanently degraded accounting). The
  immediate crash was fixed by persisting `0.0` for "unknown"; the deeper
  modeling is still open.
- **Why Deferred:** `cost_usd = 0.0` is an honest sentinel for "the driver
  did not report a cost", but it is indistinguishable in the ledger from a
  genuine zero-cost execution. That is fine for budget enforcement (which
  keys off reported cost) but blocks any later per-provider quota/true-cost
  accounting that needs to distinguish "free" from "unknown". Changing it
  touches the frozen V0.38 execution/ledger path and existing rows.
- **Required Before:** True per-provider cost reporting: add a
  `cost_source`/`cost_known` column plus a policy for how unknown cost is
  aggregated and treated at admission time, and backfill existing rows.
- **Tracked in:** V0.39/V0.40 Project State as
  `deferred_improvements.external_key = 'DEF-011'`.

### DEF-012 — Embedded migration rebuild detection

- **Status:** Deferred
- **Priority:** Medium
- **Source:** `crates/argus-core`, `crates/argus-cli` (`sqlx::migrate!`),
  hit directly while adding migration `0029`/`0030`: a newly added migration
  file under `migrations/` is embedded at compile time, but cargo does not
  treat the directory as an input, so a cached binary can silently run with
  the old migration set and report "All migrations applied successfully"
  while skipping the new file.
- **Why Deferred:** The workaround is to touch a source file containing
  `sqlx::migrate!` (or clean the crate) before building. It does not affect
  correctness of a fresh build or CI, where the migration set is expanded
  from scratch; it is a local developer-experience hazard only.
- **Required Before:** Add a `build.rs` that emits
  `cargo:rerun-if-changed=migrations` (or equivalent) for each crate that
  embeds migrations. Not fixed this round.
- **Tracked in:** Project State as
  `deferred_improvements.external_key = 'DEF-012'`.

## Roadmap / Progress / Context (found during V0.40–V0.41)

### DEF-013 — `phases.weight` is stored but unused

- **Status:** Deferred
- **Priority:** Low
- **Source:** `crates/argus-core/src/project_state/progress.rs` — milestone
  progress aggregates all WorkItems by their own `weight`; `phases.weight` is
  persisted but never read by the engine.
- **Why Deferred:** A phase is currently organizational; weighting items is
  enough for the first cut. Changing phase semantics would change progress
  math and hashes, so it should be a deliberate, versioned change.
- **Required Before:** Decide whether a phase's weight should scale its
  children's contribution; if so, bump the Progress Engine's semantics and
  adjust tests.

### DEF-014 — `ProgressDiff` matches deferred/risk/blocker transitions by title

- **Status:** Deferred
- **Priority:** Low
- **Source:** `argus-core::context` / `project_state::ProgressDiff` — items
  are diffed by title, so a rename reads as an add + a resolve.
- **Why Deferred:** Titles are stable enough for now, and IDs already exist;
  switching to ID-keyed diffs is a small change but changes the diff output.
- **Required Before:** If diffs are consumed by automation, key them by IDs
  and treat titles as display only.

### DEF-015 — Empty Planned milestone contributes weight 1 at ratio 0

- **Status:** Deferred
- **Priority:** Low
- **Source:** `project_state::ProgressEngine` — a Planned milestone with no
  WorkItems contributes `weight=1, ratio=0` to the project overall, diluting
  the percentage.
- **Why Deferred:** It is arguably correct ("planned work not done"), and the
  rule is deterministic and documented; changing it changes the project
  overall formula.
- **Required Before:** Decide whether empty Planned milestones should be
  excluded from the project denominator; if so, document the new rule and
  update tests.

## Task Intelligence / Planner (found during V0.42–V0.43)

### DEF-016 — Classifier uncommon-wording limitations

- **Status:** Deferred
- **Priority:** Low
- **Source:** `argus-core::intelligence::classifier` — a task whose real
  character is expressed in unusual wording may not match a `TaskKind`
  keyword and falls back to `unknown`, so the tier is driven by complexity
  signals alone.
- **Why Deferred:** Deliberate for a rule-based v1; the classifier is
  reproducible and explainable, and a learned classifier is future work.
- **Required Before:** A learned/assisted classifier, benchmarked against the
  deterministic baseline.

### DEF-017 — WorkItems lack richer execution metadata

- **Status:** Deferred
- **Priority:** Medium
- **Source:** `work_items` has title/description/DoD but no tags, acceptance
  criteria, or test commands. Task Intelligence therefore sees coarser
  verification signals for WorkItems than for Tasks.
- **Why Deferred:** Additive schema/UX work; the executor layer (Tasks) has
  the richer fields.
- **Required Before:** A Project Manager that derives Tasks from WorkItems.

### DEF-018 — Per-attempt tier history not wired into the scheduler

- **Status:** Deferred
- **Priority:** Medium
- **Source:** `argus-core::intelligence::escalation` — escalation uses the
  routing decision's `required_tier` as `from_tier`; the V0.38 ExecutionLoop
  does not yet feed per-attempt tier/failure history back into routing.
- **Why Deferred:** The escalation policy and telemetry exist; wiring them
  into the runtime is a V0.44+ concern (and requires provider selection).
- **Required Before:** Provider/Auth work (V0.44) and a live routing loop.

## Planner / Provider (found during V0.43–V0.44)

### DEF-019 — Exact Planner Context Snapshot Replay

- **Status:** Deferred
- **Priority:** Medium
- **Source:** `argus-core::planner` — a `PlanningRequest` binds the V0.41
  `context_hash` and packet id, and manual export *regenerates* the packet
  from the same role/purpose/work-item. If project state changes between
  request creation and export, the regenerated packet is not byte-identical
  to what was bound (the hash still guards the import, but the exact snapshot
  is not stored).
- **Why Deferred:** The context compiler is deterministic, so this is only a
  fidelity/audit gap for manual planning; the binding is not broken.
- **Required Before:** Persisting the compiled packet (or a compact content
  snapshot) at request-creation time for exact replay/audit.

### DEF-020 — Postgres E2E suites are not parallel-safe

- **Status:** Deferred
- **Priority:** Medium
- **Source:** `argus-cli/tests/{agent_pipeline_e2e,execution_loop_e2e}.rs`,
  `scheduler_crash_recovery.rs`, `argus-core::task_repository`. These suites
  share one database and assert on globally-scanned state — e.g.
  `TaskScheduler::restore()` counts active tasks across *all* projects, so a
  concurrently-running sibling makes `active_tasks` 7 instead of 1, and daemon
  tests starve. V0.44 fixed this for `provider_routing_e2e` by holding a lock
  around the global `auth_profiles` table; the V0.38-era suites still need
  `-- --ignored --test-threads=1`.
- **Why Deferred:** Pre-existing since V0.38 and unrelated to provider routing;
  the suites are green when serialized. Fixing it means per-test schema
  isolation (separate database or rolled-back transaction per test), which
  touches the V0.38 frozen harness.
- **Required Before:** CI that runs the ignored suites in parallel.

### DEF-021 — AuthKind → BillingMode is a fixed heuristic

- **Status:** Deferred
- **Priority:** Low
- **Source:** `argus_core::provider::types::AuthKind::billing_mode` maps
  `SubscriptionLogin`/`CliCredential` → `SubscriptionIncluded` and
  `ApiKey`/`OAuth`/`EnvironmentCredential`/`CloudCredential` → `ApiMetered`.
  A subscription that happens to authenticate via official OAuth would be
  classified as metered and pushed through the budget gate.
- **Why Deferred:** The error is fail-safe (an unnecessary budget preflight,
  never an unbilled bypass), no builtin provider needs it today, and the
  correct fix is per-profile billing metadata rather than a derived function.
- **Required Before:** A provider whose subscription login is OAuth-only, or a
  real Cost Router that reads these numbers for spend reporting.

## See also

- [PROJECT_STATE.md](PROJECT_STATE.md) — what's actually done vs. in progress.
- [MASTER_ROADMAP.md](MASTER_ROADMAP.md) — where the fixes above naturally land.
