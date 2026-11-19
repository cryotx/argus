# Argus Operations Runbook

This runbook is the production operations baseline for Argus.

Scope:
- startup and shutdown
- database migration workflow
- rollback and recovery playbook
- audit/security incident triage
- key rotation workflow

## 1. Preconditions

- Rust toolchain: `1.85+`
- PostgreSQL with `pgvector`
- Config present at `config/argus.toml` (or env override with `ARGUS_CONFIG_PATH`)
- Required env vars loaded (`.env` supported):
  - `ARGUS_DATABASE_URL`
  - `ARGUS_MASTER_KEY` (or value referenced by `security.master_key_env`)

Set PATH for cargo:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
```

## 2. Startup Procedure

1) Validate build and basic health:

```bash
cargo build
cargo test
```

2) Check Argus status:

```bash
cargo run -p argus-cli -- status
```

3) Start Argus daemon:

```bash
cargo run -p argus-cli -- run
```

Expected signals:
- log contains `Starting Argus`
- DB connected successfully
- migrations applied (if `database.run_migrations = true`)
- log contains `Argus started`

## 3. Graceful Shutdown

- Send `SIGINT` (Ctrl+C) or `SIGTERM` to the running process.
- Confirm shutdown logs:
  - `Shutting down gracefully...`
  - `Database connections closed`
  - `Argus stopped`

## 4. Migration Procedure

Use this for schema changes or before first deploy.

```bash
cargo run -p argus-cli -- migrate
```

Post-check:

```bash
cargo run -p argus-cli -- status
```

If migration fails:
- do not continue with deploy/startup
- inspect the failing SQL migration in `migrations/`
- fix migration in a new patch and rerun `migrate`

## 5. Rollback and Recovery

Current system has rollback primitives in `argus-deploy` and audit visibility in `argus-security`.

### 5.1 Code rollback (safe default)

1) Identify last known good revision in your VCS.
2) Roll back to that revision with standard git workflow.
3) Rebuild and retest:

```bash
cargo build
cargo test
```

4) Re-run migrations only if rollback includes schema changes.
5) Restart Argus and verify via `status`.

### 5.2 Runtime rollback (deployment pipeline)

- Use deploy snapshot rollback path where target supports it (Docker/VPS/K8s/etc).
- After rollback, run health checks and confirm key service endpoints.
- Record rollback reason and outcome in audit trail.

## 6. Audit and Security Triage

Audit events are hash-chained and can be persisted via `PostgresAuditStore`.

### 6.1 Triage checklist

1) Verify chain integrity:
- call `AuditLogger::verify_chain()` in the service path you are investigating.

2) Identify incident class:
- `blocked` (policy enforcement)
- `failure` (execution failure)
- `warned` (degraded but allowed)

3) Scope blast radius:
- actor (`cli`, `skill-runtime`, `roles=...`)
- action (`builtin:*`, `server:*`, `cli:*`, `execute:*`)
- target/resource

4) Contain:
- tighten `security.blocked_commands`
- add `security.protected_paths`
- add `security.forbidden_content_markers`
- add `security.secret_keywords`

5) Validate fix:
- rerun affected flow
- confirm expected `blocked/success` audit outcome
- confirm chain verifies

## 7. Key Rotation Workflow

The current key vault baseline is in-memory provider plus pluggable interface.

### 7.1 Rotation process

1) Prepare replacement secrets from providers.
2) Update vault entries (`put`) with new values.
3) Validate dependent integrations (LLM, deploy tokens, social adapters).
4) Remove old secrets (`delete`) after successful verification window.
5) Record rotation event in audit trail.

### 7.2 Rotation policy

- Rotate high-risk keys regularly (30-90 days).
- Rotate immediately on suspected leak.
- Never store plaintext keys in git or logs.

## 8. On-call Quick Commands

```bash
# Build and test
cargo build && cargo test

# Apply migrations
cargo run -p argus-cli -- migrate

# Status check
cargo run -p argus-cli -- status

# Start daemon
cargo run -p argus-cli -- run
```

## 9. Post-incident Template

- Incident ID:
- Time window (UTC):
- Detection method:
- Root cause:
- Security controls triggered:
- Rollback/recovery steps executed:
- Data/availability impact:
- Preventive actions:
- Owner sign-off:
