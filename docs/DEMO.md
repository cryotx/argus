# Demo Guide

Argus has two demo modes.

## Offline Alpha Demo

```bash
./scripts/demo_vertical_slice.sh
```

This mode is intentionally dependency-light. It uses the in-process MVP
orchestrator and does not require a PostgreSQL database or LLM API key.

It proves:

- the scheduler can create and complete a task
- the worker path can execute a shell-generated change
- quality checks can run
- a local git commit can be produced

It does not prove:

- real LLM decomposition
- real coding-agent execution
- PostgreSQL recovery
- PR creation
- production sandboxing

## Database-Backed CLI Demo

```bash
ARGUS_DATABASE_URL=postgres://argus:argus@localhost:5432/argus \
  ./scripts/demo_vertical_slice.sh --with-db
```

This mode runs `argus migrate` and `argus mvp`. It requires the local database
and config to be valid. It still uses the narrow MVP workflow, not the final
real coding-agent PR loop.

## Expected Output

The script prints:

- workspace path
- commands executed
- git history for the generated workspace
- final pass/fail status

The generated workspace is kept under `/tmp` by default for inspection.
