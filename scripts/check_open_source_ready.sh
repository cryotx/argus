#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

fail() { printf "[FAIL] %s\n" "$1"; exit 1; }
ok() { printf "[OK] %s\n" "$1"; }

for required in README.md README.en.md LICENSE CONTRIBUTING.md SECURITY.md .env.example docs/OPEN_SOURCE_STATUS.md openapi/argus.json scripts/demo_vertical_slice.sh; do
  [[ -f "$required" ]] || fail "missing required open-source file: $required"
done
ok "required public files exist"

for private_file in .env config/argus.toml config/providers.toml ARGUS_AUDIT.md; do
  if git ls-files --error-unmatch "$private_file" >/dev/null 2>&1; then
    fail "private runtime or audit file is tracked: $private_file"
  fi
done
ok "private runtime and audit files are not tracked"

# The checked-in shared Claude settings are repository policy, not session state.
if git ls-files | grep -E '(^|/)(\.claude|\.codex)(/|$)' | grep -v -x '.claude/settings.json' >/dev/null; then
  fail "local agent state is tracked"
fi
ok "local agent state is not tracked"

SECRET_MATCHES="$(
  git grep -n -I -E \
    '(sk-[A-Za-z0-9_-]{24,}|sk-sp-[A-Za-z0-9_-]{24,}|ghp_[A-Za-z0-9]{36}|xox[baprs]-[A-Za-z0-9-]{20,}|AKIA[0-9A-Z]{16}|-----BEGIN (RSA |OPENSSH |EC |DSA )?PRIVATE KEY-----)' \
    -- . \
    ':!Cargo.lock' \
    ':!docs/SECURITY.md' \
    ':!crates/argus-security/src/secret_scanner.rs' \
    ':!crates/argus-security/src/lib.rs' \
    ':!crates/argus-mcp/src/router.rs' \
    || true
)"

if [[ -n "$SECRET_MATCHES" ]]; then
  fail "potential hardcoded secret found"
fi
ok "no high-confidence secret patterns found in tracked files"

if [[ "${1:-}" == "--secrets-only" ]]; then
  exit 0
fi

cargo fmt --all -- --check
ok "rustfmt check passed"

cargo check -p argus-core -p argus-llm -p argus-cli -p argus-worker
ok "core Rust checks passed"

(cd web && npm run build)
ok "frontend build passed"

echo
ok "repository passes the alpha open-source readiness check"
