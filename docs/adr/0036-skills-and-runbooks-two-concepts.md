# ADR-0036: Skills and Runbooks Are Two Distinct Concepts

- **Status:** Accepted
- **Date:** 2026-09-30

## Context

`spec-argus` CAP-15 defines a **skill runtime** (`argus-skill`): versioned,
declarative skills that *propose plan shapes* and can never execute or
authorize. The Autonomous Operations Brain CAP-17 requests **skills/runbooks**
that carry `allowed actions`, `rollback`, `validation`, and a `historical
success rate` — a richer, action-adjacent concept. Collapsing these into one
concept would either grant the plan-shape skill execution semantics (violating
CAP-15) or strip the runbook of its allowed-action catalog (violating CAP-17).

## Decision

### 1. Two concepts, two crates, two names

- **Skill** (`argus-skill`, existing spec-argus CAP-15): a declarative proposer
  of *plan shapes*. It never enumerates executable actions and never executes or
  authorizes anything. Selection is a `choice` over an applicability-filtered
  catalogue with a first-class `noul`.
- **Runbook** (`argus-runbooks`, new CAP-17): a declarative operational
  procedure with `trigger`, `required_evidence`, `investigation_steps`,
  `decision_criteria`, `allowed_actions`, `rollback`, `validation`, and
  `historical_success_rate`.

### 2. A runbook selects candidate actions; it never grants authority

A runbook's `allowed_actions` are *candidate* typed capabilities, not
permissions. Selecting a runbook yields a candidate action sequence that still
crosses policy and the typed executor exactly like any other plan. A runbook
cannot authorize, and its `allowed_actions` cannot widen the capability set
beyond what policy already permits.

### 3. Learned procedures are gated

A learned skill or runbook (from incident history) is a candidate until it
passes evaluation → simulation → validation → policy → approval → promotion. The
learning pass (separate, read-only — ADR-0031) proposes candidates; it never
self-promotes, and it never modifies a privileged policy.

## Consequences

### Positive

- No collision between "propose a plan shape" and "run a procedure."
- Runbooks remain useful (they encode operator procedure) without weakening the
  authority boundary.
- A clear promotion gate prevents uncontrolled self-modification.

### Negative

- Two procedural artifacts to document and version rather than one.
- Consumers must not conflate them (naming discipline).

## Implementation Principles

1. A skill proposes plan shapes and never executes.
2. A runbook lists candidate actions; authority stays with policy + executor.
3. Learned procedures are gated through evaluation → simulation → validation →
   policy → approval → promotion.
4. Neither concept bypasses the typed-executor boundary.

## Reference

- `_bmad-output/specs/spec-argus/SPEC.md` — CAP-15 (skill runtime).
- `_bmad-output/specs/spec-argus/skills.md` — the plan-shape skill contract.
- `_bmad-output/specs/spec-autonomous-operations-brain/SPEC.md` — CAP-17.
- `_bmad-output/specs/spec-autonomous-operations-brain/glossary.md` — Runbook.
- `docs/adr/0031-validator-and-learning-pass.md` — the separate, read-only
  learning pass.
