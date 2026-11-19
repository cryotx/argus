# Open Source Status

Current label: **developer-preview / alpha**, V0.45 verification foundation.
Code and [PROJECT_STATE.md](PROJECT_STATE.md) define implementation status;
[MASTER_ROADMAP.md](MASTER_ROADMAP.md) defines future scope.

## Implemented foundations

- Rust workspace, CLI startup and 35 PostgreSQL migrations.
- Durable tasks, dependency scheduling, crash recovery, idempotent creation,
  execution leases, budget enforcement and fail-closed accounting.
- Isolated verified Git worktrees, configurable Codex / Claude Code execution path,
  quality-gated Git checkpoints and durable Task Artifacts.
- Structured Project State, evidence-driven roadmap/progress, role-specific
  Context Compiler and provider-neutral Planner proposal lifecycle.
- Deterministic capability/provider/auth/quota selection with project policy.
- V0.45 required artifact/test/Git verification, durable reports, runtime
  WorkItem promotion and explicit, idempotent operator acceptance.
- HTTP write authentication, RBAC and persisted authorization audit.
- Dashboard build/list surfaces, actual redacted read-only configuration,
  persisted audit log listing and SSE events.
- Manually maintained OpenAPI inventory with all daemon API routes and generated
  endpoint constants. CI checks route and generated-file drift, secret patterns,
  Rust checks, serialized DB E2E suites and frontend build/lint.

## Remaining boundaries

- Execution uses one startup-selected provider and remains sequential (Codex by default; Claude Code is configurable). Selection does
  not yet dispatch providers or connect an autonomous Planner/Manager loop.
- Only the Developer execution role runs autonomously; specialized teams and
  multi-project concurrent operation remain planned.
- The V0.45 policy verifies execution/test evidence, not prose DoD semantics;
  semantic Reviewer and broader lint/build/health evaluators remain future work.
- Git checkpoint production is implemented; automated integration, PR,
  deployment and rollback are not connected to the main execution loop.
- Worktrees provide repository isolation, not a complete process sandbox.
  Docker sandbox requests now fail closed; Docker isolation is not implemented.
- The dashboard does not implement the full planning/evaluation/control workflow.
  Online configuration writes return 501 rather than claiming success.
- Read-only API requests retain anonymous Viewer access for the local dashboard.
  HTTP completion audit is best-effort; authorization audit is mandatory before writes.
- Historical bootstrap note-based verification remains a trusted legacy path.
- OpenAPI response types remain manually maintained; inventory drift checks
  are not schema/type generation.

## Verification evidence

V0.45 adds ten E2E cases using real PostgreSQL, processes, Git worktrees and
commits. The coding Agent is a local deterministic script, so these tests make
no model calls. A separate opt-in real-Agent test exists and is not implicitly
enabled by `--ignored`. DB suites remain serialized (DEF-020).

The offline MVP demo still generates a small Rust application deterministically;
it is a contributor smoke path, not evidence of autonomous project management.
Deployment/social/memory integrations have varying implementation depth.
