# Architecture Decision Records (ADR)

This directory is the durable record of *why* ARGUS is built the way it is —
so a decision's rationale survives past whatever chat session produced it.

An ADR is not a design doc and not a roadmap entry. It records one decision,
the alternatives that were rejected, and why. [`docs/MASTER_ROADMAP.md`](../MASTER_ROADMAP.md)
says *what* is planned; [`docs/ARCHITECTURE.md`](../ARCHITECTURE.md) says
*how the system is organized*; an ADR says *why a specific choice was made*
and is expected to still make sense years later, independent of either.

## When to write one

Write an ADR when a decision:

- constrains future work (something later code must not violate), or
- was genuinely contested (a real alternative existed and was rejected), or
- would otherwise only exist in a chat transcript or a commit message nobody
  will re-read.

Don't write one for routine implementation choices reversible by editing a
function. If in doubt, prefer writing a short one over skipping it — the cost
of an unnecessary ADR is much lower than the cost of a lost decision.

## Format

One file per decision: `docs/adr/NNNN-short-title.md`, numbered sequentially,
never renumbered or reused even if a decision is later superseded.

```markdown
# NNNN. Title

Status: Accepted | Superseded by NNNN | Deprecated

## Context
What situation forced this decision? What were the constraints?

## Decision
What was decided, stated as a rule, not a narrative.

## Alternatives Considered
What else was on the table, and why it was rejected.

## Consequences
What this makes easier, what it makes harder, what it forbids.
```

A superseded ADR is never deleted or edited to look wrong in hindsight — a
new ADR supersedes it and both stay on record.

## Status legend

| Status | Meaning |
|---|---|
| Accepted | In force; code and future decisions should conform to it |
| Superseded by NNNN | No longer in force; see the linked ADR |
| Deprecated | No longer in force; not replaced by a specific successor |

## Register

The decisions below are already in force across the codebase as of V0.38.
They are recorded here as a register — each should eventually get its own
numbered file (`0001-...md` etc.) with full Context/Alternatives/Consequences;
until then, this table is the authoritative summary and must not be treated
as a lesser or informal version of the decision.

| # | Decision | Status |
|---|---|---|
| 1 | **Agent is not Model.** An Agent is a runtime role ARGUS assigns; the model or CLI behind it (Claude, Codex CLI, OpenCode, ...) is an implementation detail the Agent abstraction hides. Code must never hardcode a specific model where "an Agent" is meant. | Accepted |
| 2 | **Project State is source of truth.** Git history, the database (tasks, workspaces, spend ledger), and verified artifacts are authoritative. A conversation, an Agent's self-report, or a chat transcript is evidence to be checked against this state, never a substitute for it. | Accepted |
| 3 | **Worktree isolation.** Every task execution runs in its own Git worktree, verified before the Agent is spawned (ownership, branch, path safety, base revision). The canonical per-project repository is never used as an Agent's working directory and is never modified by an Agent directly. | Accepted |
| 4 | **Spend Ledger as accounting truth.** Realized cost lives in the persistent spend ledger (`llm_usage`), not in memory and not in an Agent's self-reported usage. If a spend write fails, the system fails closed (`AccountingState::Degraded`) rather than let paid execution continue against an unreliable ledger. | Accepted |
| 5 | **Budget blocked is not execution failure.** A task deferred by an admission gate (budget exhausted, accounting degraded, workspace not yet ready) is not a task failure: it does not consume a retry attempt, and its workspace is preserved and reused, not recreated, when it becomes schedulable again. | Accepted |
| 6 | **No automatic merge from task worktree.** ARGUS never merges, pushes, or opens a PR from a task's worktree on its own initiative. Producing a verified Git checkpoint of an Agent's work (V0.38 Phase 4) is a distinct, later step from integrating that work into the canonical branch — the two are never conflated. | Accepted |
| 7 | **Subscription-first / API-fallback strategy.** Where a provider officially supports authenticated subscription login (not a scraped or unofficial channel), that is the default; direct API-key usage is a fallback path, and every paid API call is gated by `BudgetService`, never issued directly by an Agent or driver. | Accepted |
| 8 | **Planner provider is replaceable.** The component that decides *what* to do (the Planner) is a pluggable provider behind a protocol boundary (V0.43), exactly like a Runtime provider (V0.44). No planning logic may assume a specific model or vendor. | Accepted |
| 9 | **Task completion requires evidence.** A task's `Completed` state is set only from something independently checkable — a Git checkpoint, a passing Quality Gate, an artifact record — never from an Agent's self-reported "done." "Reported complete" and "verified complete" are distinct states and must not be collapsed into one. | Accepted |
| 10 | **Role, Provider, and Auth are separate axes.** Which role does the work (Planner, Developer, Reviewer, ...), which provider/CLI fills that role (Claude, Codex CLI, OpenCode, ...), and how that provider is authenticated (subscription login, API key, environment credential) vary independently. Code must not conflate any two of these — a provider is not bound to one role, and a role is not bound to one auth method. | Accepted |
| 11 | **Prompt is not a permission boundary.** What an Agent is told not to do in its instructions is guidance, not enforcement. Actual boundaries are enforced in code the Agent cannot talk its way around: `argus-security::rbac` (role → permission), `argus-security::command_guard` (denylisted destructive commands), and worktree/workspace verification (Layer 2). A capability an Agent must not have is one it is not given, not one it is merely asked to avoid using. | Accepted |

## Related documents

- [docs/MASTER_ROADMAP.md](../MASTER_ROADMAP.md) — long-term roadmap and the architecture principles these decisions implement.
- [docs/ARCHITECTURE.md](../ARCHITECTURE.md) — the layered system model these decisions constrain.
- [docs/PROJECT_STATE.md](../PROJECT_STATE.md) — what is actually built today.
