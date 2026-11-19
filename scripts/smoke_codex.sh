#!/usr/bin/env bash
set -euo pipefail
printf '警告：将消耗 Codex 订阅额度\n'
read -r -p '输入 yes 才继续：' answer
[[ "$answer" == yes ]] || exit 1

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
smoke_dir="$(mktemp -d /tmp/argus-codex-smoke.XXXXXX)"
smoke_db="argus_smoke_test_$(python3 -c 'import uuid; print(uuid.uuid4().hex)')"
daemon_pid=''
cleanup() {
  if [[ -n "$daemon_pid" ]]; then kill "$daemon_pid" 2>/dev/null || true; wait "$daemon_pid" || true; fi
  printf '保留测试目录：%s\n保留测试库：%s\n' "$smoke_dir" "$smoke_db"
}
trap cleanup EXIT
[[ "$(docker inspect --format '{{(index (index .NetworkSettings.Ports "5432/tcp") 0).HostIp}}:{{(index (index .NetworkSettings.Ports "5432/tcp") 0).HostPort}}' argus-test-db)" == 127.0.0.1:55432 ]]
export PGPASSWORD=argus_test_only_20261013
docker exec -e PGPASSWORD="$PGPASSWORD" argus-test-db createdb -U argus_test "$smoke_db"
export DATABASE_URL="postgres://argus_test:$PGPASSWORD@127.0.0.1:55432/$smoke_db"
export ARGUS_DATABASE_URL="$DATABASE_URL" ARGUS_DEFAULT_WORKER=codex ARGUS_ENV=development ARGUS_ACTOR_ROLE=admin
export ARGUS_MASTER_KEY="$(python3 -c 'import secrets; print(secrets.token_hex(16))')"
export ARGUS_HTTP_ADMIN_TOKEN="$ARGUS_MASTER_KEY"
export ARGUS_WORKSPACE_ROOT="$smoke_dir/canonical" ARGUS_WORKTREE_ROOT="$smoke_dir/worktrees"
export ARGUS_SERVER_HOST=127.0.0.1
export ARGUS_SERVER_PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')"
unset ARGUS_RUN_REAL_AGENT_TESTS ARGUS_AGENT_BINARY_OVERRIDE ARGUS_PROVIDERS_CONFIG_PATH
cp -R "$repo_root/examples/tiny-rust-app" "$smoke_dir/seed"
git -C "$smoke_dir/seed" init -q
git -C "$smoke_dir/seed" add .
git -C "$smoke_dir/seed" -c user.name=argus-smoke -c user.email=smoke@example.invalid commit -qm fixture
baseline="$(git -C "$smoke_dir/seed" rev-parse HEAD)"
printf '' > "$smoke_dir/argus.toml"
cd "$repo_root"
cargo build -p argus-cli
"$repo_root/target/debug/argus" --config "$smoke_dir/argus.toml" migrate
cd "$smoke_dir"
"$repo_root/target/debug/argus" --config "$smoke_dir/argus.toml" run > "$smoke_dir/daemon.log" 2>&1 &
daemon_pid=$!
base="http://127.0.0.1:$ARGUS_SERVER_PORT"
api() { curl -fsS -H "Authorization: Bearer $ARGUS_HTTP_ADMIN_TOKEN" -H 'Content-Type: application/json' "$@"; }
ready=false
for _ in {1..60}; do
  if api "$base/api/status" >/dev/null 2>&1; then ready=true; break; fi
  kill -0 "$daemon_pid" 2>/dev/null || { cat "$smoke_dir/daemon.log"; exit 1; }
  sleep 1
done
[[ "$ready" == true ]] || { cat "$smoke_dir/daemon.log"; exit 1; }
project_id="$(api -X POST "$base/api/projects" -d '{"name":"codex-smoke","language":"rust"}' | python3 -c 'import json,sys,uuid; print(uuid.UUID(json.load(sys.stdin)["id"]))')"
mkdir -p "$ARGUS_WORKSPACE_ROOT/$project_id"
cp -R "$smoke_dir/seed/." "$ARGUS_WORKSPACE_ROOT/$project_id/"
task_id="$(api -X POST "$base/api/tasks" -d "{\"project_id\":\"$project_id\",\"title\":\"Add one unit test for health()\",\"description\":\"Add one test verifying health() returns a nonempty string. Preserve existing code and tests. Do not commit changes.\",\"test_commands\":[\"cargo test\"],\"acceptance_criteria\":[\"The new test passes and existing function implementations are not modified\"]}" | python3 -c 'import json,sys,uuid; print(uuid.UUID(json.load(sys.stdin)["id"]))')"
poll_deadline=$((SECONDS + 2400))
task='null'
poll_failed() {
  printf '%s\n最后一次任务 JSON：%s\n' "$1" "$task" >&2
  tail -n 50 "$smoke_dir/daemon.log" >&2
  exit 1
}
while true; do
  kill -0 "$daemon_pid" 2>/dev/null || poll_failed 'daemon 进程已退出'
  remaining=$((poll_deadline - SECONDS))
  ((remaining > 0)) || poll_failed '任务轮询超时（2400 秒）'
  if ! next_task="$(api --max-time "$remaining" "$base/api/tasks/$task_id")"; then
    kill -0 "$daemon_pid" 2>/dev/null || poll_failed 'daemon 进程已退出'
    ((SECONDS < poll_deadline)) || poll_failed '任务轮询超时（2400 秒）'
    poll_failed '读取任务状态失败'
  fi
  task="$next_task"
  ((SECONDS < poll_deadline)) || poll_failed '任务轮询超时（2400 秒）'
  status="$(printf '%s' "$task" | python3 -c 'import json,sys; print(json.load(sys.stdin)["status"])')"
  case "$status" in completed|failed|timed_out|cancelled) break ;; esac
  sleep 1
done
printf '任务状态：%s\n' "$task"
docker exec -e PGPASSWORD="$PGPASSWORD" argus-test-db psql -U argus_test -d "$smoke_db" -X -c "SELECT commit_sha AS checkpoint_commit FROM task_artifacts WHERE task_id='$task_id'; SELECT agent_type,success,cost_usd,attempt_number,error FROM llm_usage WHERE task_id='$task_id' ORDER BY created_at;"
worktree="$(docker exec -e PGPASSWORD="$PGPASSWORD" argus-test-db psql -U argus_test -d "$smoke_db" -X -Atc "SELECT worktree_path FROM task_workspaces WHERE task_id='$task_id' ORDER BY created_at DESC LIMIT 1")"
if [[ -n "$worktree" ]]; then git -C "$worktree" diff --stat "$baseline"; fi
[[ "$status" == completed ]]
