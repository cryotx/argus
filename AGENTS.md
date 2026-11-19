# AGENTS.md - Argus Agent Playbook

This guide is for coding agents operating in this repository.
Prefer existing local patterns over generic best practices when they differ.
If user instructions conflict with this file, user instructions win.

## Ground Truth
- Primary stack: Rust workspace (`edition = 2024`, `rust-version = 1.85`).
- Workspace root: `Cargo.toml`; crates under `crates/*`.
- CLI entrypoint: `crates/argus-cli/src/main.rs`.
- Database migrations: `migrations/` (run via CLI).
- Frontend dashboard: `web/` (Vite + React + TypeScript).

## Repository-Specific Agent Rules
Checked in this repository:
- `.cursorrules`: not present.
- `.cursor/rules/`: not present.
- `.github/copilot-instructions.md`: not present.

If any of these files are added later, treat them as higher-priority local policy.

## High-Level Crate Map
- `crates/argus-core`: orchestration, scheduler, decomposition, quality checks.
- `crates/argus-cli`: command entrypoint, API startup, runtime commands.
- `crates/argus-llm`, `crates/argus-memory`: model routing, memory lifecycle.
- `crates/argus-worker`, `crates/argus-mcp`, `crates/argus-skills`: execution substrate.
- `crates/argus-security`: RBAC, command guard, audit, secret scanning.
- `crates/argus-git`, `crates/argus-deploy`, `crates/argus-social`, `crates/argus-monitor`: integrations and delivery.

## Setup and Local Bring-Up
- Initial setup: `./scripts/setup.sh`
- Apply DB migrations: `cargo run -p argus-cli -- migrate`
- Start service: `cargo run -p argus-cli -- run`
- Check status: `cargo run -p argus-cli -- status`

## Build, Lint, and Test Commands
Default to workspace-wide verification, then narrow scope for faster iteration.

### Build and Compile Checks
- Fast compile check (all crates): `cargo check --workspace`
- Single crate check: `cargo check -p argus-core`
- Debug build (all crates): `cargo build --workspace`
- Release build (all crates): `cargo build --workspace --release`

### Formatting and Linting
- Format all Rust code: `cargo fmt --all`
- Formatting check only: `cargo fmt --all -- --check`
- Lint all targets: `cargo clippy --workspace --all-targets`
- Lint with denied warnings: `cargo clippy --workspace --all-targets -- -D warnings`

### Test Commands (with single-test workflows)
- Full test suite: `cargo test --workspace`
- Single crate tests: `cargo test -p argus-security`
- List available tests in a crate: `cargo test -p argus-core -- --list`
- Run tests by substring: `cargo test -p argus-core scheduler`
- Run one exact test path:
  - `cargo test -p argus-security command_guard::tests::test_allow_safe_command -- --exact`
- Run one exact test with output:
  - `cargo test -p argus-core task_scheduler::tests::test_scheduler_task_failure -- --exact --nocapture`
- Run ignored tests (integration/DB-heavy):
  - `cargo test -p argus-cli -- --ignored --nocapture`
- Run one ignored integration test exactly:
  - `cargo test -p argus-cli test_cmd_status_integration -- --ignored --exact --nocapture`

### DB-Backed Test Notes
- Some tests require `ARGUS_DATABASE_URL` (or `DATABASE_URL`).
- Prefer explicit env setup before running ignored tests.

### Frontend Commands (`web/`)
- Install deps: `npm ci`
- Dev server: `npm run dev`
- Typecheck + production build: `npm run build`
- Lint frontend: `npm run lint`

## Coding Style Guidelines
Follow surrounding code first; use these defaults when no local pattern is obvious.

### Imports
- Group imports with blank lines: `std` -> third-party -> internal crates.
- Prefer explicit imports over wildcard/glob imports.
- Let `rustfmt` own final ordering and wrapping.

### Formatting and Structure
- Run `cargo fmt --all` after Rust changes.
- Keep functions focused; extract helpers to reduce nesting.
- Prefer early returns over deep `if/else` pyramids.
- Keep module layout consistent with nearby files.

### Types and API Design
- Reuse shared domain types from `argus_core::types` where appropriate.
- Prefer `ArgusResult<T>` and `ArgusError` in reusable library crates.
- Use enums for finite states; avoid stringly-typed state flags.
- Derive only traits you need (`Debug`, `Clone`, `Serialize`, etc.).
- Keep serde/sqlx field naming stable (`snake_case` unless compatibility requires otherwise).

### Naming Conventions
- Modules/files/functions/variables: `snake_case`.
- Structs/enums/traits: `PascalCase`.
- Constants/statics: `SCREAMING_SNAKE_CASE`.
- Test names: behavior-oriented, e.g. `test_<condition>_<expected>`.

### Error Handling
- Add context at failure boundaries (`context`, `with_context`).
- Avoid `unwrap`/`expect` in non-test code.
- `unwrap`/`expect` are acceptable in tests when intent is clear.
- Use `anyhow::Result` mainly at app/CLI boundaries.

### Async and Concurrency
- Use `tokio` primitives and explicit async boundaries.
- Use `Arc<T>` for shared runtime state; avoid hidden global mutability.
- Avoid blocking work on async executors unless explicitly isolated.
- Keep event-driven flows aligned with existing EventBus usage.

### Logging, Security, Secrets
- Use structured `tracing` events with useful fields.
- Never log secrets, tokens, credentials, or private key material.
- Preserve RBAC and command-guard checks in execution paths.
- Maintain audit logging for security-sensitive actions.

### Config, Environment, and Data Access
- Load config via `ArgusConfig::load(...)` or `ArgusConfig::load_auto()`.
- Respect precedence: environment variables override config file values.
- Keep secrets in environment or `.env`, never hardcode.
- Use SQL parameter binding (`.bind(...)`), never string-concatenate user input.
- Prefer typed SQL mapping (`query_as`) where practical.

### Testing Expectations
- Keep unit tests near implementation (`#[cfg(test)]`).
- Use `#[tokio::test]` for async behavior.
- Keep tests deterministic with direct assertions.
- Add regression tests for bug fixes and behavior changes.
- For one failing test, start with `-- --exact --nocapture`.

### Documentation and Dependency Hygiene
- Use `//!` for module docs and `///` for public APIs.
- Document intent/invariants, not obvious line-by-line behavior.
- Remove stale comments during refactors.
- Prefer workspace-managed dependency versions in root `Cargo.toml`.
- Add dependencies only when existing crates/utilities cannot cover the need.

## Agent Workflow Rules
- Make minimal, scoped edits; avoid unrelated rewrites.
- Do not revert user changes unless explicitly asked.
- Run the smallest useful verification first, then broaden.
- When touching scheduler/security/deploy flows, run focused tests in those crates.
