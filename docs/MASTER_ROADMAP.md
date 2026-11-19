# Master Roadmap

This is the authoritative long-term roadmap for ARGUS, from the current
release through the V1.0 north star. It exists so the product direction is
fixed in the repository rather than reconstructed from chat history each
time work resumes.

- For **what's actually built right now**, see [PROJECT_STATE.md](PROJECT_STATE.md).
- For **how the system is organized**, see [ARCHITECTURE.md](ARCHITECTURE.md).
- For **why specific decisions were made**, see [adr/README.md](adr/README.md).
- [`ROADMAP.md`](ROADMAP.md) is the original pre-V0.35, week-by-week initial
  build plan (P0–P14). It's kept for history; this document supersedes it as
  the forward-looking roadmap from V0.38 onward.

## Status legend

| Status | Meaning |
|---|---|
| **In Progress** | Actively being built; see PROJECT_STATE.md for the exact slice |
| **Planned** | Committed direction, not yet started |
| **Deferred** | Known and intentionally postponed — see DEFERRED_IMPROVEMENTS.md |

Nothing in this document marked Planned should be read as implemented. If
code and this roadmap ever disagree about what's built, PROJECT_STATE.md and
the code itself win.

Version numbering (V0.38, V0.39, ... V1.0) is provisional — it fixes an
*order*, not a promise about pace or a final numbering scheme. It may be
renumbered or split later; that is not itself a reason to touch code.

## Architecture Principles

These are binding constraints on every version below, not aspirations.
Several are already enforced in code today (see [adr/README.md](adr/README.md)
for the specific decisions); the rest exist precisely so later versions don't
violate them by accident.

1. **Agent != Model.** An Agent is a role ARGUS assigns work to; the model or
   CLI behind it is swappable.
2. **Role != Provider.** A role (Planner, Developer, Reviewer, ...) is a
   function in the system; a provider (Claude, Codex CLI, OpenCode, an API
   vendor) is one way to fill it. The same role may be filled by different
   providers at different times.
3. **Project Memory belongs to ARGUS, not the model.** Nothing a project
   needs to remember may live only inside a model's context window or a
   provider-side session.
4. **Conversation is evidence, not source of truth.** What was said in chat
   informs decisions; it is never itself the record of what happened.
5. **Project State / Git / DB / Artifacts are source of truth.** Durable,
   inspectable, queryable state — not narration — is authoritative.
6. **Planner model must be replaceable.** Swapping the model behind planning
   must never require touching execution code.
7. **Codex CLI / Claude / OpenCode are runtime providers, not fixed roles.**
   None of them is "the" Agent; each is one implementation of the Agent
   Runtime provider interface.
8. **Subscription login should be preferred where officially supported.**
   Direct API-key usage is a fallback, not the default.
9. **API is fallback controlled by BudgetService.** No paid API call bypasses
   budget enforcement, ever.
10. **Planner and Developer permissions must be separate.** The thing that
    decides what to do must not implicitly hold the permissions of the thing
    that does it.
11. **Canonical repository is never the Agent workspace.** An Agent always
    works in an isolated, verified worktree; the canonical repository is
    read/checkpointed, never edited in place by an Agent.
12. **Verified artifacts, not Agent claims, determine completion.** A task is
    done when its output is independently verified (tests, diff inspection,
    a real Git checkpoint) — never merely because an Agent reported success.

## Roadmap

### V0.38 — Reliable Agent Delivery Runtime
**Status: Done**

Make one Agent reliably take one task from claim to a verified, isolated,
recoverable execution, ending in a durable, inspectable record — the
foundation every later version stands on. All four phases are complete —
see PROJECT_STATE.md for current detail:

- Crash-safe recovery; spend/budget enforcement with a persistent ledger;
  fail-closed accounting; Git worktree isolation with ownership/branch/path
  verification; workspace persistence across restarts; production
  `ExecutionLoop` running a real Agent in a real, verified worktree; no
  hot-looping on a stuck admission gate (Phases 1-3, V0.38.1).
- Phase 4: ARGUS-driven Git Checkpoint of verified Agent output, a durable
  Task Artifact record, commit idempotency, and crash reconciliation between
  "Agent finished" and "checkpoint committed" — a task never reaches
  `Completed` ahead of its Artifact (Principle 12).

### V0.39 — Project State / Memory
**Status: Done (foundation)** — structured Project State, snapshot, service,
API, and ARGUS self-bootstrap are implemented and tested.

Give ARGUS its own durable understanding of a project — independent of any
single conversation or model context window (Principle 3).

- A Project State store separate from task/scheduler state.
- Ingestion from Git history, task artifacts, and Git checkpoints.
- A retrieval API planners and agents query instead of re-deriving context
  from scratch or from chat history.

### V0.40 — Roadmap & Progress Engine
**Status: Done** — deterministic, evidence-driven progress is implemented
(roadmap hierarchy, phases, weights, dependencies, ProgressDiff);
see PROJECT_STATE.md.

Turn a project goal into a maintained, versioned roadmap whose progress is
computed from verified artifacts, not self-reported status (Principle 12).

- Roadmap/milestone generation from a stated goal.
- Progress computed from Task Artifacts and Git Checkpoints.
- Drift detection: flag when the roadmap and the repository disagree.

### V0.41 — Context Compiler
**Status: Done**

Compile the structured Project State into a minimal, relevant, traceable
context packet for a specific role and purpose — deterministically, with no
model call: `Project Memory → Context Compiler → Role-specific Context Packet
→ Planner / Agent`.

- `ContextRequest` (role, purpose, optional WorkItem/Task/Execution, budget)
  and an immutable, hashed `ContextPacket` with provenance source refs.
- Deterministic selection/ranking, budget truncation, JSON + Markdown
  renderers, and the manual-planner packet foundation.

### V0.42 — Task Intelligence / Model Routing
**Status: Done**

Classify the work itself — difficulty and risk — and route it to the right
provider/model under an inspectable policy.

- Task/WorkItem difficulty and risk classification.
- A routing policy engine with per-project/per-role overrides and an audit
  trail of routing decisions.

### V0.43 — Planner Harness / Protocol
**Status: Done**

A stable, versioned protocol between planning (deciding what to do) and
execution (doing it), plus the harness that runs it — so either side can
evolve independently (Principle 6).

- Planner request/response schema; plan versioning as a first-class concept.
- Contract tests so a protocol change is caught, not discovered in production.
- Manual / Web Planner handoff built on the same protocol and Project State
  (V0.39), so a human and the autonomous planner share one source of truth.

### V0.44 — Provider / Auth / Subscription / Quota
**Status: Done** — deterministic capability-tier → concrete execution target
routing, with subscription and API billing kept as separate balances. Runtime
*dispatch* (spawn/cancel/resume/parse_usage across providers) is still the
V0.38 single-provider path; multi-provider execution is a later phase.

A first-class identity, provider, and billing-boundary layer (Principles
8, 9), making both the Planner and the Agent Runtime pluggable.

- Provider Capability Registry (what each provider can actually do).
- Auth provider abstraction and OAuth flows; subscription-login preferred,
  API-key fallback, enforced in that order.
- Per-provider quota accounting feeding BudgetService.

### V0.45 — Evaluation Harness
**Status: Done (foundation)** — pluggable required checks, real Git and persisted
artifact/test evidence, durable evaluation reports, runtime WorkItem promotion,
and separate operator acceptance. See [EVALUATION_HARNESS.md](EVALUATION_HARNESS.md).
The first policy verifies execution evidence; semantic DoD review and additional
evaluator kinds are later extensions.

Generalize the existing per-task Quality Gate into an Evaluation Harness that
distinguishes `ReportedComplete` / `VerifiedComplete` / `Accepted` with
recorded evidence (ADR #9).

- Pluggable evaluators and a recorded evaluation trail.
- WorkItem completion requires evidence-backed verification, not a claim.

### V0.46 — Environment / Resource State
**Status: Planned**

Track the environments and resources a project actually depends on (ports,
databases, services) and reconcile desired vs. observed state.

- Environment provisioning and a tracked `ProjectResource` registry.
- General desired-vs-observed reconciliation and drift detection.

### V0.47 — Portfolio Control Plane
**Status: Planned**

One control surface across many projects — status, budget, and risk at a
glance, ahead of actually running many projects at once (that's V0.7).

- Cross-project status/budget rollup.
- A single place to see and act on every project ARGUS is responsible for.

### V0.5 — Autonomous Project Manager
**Status: Planned**

ARGUS manages one project's full lifecycle — planning, execution,
verification, reporting — with minimal human input, composing V0.39–V0.47.

### V0.6 — Multi-Agent Team
**Status: Planned**

Multiple specialized agents (distinct roles, not just parallel copies of the
same role) collaborate on one project under a Manager/Planner.

### V0.7 — Multi-Project Management
**Status: Planned**

One ARGUS instance runs several independent projects concurrently without
cross-project interference — the Portfolio Control Plane (V0.47) under real
multi-project load.

### V0.8 — Chief / Portfolio Manager
**Status: Planned**

A top-level "Chief" role allocates attention and budget across the
portfolio and escalates to humans only when it should (Principle 10 applies
here too: the Chief plans allocation, it does not silently gain execution
permissions).

### V0.9 — Long-running Autonomous Operation
**Status: Planned**

ARGUS operates continuously over weeks/months without accumulating drift,
orphaned state, or silent failure — the operational maturity bar, not a
single feature.

### V1.0 — Agent / Project Operating System
**Status: Planned (north star)**

ARGUS as a general-purpose operating system for autonomous agent work across
many projects, providers, and teams — every principle above holding at scale.

## Non-Goals (Current Phase)

V0.38 Phase 4 explicitly does **not** include any of the following. They are
real, on the roadmap above, and deliberately not this sprint:

- Parallel Workers (a later stage, beyond V0.47)
- Manager / Planner Agent, Multi-Agent teams (V0.6)
- Multi-project autonomy (V0.7)
- An Environment Harness (see Long-Term Capability Catalog below)
- Automatic deployment
- Automatic merge from a task worktree (see ADR #6 — this is permanent, not
  just deferred)
- A Chief / portfolio-allocation role (V0.8)
- A full web UI
- A full secrets management platform (beyond the existing `key_vault`)

Scope creep back into this list without going through Development Discipline
below is exactly the failure mode this document exists to prevent.

## Development Discipline

Every new capability — including everything in the Long-Term Capability
Catalog below — follows the same path before it gets code:

```
Idea → Proposed Capability → Architecture Review → Roadmap Placement → Implementation
```

Not:

```
Idea → immediately code
```

"Architecture Review" means checking the idea against the Architecture
Principles above and the layer model in [ARCHITECTURE.md](ARCHITECTURE.md);
"Roadmap Placement" means it gets a version and a status in this document
before any code lands. This is the mechanism that keeps ARGUS from
re-acquiring the scope sprawl its earlier iterations had.

## Long-Term Capability Catalog

These are committed long-term directions that don't yet have a specific
version slot above, either because they cut across several versions or
because their exact placement depends on work that comes first. Recording
them here now means they don't need to be re-derived from chat history when
their time comes. All are **Planned** unless stated otherwise; none may be
implemented ahead of being given a roadmap version per Development Discipline
above.

### Agent composition

An Agent is not a single object bound to a model. Long-term, an Agent's
identity is the combination of independent axes (ADR #1, #10):

```
Agent
├── Role              (Planner, Developer, Reviewer, Tester, Integrator, ...)
├── Project
├── Runtime Provider   (Claude Code, Codex CLI, OpenCode, ...)
├── Model
├── Auth Profile       (subscription login, API key, environment credential)
├── Context            (compiled per Context Compiler, below)
├── Permissions
├── Workspace
└── Budget
```

The same provider can fill different roles on different tasks; the same role
can be filled by different providers. Lands with V0.44 (Provider / Auth /
Subscription / Quota).

### Context Compiler

Give each role the minimum context it needs, computed from Project
Intelligence (V0.39/V0.40) rather than handed the full project history:
`Project Memory → Context Compiler → Minimal Relevant Context → Model`. A
Chief needs portfolio health; a Project Manager needs charter/roadmap/open
work; a Developer needs its task, Definition of Done, and relevant files.
Implemented as deterministic infrastructure in V0.41 (Context Compiler);
semantic retrieval may be layered on later.

### Planner Harness / Planner Protocol

The Planner Protocol (V0.43) is the schema; the Planner Harness is what runs
it: Context Compiler → Project State Loader → Provider Routing → structured
`PlanningRequest`/`PlanningResult` → Plan Validation → Plan Diff → a State
Update Proposal (never an in-place overwrite — see Plan Versioning below). A
`PlanningRequest` carries the project charter, current state, roadmap, open
work, decisions, risks, deferred improvements, recent artifacts, and budget
constraints; a `PlanningResult` proposes roadmap updates, work items,
decisions, risks, and a recommended next action, all reviewable before they
change Project State.

### Execution Harness

The generalization of today's `ExecutionLoop` + `AgentTaskExecutor` (Layers
3/5, already Implemented for one provider): a uniform `ExecutionRequest` in →
workspace, permissions, timeout, environment, provider execution,
stdout/stderr, usage/cost, retries, recovery, Git changes, and Artifact out —
so that Claude, Codex, OpenCode, or a future provider all look the same to
everything above this layer. Formalizing this as a named, provider-agnostic
harness (rather than the current single-provider wiring) is V0.43 work.

### Evaluation Harness

A task being reported done by an Agent and a task being verified complete are
different states (ADR #9). Today's Quality Gate (`argus-core::quality`) is
the first, narrow instance of this. Long-term this generalizes to a layer
that distinguishes `ReportedComplete` / `VerifiedComplete` / `Accepted`, and
can call on tests, build, static checks, required files, Git diff, security
checks, a stated Definition of Done, or a Reviewer Agent — only a
`VerifiedComplete` result may advance Roadmap & Progress Engine (V0.40)
state.

### Provider / Auth / Quota

Auth is independent of provider and role (ADR #10). Long-term shape:

```
AuthProfile: subscription login | CLI credential | OAuth | API key | environment credential
QuotaSnapshot: provider, auth_profile, available, remaining, reset_at, checked_at, confidence
FallbackPolicy: Never | Ask | AutomaticWithinBudget
Routing: Subscription → Alternate Subscription → API Fallback → BudgetService
```

Subscription-first / API-fallback (ADR #7) stays the default ordering; a
provider-reported quota is always best-effort, cross-checked against local
usage estimates and rate-limit observations, never trusted alone. ARGUS must
never scrape or reuse an unofficial/private OAuth token — only official
CLI/provider-supplied auth flows. Lands with V0.44 (Provider / Auth /
Subscription / Quota); routing policy itself is V0.42 (Task Intelligence /
Model Routing).

### Manual / Web Planner

A Manual Planner provider lets a human keep using a web chat UI (ChatGPT
Web, Claude Web, or similar) as a Planning provider without requiring API
access: `ARGUS → generates a Planner Packet → user pastes into a web model →
model returns a structured PlanningResult → user imports it → ARGUS updates
Project State`. This avoids long-lived web chats degrading and lets a
project's actual state live in ARGUS rather than in that chat's history — a
web model is just one more Planner Provider. Part of V0.43 (Planner Harness /
Protocol).

### Environment / Resource Layer

Lands with V0.46 (Environment / Resource State), behind Project State, Roadmap
Engine, and the Planner Protocol (V0.39–V0.43). Covers:

- **Environment Provisioning** — runtime (Docker / native / external),
  network, database, storage, and host choices per project/environment.
- **Environment Configuration Hierarchy** — `Global Defaults → Project
  Policy → Environment Override → Resolved Environment`.
- **Port Allocation** (`PortAllocator`, `auto_persistent` mode) — an
  automatically assigned port is checked against reserved ports, OS-level
  usage, and ARGUS's own resource registry once, then persisted and reused,
  never re-randomized.
- **Database Allocation** (`DatabaseMode`: `SharedInstance` /
  `DedicatedDatabase` / `DedicatedInstance` / `External`) — shared-instance
  isolation and fully-separate-instance isolation are different guarantees
  and must stay distinguishable.
- **Resource Registry** (`ProjectResource`) — every container, process,
  port, database, volume, domain, or service ARGUS creates is a tracked
  resource with an owner (`resource_id`, `project_id`, `environment_id`,
  `resource_type`, `ownership`, `desired_state`, `observed_state`,
  `created_at`, `last_seen_at`) — never something an Agent stands up ad hoc
  through a shell command with no record of it existing.
- **Reconciliation** — a general desired-vs-observed-state model (e.g.
  desired "container running" vs. observed "container missing") that
  detects drift, retries, repairs, or escalates to a human. The crash
  reconciliation in V0.38 Phase 4 (Agent finished vs. checkpoint committed)
  is the first, narrow instance of this general capability, not the whole of
  it.

### Secrets Management

A `SecretRef` — scoped by project, role, and provider, with expiration,
rotation, redaction, and audit — replaces handing secret material to a model
in plaintext. Logs, Planner Packets, and Artifacts must never leak a secret
value. `argus-security::key_vault` (Implemented, partial) is the current
storage primitive this builds on; the reference/scoping/redaction model
above does not exist yet.

### Provider Capability Registry

A queryable registry of what each provider can actually do — structured
output, cost reporting, session resume, sandboxing, workspace enforcement,
network access, tool use, cancellation, context size, supported auth types —
so routing decisions are made from declared capability, not assumption.
Lands with V0.44 (Provider / Auth / Subscription / Quota).

### Plan Versioning / Roadmap Revision

A roadmap change is never a silent overwrite. Each material planning change
becomes a `Roadmap Revision` recording the old plan, the new plan, a diff,
the reason, which planner produced it, any human approval, and a timestamp —
so "why did the plan change" (e.g. parallel workers being superseded by
Project Memory as the next priority) has an answer in the repository, not
just in memory. Lands with the Planner Protocol, V0.43.

### Provenance

The full chain from intent to code must be traceable end to end: `Project →
Roadmap → Work Item → Planner Run → Task → Execution → Agent → Workspace →
Artifact → Commit → Evaluation → Cost`. Today's `argus-security::audit`
records security-relevant actions; task/artifact-level provenance across
this whole chain does not exist yet.

### Planner Evaluation

A benchmark for the Planner role itself, not just task output: per planner
provider/model, track acceptance/rejection rate, downstream task success,
human corrections, replans, cost, and latency, so provider/model routing for
planning is chosen from evidence rather than assuming the most expensive
model plans best. Depends on the Planner Protocol (V0.43) and Planner
Evaluation data existing first.

### Portfolio / Human Inbox / Approval Gates

The Portfolio Control Plane (V0.47) shows every project's status, budget,
and blockers in one place so a person doesn't need to open each project's
conversation to know its state. A **Human Inbox** ("Needs Your Attention")
surfaces only what genuinely requires a human decision — an architecture
call, API-fallback approval, a production deploy, a merge conflict, a budget
increase, a dangerous operation — rather than requiring continuous
supervision. **Approval Gates** make this a system-enforced policy, not a
prompt convention (ADR #11): e.g. a Developer role may act freely inside its
own worktree but never modify the canonical branch; an Integrator may act on
an integration branch but requires approval to touch `main`; production
deploys always require approval. Depends on V0.47 and the RBAC substrate
(already Implemented).

### Integration / Merge

Per ADR #6, ARGUS never auto-merges from a task worktree. The long-term
Integration Layer this implies: `Task Artifacts → Integration Queue →
Rebase/Merge Simulation → Full Test → Conflict Detection → Reviewer/
Integrator → Merge Candidate`. Several tasks each individually passing their
own Quality Gate does not imply they merge cleanly together — that's what
this layer exists to check. Not started; depends on the Evaluation Harness
above.

### Parallel Workers / Multi-Agent / Chief

Scheduled above (Parallel Workers as a later stage beyond V0.47; Multi-Agent
as V0.6; Chief as V0.8) — listed here
only to be explicit that none of the three is scoped ahead of the
prerequisites called out in their roadmap entries: Parallel Workers needs
proven concurrent workspace isolation first; Multi-Agent needs the Planner
Protocol and separated role permissions first; Chief needs the Portfolio
Control Plane first.
