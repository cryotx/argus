#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

ok() { printf "[OK] %s\n" "$1"; }
warn() { printf "[WARN] %s\n" "$1"; }
fail() { printf "[ERROR] %s\n" "$1"; exit 1; }

if [[ ! -f "$ROOT_DIR/Cargo.toml" ]]; then
  fail "Run this script inside the argus repository."
fi

if ! command -v cargo >/dev/null 2>&1; then
  fail "cargo is required. Install Rust from https://rustup.rs"
fi

if [[ -z "${ARGUS_DATABASE_URL:-}" && -n "${DATABASE_URL:-}" ]]; then
  export ARGUS_DATABASE_URL="$DATABASE_URL"
fi

if [[ -z "${ARGUS_DATABASE_URL:-}" ]]; then
  fail "Set ARGUS_DATABASE_URL (or DATABASE_URL) before running E2E MVP."
fi

if [[ -z "${ARGUS_MASTER_KEY:-}" ]]; then
  export ARGUS_MASTER_KEY="0123456789abcdef0123456789abcdef"
  warn "ARGUS_MASTER_KEY was not set; using temporary local key for this run."
fi

if [[ "$ARGUS_MASTER_KEY" == *"replace-with"* ]]; then
  fail "ARGUS_MASTER_KEY still uses placeholder value. Set a real secret."
fi

PROMPT="${1:-Create a Rust hello-world project with one test, run tests, and commit changes}"
WORKSPACE="${2:-/tmp/argus-mvp-$(date +%s)}"

echo "Argus MVP E2E"
echo "repo: $ROOT_DIR"
echo "workspace: $WORKSPACE"
echo "prompt: $PROMPT"
echo

cd "$ROOT_DIR"

echo "1) Applying migrations"
cargo run -p argus-cli -- migrate
ok "migrations completed"

echo
echo "2) Running MVP flow"
MVP_OUTPUT="$(cargo run -p argus-cli -- mvp --prompt "$PROMPT" --workspace "$WORKSPACE" 2>&1)"
printf "%s\n" "$MVP_OUTPUT"

SUMMARY_LINE="$(printf "%s\n" "$MVP_OUTPUT" | grep -E "MVP completed: tasks=[0-9]+, completed=[0-9]+" | tail -n 1 || true)"
if [[ -z "$SUMMARY_LINE" ]]; then
  fail "MVP output missing completion summary"
fi

if [[ "$SUMMARY_LINE" =~ tasks=([0-9]+),[[:space:]]completed=([0-9]+) ]]; then
  TASKS="${BASH_REMATCH[1]}"
  COMPLETED="${BASH_REMATCH[2]}"
else
  fail "Unable to parse MVP summary: $SUMMARY_LINE"
fi

if [[ "$TASKS" -eq 0 ]]; then
  fail "MVP produced zero tasks"
fi

if [[ "$COMPLETED" -lt "$TASKS" ]]; then
  fail "MVP incomplete: tasks=$TASKS completed=$COMPLETED"
fi

[[ -f "$WORKSPACE/Cargo.toml" ]] || fail "Expected generated Cargo.toml in workspace"
[[ -f "$WORKSPACE/src/main.rs" ]] || fail "Expected generated src/main.rs in workspace"

[[ -d "$WORKSPACE/.git" ]] || fail "Expected workspace to be a git repository"
if ! git -C "$WORKSPACE" log --oneline -1 >/dev/null 2>&1; then
  fail "Expected at least one git commit in workspace"
fi
ok "git commit created: $(git -C "$WORKSPACE" log --oneline -1)"

ok "mvp flow completed"

echo
echo "MVP E2E succeeded"
echo "workspace output: $WORKSPACE"
