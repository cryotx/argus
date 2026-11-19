# Tech Stack — Complete Dependency Reference

This document lists every Rust crate dependency, third-party tool, and external service used by Argus, organized by crate and purpose.

---

## 1. Workspace-Level Dependencies

These are shared across multiple crates and specified in `[workspace.dependencies]` in the root `Cargo.toml`.

### 1.1 Async Runtime & Concurrency

| Crate | Version | Purpose |
|-------|---------|---------|
| `tokio` | 1.x | Async runtime (full features: rt-multi-thread, macros, signal, process, fs, io, sync, time, net) |
| `tokio-stream` | 0.1.x | Stream utilities for async iterators |
| `tokio-util` | 0.7.x | Codec, compat layers, cancellation tokens |
| `futures` | 0.3.x | Future combinators, stream traits |

### 1.2 Serialization

| Crate | Version | Purpose |
|-------|---------|---------|
| `serde` | 1.x | Serialize/deserialize framework (derive feature) |
| `serde_json` | 1.x | JSON serialization |
| `toml` | 0.8.x | TOML config file parsing |
| `serde_yaml` | 0.9.x | YAML parsing (for Docker Compose, K8s manifests) |

### 1.3 Error Handling

| Crate | Version | Purpose |
|-------|---------|---------|
| `thiserror` | 2.x | Derive `Error` trait for custom error types |
| `anyhow` | 1.x | Flexible error handling in application code |

### 1.4 Logging & Tracing

| Crate | Version | Purpose |
|-------|---------|---------|
| `tracing` | 0.1.x | Structured logging and distributed tracing |
| `tracing-subscriber` | 0.3.x | Subscriber implementations (fmt, json, env-filter) |
| `tracing-appender` | 0.2.x | Non-blocking log file appender |

### 1.5 Date/Time

| Crate | Version | Purpose |
|-------|---------|---------|
| `chrono` | 0.4.x | Date/time types with timezone support (serde feature) |

### 1.6 IDs

| Crate | Version | Purpose |
|-------|---------|---------|
| `uuid` | 1.x | UUID v4 generation (v4, serde features) |

### 1.7 Concurrent Data Structures

| Crate | Version | Purpose |
|-------|---------|---------|
| `dashmap` | 6.x | Concurrent hash map (for working memory, caches, rate limiters) |

---

## 2. Per-Crate Dependencies

### 2.1 argus-core

Core business logic: Brain Engine, Task Scheduler, Config, Event Bus.

| Crate | Version | Purpose |
|-------|---------|---------|
| `tokio` | (workspace) | Broadcast channels for event bus, timers for scheduler |
| `serde` | (workspace) | Event serialization |
| `serde_json` | (workspace) | Structured data in events and task context |
| `chrono` | (workspace) | Timestamps in events and tasks |
| `uuid` | (workspace) | IDs for tasks, events, projects |
| `dashmap` | (workspace) | Working memory, in-memory caches |
| `regex` | 1.x | Config NL pattern matching, intent keyword detection |
| `petgraph` | 0.6.x | Directed acyclic graph for task dependencies |

### 2.2 argus-llm

LLM provider management, routing, failover, budget tracking.

| Crate | Version | Purpose |
|-------|---------|---------|
| `reqwest` | 0.12.x | HTTP client for all LLM API calls (json, stream, rustls-tls features) |
| `tokio` | (workspace) | Async HTTP, timers for circuit breaker |
| `serde` | (workspace) | Request/response serialization |
| `serde_json` | (workspace) | JSON API payloads |
| `async-trait` | 0.1.x | Async trait for `LlmProvider` trait |
| `futures` | (workspace) | Stream handling for streaming LLM responses |
| `eventsource-stream` | 0.2.x | SSE parsing for streaming API responses |
| `tiktoken-rs` | 0.6.x | Token counting for OpenAI-compatible models |
| `backoff` | 0.4.x | Exponential backoff for retries and circuit breaker |

### 2.3 argus-memory

Four-layer memory, embedding, semantic search, context builder.

| Crate | Version | Purpose |
|-------|---------|---------|
| `sqlx` | 0.8.x | PostgreSQL async driver (runtime-tokio, tls-rustls, postgres, uuid, chrono, json features) |
| `pgvector` | 0.4.x | pgvector Rust types for `vector(1024)` columns |
| `fastembed` | 4.x | Local BGE-M3 embedding model (ONNX runtime, no API calls) |
| `dashmap` | (workspace) | Working memory (RAM layer) |
| `tokio` | (workspace) | Async DB queries, scheduled lifecycle jobs |

### 2.4 argus-worker

Worker pool, subprocess drivers, sandboxing, resource monitoring.

| Crate | Version | Purpose |
|-------|---------|---------|
| `tokio` | (workspace) | Process spawning (`tokio::process::Command`), I/O streams |
| `bollard` | 0.17.x | Docker API client for container-based sandboxing |
| `serde` | (workspace) | Worker communication protocol serialization |
| `serde_json` | (workspace) | JSON task payloads and worker output |
| `nix` | 0.29.x | Unix process management (signals, resource limits via setrlimit) |
| `procfs` | 0.17.x | Linux `/proc` filesystem reading for resource monitoring |
| `tempfile` | 3.x | Temporary directories for worker sandboxes |

### 2.5 argus-mcp

MCP client protocol, server registry, security auditing.

| Crate | Version | Purpose |
|-------|---------|---------|
| `reqwest` | (workspace) | HTTP client for SSE and Streamable HTTP MCP protocols |
| `tokio` | (workspace) | Subprocess management for stdio MCP servers |
| `serde` | (workspace) | MCP message serialization |
| `serde_json` | (workspace) | JSON-RPC message format |
| `eventsource-stream` | 0.2.x | SSE protocol parsing for MCP SSE transport |
| `async-trait` | 0.1.x | MCP client trait |
| `jsonrpc-core` | 18.x | JSON-RPC 2.0 message types (used by MCP protocol) |

### 2.6 argus-skills

Skills runtime, built-in skills, skill generator.

| Crate | Version | Purpose |
|-------|---------|---------|
| `tokio` | (workspace) | Skill execution as async subprocess |
| `serde` | (workspace) | Skill manifest serialization |
| `serde_json` | (workspace) | Skill I/O format |
| `tempfile` | 3.x | Temporary script files for self-generated skills |
| `which` | 7.x | Find Python/Node interpreters on PATH |

### 2.7 argus-git

Git operations: clone, branch, commit, push, PR management.

| Crate | Version | Purpose |
|-------|---------|---------|
| `gix` | 0.68.x | Pure-Rust git implementation (gitoxide) for local operations |
| `reqwest` | (workspace) | GitHub/GitLab API calls for PR management |
| `serde` | (workspace) | API response parsing |
| `serde_json` | (workspace) | GitHub API JSON |
| `tokio` | (workspace) | Async remote operations |

### 2.8 argus-deploy

Deployment pipeline, deployers, health checks, rollback.

| Crate | Version | Purpose |
|-------|---------|---------|
| `bollard` | 0.17.x | Docker API for Docker deployer (build, push, run, compose) |
| `russh` | 0.46.x | SSH2 client for VPS deployer (async, pure Rust) |
| `russh-keys` | 0.46.x | SSH key management |
| `reqwest` | (workspace) | Vercel/K8s API calls, HTTP health probes |
| `kube` | 0.97.x | Kubernetes API client (optional, feature-gated) |
| `k8s-openapi` | 0.23.x | K8s resource type definitions (optional, feature-gated) |
| `tokio` | (workspace) | Pipeline orchestration, health check intervals |
| `serde` | (workspace) | Deploy config serialization |
| `serde_json` | (workspace) | Pipeline stage results |

### 2.9 argus-social

Social gateway, platform adapters, notification engine, web dashboard.

| Crate | Version | Purpose |
|-------|---------|---------|
| `axum` | 0.8.x | Web framework for REST API and WebSocket (ws, json features) |
| `axum-extra` | 0.10.x | TypedHeader, cookie support |
| `tower` | 0.5.x | Middleware layer (rate limiting, CORS, compression) |
| `tower-http` | 0.6.x | CORS, compression, static file serving, trace layer |
| `teloxide` | 0.13.x | Telegram Bot API framework |
| `serenity` | 0.12.x | Discord bot library |
| `poise` | 0.6.x | Discord command framework (built on serenity) |
| `reqwest` | (workspace) | WeChat Enterprise API, Slack API calls |
| `tokio` | (workspace) | Async adapters, WebSocket handling |
| `serde` | (workspace) | Message serialization |
| `serde_json` | (workspace) | API payloads |
| `tokio-tungstenite` | 0.24.x | WebSocket implementation (used internally by axum ws) |

### 2.10 argus-monitor

System monitoring, watchdog, alerting.

| Crate | Version | Purpose |
|-------|---------|---------|
| `sysinfo` | 0.32.x | Cross-platform system info (CPU, memory, disk, processes) |
| `tokio` | (workspace) | Monitoring intervals, watchdog timer |
| `procfs` | 0.17.x | Linux-specific process monitoring |

### 2.11 argus-security

Key vault, RBAC, audit logging, command guard, secret scanner.

| Crate | Version | Purpose |
|-------|---------|---------|
| `aes-gcm` | 0.10.x | AES-256-GCM encryption for key vault |
| `argon2` | 0.5.x | Argon2id key derivation from master password |
| `bcrypt` | 0.16.x | Password hashing for web dashboard login |
| `jsonwebtoken` | 9.x | JWT token creation and validation for web API auth |
| `zeroize` | 1.x | Secure memory zeroing for key material |
| `regex` | 1.x | Secret detection patterns (API keys, passwords, tokens) |
| `rand` | 0.8.x | Cryptographic random (nonce generation, token generation) |
| `sqlx` | (workspace) | Audit log persistence |
| `tokio` | (workspace) | Buffered batch audit insert |

### 2.12 argus-cli

CLI entry point, argument parsing, daemon mode.

| Crate | Version | Purpose |
|-------|---------|---------|
| `clap` | 4.x | CLI argument parsing (derive feature) |
| `tokio` | (workspace) | Async runtime entry point |
| `tracing` | (workspace) | Logging initialization |
| `tracing-subscriber` | (workspace) | Subscriber setup (JSON output, env filter) |
| `tracing-appender` | (workspace) | File-based log output |
| `ctrlc` | 3.x | Signal handling (SIGINT, SIGTERM) — or use `tokio::signal` directly |
| `dotenvy` | 0.15.x | Load `.env` files for development |

---

## 3. Database Dependencies

| Component | Version | Purpose |
|-----------|---------|---------|
| PostgreSQL | 16+ | Primary data store |
| pgvector extension | 0.7+ | Vector similarity search for memory embeddings |
| pg_trgm extension | (built-in) | Trigram text search for project/memory name search |
| uuid-ossp extension | (built-in) | UUID generation in SQL |

### PostgreSQL Driver

`sqlx` is the async PostgreSQL driver, shared by `argus-memory`, `argus-security`, and any crate that needs direct DB access. Connection pool management is centralized in `argus-core` and passed via `Arc<PgPool>`.

```toml
[workspace.dependencies.sqlx]
version = "0.8"
features = [
    "runtime-tokio",
    "tls-rustls",
    "postgres",
    "uuid",
    "chrono",
    "json",
    "migrate",
]
```

---

## 4. Embedding Model

| Component | Details |
|-----------|---------|
| Model | BGE-M3 (BAAI/bge-m3) |
| Runtime | ONNX Runtime via `fastembed-rs` |
| Dimensions | 1024 |
| Execution | Local CPU (no GPU required, no API calls) |
| Loading | Once at startup, shared via `Arc` |
| Batch size | Configurable (default: 32) |

**Why BGE-M3?**
- Multilingual (Chinese + English critical for Argus)
- 1024 dimensions (good balance of quality vs. storage)
- Available via `fastembed-rs` (local, no API dependency)
- Fixed regardless of LLM provider changes (memory vector consistency)

---

## 5. Third-Party AI Tools (Worker Drivers)

These are external CLI tools that Argus spawns as worker subprocesses.

| Tool | Protocol | Purpose |
|------|----------|---------|
| OpenCode | stdin/stdout JSON | Primary AI coding agent |
| Claude Code | stdin/stdout | Anthropic's coding agent |
| Aider | stdin/stdout | Git-aware AI pair programmer |
| Generic CLI | stdin/stdout (configurable) | Any CLI tool as a worker |

Workers are spawned via `tokio::process::Command` and communicate through stdin/stdout. Argus does NOT link against these tools — they are external binaries that must be installed on the system.

---

## 6. Development Tooling

| Tool | Purpose |
|------|---------|
| `cargo` | Build system and package manager |
| `rustfmt` | Code formatting |
| `clippy` | Linting |
| `cargo-nextest` | Fast test runner (optional, recommended) |
| `sqlx-cli` | Database migration management (`cargo install sqlx-cli`) |
| `cargo-watch` | Auto-rebuild on file changes during development |
| `cargo-deny` | Audit dependencies for vulnerabilities and license compliance |
| `cargo-udeps` | Find unused dependencies |

---

## 7. Cargo Feature Flags

Optional features that can be enabled/disabled at compile time:

```toml
[workspace]

# Note: Features are defined per-crate, not at workspace level.
# The following features would be in argus-cli/Cargo.toml:
[features]
default = ["telegram", "web-dashboard"]

# Social platform adapters
telegram = ["dep:teloxide"]
discord = ["dep:serenity", "dep:poise"]
wechat = []           # uses reqwest (always available)
slack = []            # uses reqwest (always available)
web-dashboard = ["dep:axum", "dep:tower-http"]

# Deployers
docker = ["dep:bollard"]      # enabled by default
vps = ["dep:russh"]            # enabled by default
kubernetes = ["dep:kube", "dep:k8s-openapi"]
vercel = []                    # uses reqwest

# Worker sandboxing
docker-sandbox = ["dep:bollard"]  # Docker-based worker isolation

# Monitoring
prometheus = []                # /metrics endpoint
```

This allows building a minimal binary without unused platform adapters:
```bash
# Minimal build (Telegram + Docker only)
cargo build --release --no-default-features --features telegram,docker

# Full build
cargo build --release --all-features
```

---

## 8. Workspace Cargo.toml Structure

```toml
[workspace]
resolver = "2"
members = [
    "crates/argus-core",
    "crates/argus-llm",
    "crates/argus-memory",
    "crates/argus-worker",
    "crates/argus-mcp",
    "crates/argus-skills",
    "crates/argus-git",
    "crates/argus-deploy",
    "crates/argus-social",
    "crates/argus-monitor",
    "crates/argus-security",
    "crates/argus-cli",
]

[workspace.package]
version = "0.1.0"
edition = "2024"
license = "MIT"
rust-version = "1.85"

[workspace.dependencies]
# Async
tokio = { version = "1", features = ["full"] }
tokio-stream = "0.1"
tokio-util = { version = "0.7", features = ["codec"] }
futures = "0.3"

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"

# Error handling
thiserror = "2"
anyhow = "1"

# Logging
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
tracing-appender = "0.2"

# Date/time
chrono = { version = "0.4", features = ["serde"] }

# IDs
uuid = { version = "1", features = ["v4", "serde"] }

# Concurrent data structures
dashmap = "6"

# HTTP
reqwest = { version = "0.12", features = ["json", "stream", "rustls-tls"], default-features = false }

# Database
sqlx = { version = "0.8", features = ["runtime-tokio", "tls-rustls", "postgres", "uuid", "chrono", "json", "migrate"] }

# Async traits
async-trait = "0.1"
```

---

## 9. External Services

| Service | Required | Purpose |
|---------|----------|---------|
| PostgreSQL 16+ | Yes | Primary data store |
| Docker Engine | Recommended | Worker sandboxing, Docker deployer |
| Git | Yes | Version control (via gix, but `git` CLI as fallback) |
| OpenCode / Claude Code / Aider | At least one | Worker subprocess for AI coding |
| LLM API (any provider) | Yes | Intelligence layer (at minimum, local Ollama) |

### Optional Services

| Service | Purpose |
|---------|---------|
| GitHub / GitLab | PR workflow, code hosting |
| Container Registry | Docker image storage for deployments |
| Telegram Bot API | Telegram adapter |
| Discord Bot | Discord adapter |
| WeChat Enterprise API | WeChat adapter |
| Slack API | Slack adapter |
| Vercel API | Vercel deployer |
| Kubernetes Cluster | K8s deployer |
| Let's Encrypt / ACME | TLS certificates for web dashboard |

---

## 10. Version Pinning Policy

- **Major version pinning**: All dependencies specify major version (e.g., `tokio = "1"`)
- **Cargo.lock committed**: Yes — reproducible builds
- **Update cadence**: Monthly `cargo update` with CI validation
- **Security audits**: `cargo deny check` in CI, `cargo audit` for known vulnerabilities
- **MSRV**: 1.85 (Rust 2024 edition)

---

## 11. Binary Size Estimates

| Configuration | Estimated Size |
|--------------|---------------|
| Debug build (full features) | ~150-200 MB |
| Release build (full features) | ~30-50 MB |
| Release build (minimal: Telegram + Docker) | ~20-35 MB |
| Release build with `strip` and LTO | ~15-30 MB |

Optimization settings for release:
```toml
[profile.release]
lto = "thin"
codegen-units = 1
strip = true
opt-level = 3
```

---

## 12. Cross-Compilation Targets

| Target | Support |
|--------|---------|
| `x86_64-unknown-linux-gnu` | Primary (development & deployment) |
| `x86_64-unknown-linux-musl` | Static binary for Alpine/minimal containers |
| `aarch64-unknown-linux-gnu` | ARM64 servers (AWS Graviton, etc.) |
| `x86_64-apple-darwin` | macOS development |
| `aarch64-apple-darwin` | Apple Silicon development |

The primary deployment target is Linux x86_64. macOS targets are for development only.
