#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

ok() { printf "[OK] %s\n" "$1"; }
warn() { printf "[WARN] %s\n" "$1"; }
fail() { printf "[ERROR] %s\n" "$1"; exit 1; }

check_cmd() {
  local cmd="$1"
  local required="$2"
  local hint="$3"
  if command -v "$cmd" >/dev/null 2>&1; then
    ok "$cmd found"
  else
    if [[ "$required" == "required" ]]; then
      fail "$cmd is required. ${hint}"
    else
      warn "$cmd not found. ${hint}"
    fi
  fi
}

copy_if_missing() {
  local src="$1"
  local dst="$2"
  if [[ -f "$dst" ]]; then
    ok "exists: ${dst#$ROOT_DIR/}"
  else
    cp "$src" "$dst"
    ok "created: ${dst#$ROOT_DIR/}"
  fi
}

if [[ ! -f "$ROOT_DIR/Cargo.toml" ]]; then
  fail "Run this script inside the argus repository."
fi

echo "Argus setup wizard"
echo "repo: $ROOT_DIR"
echo

echo "1) Checking required tools"
check_cmd "cargo" "required" "Install Rust from https://rustup.rs"
check_cmd "rustc" "required" "Install Rust from https://rustup.rs"
check_cmd "git" "required" "Install Git before running coding tasks"

echo
echo "2) Checking optional tools"
check_cmd "psql" "optional" "Optional if you use remote Postgres; useful for DB checks"
check_cmd "pg_isready" "optional" "Optional; used to quickly test local Postgres"
check_cmd "curl" "optional" "Optional; used for API health tests"
check_cmd "codex" "optional" "Default coding executor; install and authenticate Codex CLI, or select Claude Code"
check_cmd "claude" "optional" "Optional alternative: set ARGUS_DEFAULT_WORKER=claude_code"

echo
echo "3) Preparing local config files"
copy_if_missing "$ROOT_DIR/config/argus.default.toml" "$ROOT_DIR/config/argus.toml"
copy_if_missing "$ROOT_DIR/config/providers.example.toml" "$ROOT_DIR/config/providers.toml"
copy_if_missing "$ROOT_DIR/.env.example" "$ROOT_DIR/.env"

echo
echo "4) Basic sanity checks"
if grep -q "replace-with-32-byte-secret" "$ROOT_DIR/.env"; then
  warn ".env still has placeholder ARGUS_MASTER_KEY"
else
  ok "ARGUS_MASTER_KEY looks customized"
fi

if grep -q '\${[A-Z_][A-Z0-9_]*}' "$ROOT_DIR/config/providers.toml"; then
  warn "Optional chat providers use environment references; configure matching keys or disable unused providers"
fi

echo
echo "Setup complete. Next steps:"
echo "  1) Edit .env: set ARGUS_DATABASE_URL, random ARGUS_MASTER_KEY and distinct HTTP tokens"
echo "  2) Authenticate Codex CLI, or select Claude Code with ARGUS_DEFAULT_WORKER=claude_code"
echo "  3) Use ARGUS_ENV=development locally; chat provider API configuration is optional in development"
echo "  4) Ensure Postgres + pgvector is reachable (local demo: docker compose up -d postgres)"
echo "  5) Run: cargo run -p argus-cli -- migrate"
echo "  6) Run: cargo run -p argus-cli -- run"
echo "  7) Check: curl http://127.0.0.1:3100/api/status"
echo "  8) Optional dashboard: cd web && npm ci && npm run dev"
