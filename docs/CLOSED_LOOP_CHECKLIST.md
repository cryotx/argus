# Closed-Loop Delivery Checklist

This checklist defines the minimum bar for "true closed-loop" development in Argus:

`Goal -> Decompose -> Schedule -> Execute -> Quality Gate -> Git Artifact -> Deploy/Rollback -> Status Report`

Use this document as the source of truth for release readiness.

## 1) Definition of Done (DoD)

A work item is considered closed-loop complete only if all items below pass:

- [ ] Natural-language goal is accepted and classified.
- [ ] Goal is decomposed into dependency-aware tasks.
- [ ] Scheduler executes tasks with deterministic state transitions.
- [ ] Quality gate runs (tests/lint/security checks) and either passes or creates fix tasks.
- [ ] Git artifact exists (branch + commit; optional PR based on policy).
- [ ] Deploy stage executes with health check.
- [ ] Rollback path is validated for failed health checks.
- [ ] Final status is emitted to at least one operator channel (CLI/API/Web/Chat).
- [ ] Audit trail exists for security-sensitive operations.

If any item is missing, the loop is not complete.

## 2) Environment Readiness Gate

Run these before any E2E verification:

```bash
./scripts/setup.sh
cargo run -p argus-cli -- migrate
cargo check --workspace
```

DB-backed tests require one of:

- `ARGUS_DATABASE_URL`
- `DATABASE_URL`

Quick validation command:

```bash
cargo test -p argus-cli -- --ignored --nocapture
```

## 3) Quality Gate Baseline (must pass)

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

If only one area changed, run focused verification first, then run full baseline.

## 4) Golden E2E Scenario

Use one stable scenario as the primary regression guard.

Example goal:

"Create a Rust hello-world project, add one unit test, run tests, and commit changes."

Minimum checks for this scenario:

- [ ] Intent recognized as project/task creation.
- [ ] At least 4 atomic tasks produced with dependencies.
- [ ] Scheduler advances task states to completion.
- [ ] Test command executes and passes.
- [ ] Commit object created with non-empty message.
- [ ] Completion event/report generated.

Recommended command path (adapt per runtime mode):

```bash
cargo run -p argus-cli -- mvp --prompt "Create a Rust hello-world project with one test, run tests, and commit"
```

One-command local path:

```bash
./scripts/e2e_mvp.sh
```

## 5) Failure and Rollback Validation

You need one intentionally failing deployment probe in CI/staging.

Pass criteria:

- [ ] Health check failure is detected.
- [ ] Rollback action is triggered automatically.
- [ ] System returns to previous healthy state.
- [ ] Failure + rollback are both visible in logs/events.

Related crate tests to run when changing deployment logic:

```bash
cargo test -p argus-deploy
```

## 6) Security and Governance Gate

For loop completion in any production-like flow:

- [ ] RBAC enforced for protected actions.
- [ ] Command guard blocks dangerous shell patterns.
- [ ] Secret scanner prevents sensitive token leakage.
- [ ] Audit log records who/what/when for critical operations.

Focused verification:

```bash
cargo test -p argus-security
cargo test -p argus-mcp
```

## 7) CI Merge Gate (required)

Every PR should pass all required checks:

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] Golden E2E scenario job
- [ ] DB-backed integration job (ignored tests enabled with DB)

If any gate is flaky, mark it unstable, fix reliability, then make it required.

## 8) Observability Minimum

Closed-loop requires fast diagnosis on failure.

Track at least:

- Task counts by state (queued/running/succeeded/failed/retried)
- Loop stage durations (decompose/schedule/execute/test/deploy)
- Failure categories (infra/test/security/provider)
- Rollback count and success rate

Operational target:

- Mean time to identify failing stage < 10 minutes.

## 9) Suggested Execution Plan (7-10 days)

Day 1

- Lock this checklist as release gate document.
- Ensure DB-backed ignored tests run locally.

Day 2-3

- Implement/refresh golden E2E script and assertions.
- Stabilize logs/events needed for debugging.

Day 4-5

- Add rollback failure simulation in staging test path.
- Confirm automatic rollback behavior.

Day 6-7

- Wire all gates into CI.
- Mark failing checks non-optional blockers only after stabilization.

Day 8-10 (buffer)

- Remove flaky tests.
- Tighten thresholds and finalize release readiness report.

## 10) Exit Criteria (Project-Level)

Project reaches "closed-loop complete" when:

- [ ] 3 consecutive CI runs pass full merge gate.
- [ ] Golden E2E passes 10 consecutive runs.
- [ ] At least 1 rollback drill succeeds end-to-end.
- [ ] Security-focused suites (`argus-security`, `argus-mcp`) pass.
- [ ] Runtime status output confirms healthy services.

Recommended runtime smoke:

```bash
cargo run -p argus-cli -- status
```

## 11) Branch Protection (Required for Real Closure)

Enable branch protection on `main` (or your default branch) with these settings:

- [ ] Require pull request before merge (no direct push)
- [ ] Require status checks to pass before merge
- [ ] Require branch to be up to date before merge
- [ ] Require conversation resolution before merge
- [ ] Restrict force pushes and branch deletion
- [ ] Restrict bypass permissions to repository owner/admin only

Required CI checks (must match job names in `.github/workflows/ci.yml`):

- `rust`
- `integration-db`
- `e2e-mvp`

Recommended review policy:

- [ ] Minimum 1 approval for normal changes
- [ ] Minimum 2 approvals for security/deploy/core-scheduler changes
- [ ] CODEOWNERS enabled for `crates/argus-security/**`, `crates/argus-deploy/**`, `crates/argus-core/**`

## 12) Three-Green Merge Rule

Before merging release-sensitive PRs:

- [ ] Same PR branch has 3 consecutive successful CI runs
- [ ] No flaky failure in `integration-db` or `e2e-mvp` during those runs
- [ ] Last run is based on latest target branch HEAD

Operational fallback if flaky:

1. Mark flaky check as non-blocking temporarily.
2. Open tracking issue with owner and SLA.
3. Fix stability.
4. Restore as required check.
