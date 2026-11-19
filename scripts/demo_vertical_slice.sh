#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

ok() { printf "[OK] %s\n" "$1"; }

MODE="${1:---offline}"
PROMPT="${2:-Create a Rust hello-world project with one test, run tests, and commit changes}"
WORKSPACE="${3:-/tmp/argus-demo-$(date +%s)}"

cd "$ROOT_DIR"

case "$MODE" in
  --offline)
    echo "Argus vertical slice demo (offline alpha)"
    echo "repo: $ROOT_DIR"
    echo "mode: in-process MVP test"
    echo
    cargo test -p argus-core mvp::tests::test_mvp_orchestrator_commits_workspace -- --exact --nocapture
    ok "offline MVP loop passed"
    ;;

  --with-db)
    echo "Argus vertical slice demo (database-backed CLI)"
    echo "repo: $ROOT_DIR"
    echo "workspace: $WORKSPACE"
    echo "prompt: $PROMPT"
    echo
    "$ROOT_DIR/scripts/e2e_mvp.sh" "$PROMPT" "$WORKSPACE"
    ok "database-backed MVP loop passed"
    ;;

  *)
    cat >&2 <<USAGE
Usage:
  ./scripts/demo_vertical_slice.sh
  ./scripts/demo_vertical_slice.sh --offline
  ./scripts/demo_vertical_slice.sh --with-db [prompt] [workspace]

The offline mode does not require PostgreSQL or LLM credentials.
USAGE
    exit 2
    ;;
esac
