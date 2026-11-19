# Evaluation Harness — V0.45 foundation

ARGUS distinguishes an Agent's reported completion, independently verified
execution, and explicit operator acceptance. The harness reads the database
and Git; the caller cannot submit a passing result or a command to the evaluator.

## Verification policy

Both built-in evaluators are required:

- `task_artifacts`: a nonempty definition of done, at least one linked task,
  every task completed, matching project/task/execution/workspace ownership,
  a persisted passing quality gate, and independently run test commands with
  passing test evidence. Notes and Agent reports cannot substitute for this.
- `git_checkpoints`: actual Git commit objects, ancestry from the recorded
  base, checkpoint ownership trailers, and an exact nonempty changed-file set
  matching the artifact. It runs only fixed, read-only Git commands, with a timeout.

An additional `Evaluator` can be registered with `with_evaluator`; it is
required and cannot replace the built-in checks. Outcomes are `passed`,
`failed`, or `indeterminate`. Missing evidence or an unavailable verifier is
never a pass. The v1 checks establish execution and test evidence; they do
not interpret prose DoD or substitute for a semantic Reviewer, lint, or build
evaluator. Test commands must express the actual acceptance contract.

## State and persistence

`work_item_evaluations` stores each report, input hash, version, per-check
result and artifact references. Reports are append-only. A passing report,
artifact links, WorkItem promotion to `VerifiedComplete`, activity and project
revision commit in one transaction. A failed re-evaluation of previously
complete work returns it to `ReportedComplete`. An unverified active item
keeps its current state. Deferred/cancelled work cannot be evaluated.

The production execution loop evaluates linked WorkItems after checkpointed
task completion. A failed persistence call leaves the task artifact intact
and verification can be retried through the API or CLI. Linking additional
tasks invalidates prior completion and increments the project revision.
Project/work-item locks follow the same order as Planner apply.

Acceptance is a separate admin/owner action. It requires a current passing
report, unchanged task/DoD evidence, and successful rechecking. The actor is
taken from authenticated HTTP identity or CLI security, not request JSON.
Acceptance is idempotent for a WorkItem/report pair and is recorded separately.
Historical bootstrap records retain their existing operator note evidence;
the trusted legacy ProjectStateService setter remains for those records.
New runtime verification and these HTTP/CLI workflows use the harness.

## CLI

```bash
argus project link-task <project-slug-or-uuid> <work-item-uuid> <task-uuid>
argus project evaluate <project-slug-or-uuid> <work-item-uuid>
argus project evaluations <project-slug-or-uuid> <work-item-uuid>
ARGUS_ACTOR_ROLE=admin argus project accept <project> <work-item> <evaluation>
```

## HTTP

All paths below are under `/api/projects/{project_id}/work-items/{work_item_id}`:

| Method | Suffix | Behavior |
|---|---|---|
| POST | `/tasks/{task_id}` | Link a task from the same project |
| POST | `/evaluate` | Independently evaluate and persist a report |
| GET | `/evaluations` | Read the latest 100 reports |
| POST | `/accept` | Accept `{"evaluation_id":"<uuid>"}` |

Task creation now accepts `test_commands` and `acceptance_criteria` arrays.
Link a WorkItem before execution for automatic verification; link after
execution and call `/evaluate` for an existing task.

HTTP writes, including `/api/chat`, require a bearer token configured on the
server: `ARGUS_HTTP_ADMIN_TOKEN`, `ARGUS_HTTP_DEVELOPER_TOKEN`, or
`ARGUS_HTTP_VIEWER_TOKEN`. Tokens must be distinct, at least 32 ASCII characters,
and supplied only through the environment. Read requests retain anonymous
viewer access for the local dashboard. A role header cannot grant permission.
Evaluation requires WriteCode and ExecuteCommand; acceptance, provider policy
changes, and plan approval/application require ConfigureSystem. Auth-profile
writes require ManageSecrets. Authorization audits are persisted before any
write handler runs; unavailable audit storage returns 503 without executing it.
Completion audits are best-effort after the handler returns.

`GET /api/config` returns the actual process configuration with credentials
and URLs redacted. Configuration is read-only; `PUT` returns 501. The dashboard
reflects that behavior. `/api/logs` reads persisted audit actions, with level,
source, pagination and bounded limits; it does not return sample logs or raw
process stdout.

## Validation

```bash
ARGUS_DATABASE_URL=<disposable-postgres-url> \
  cargo test -p argus-cli --test evaluation_e2e -- --ignored --test-threads=1
cargo test -p argus-cli http_security
cargo test -p argus-worker sandbox
python3 scripts/check_openapi_contract.py
```

The ten E2E cases use real PostgreSQL, Git worktrees, processes and test
commands. Only the coding-agent process is a deterministic local script;
there are no model calls or API charges. Broader ignored DB suites remain
serialized (DEF-020). CI also checks frontend build/lint, generated endpoint
constants, API route inventory and tracked secret patterns.
