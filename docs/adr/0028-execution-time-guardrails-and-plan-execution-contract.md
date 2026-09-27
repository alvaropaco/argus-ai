# ADR-0028: Execution-Time Guardrails and the Plan Execution Contract

- **Status:** Accepted
- **Date:** 2026-09-27

## Context

Story 1.7 must add per-capability execution-time guardrails, a plan execution
state machine with fail-stop and per-step rollback, and idempotency.

Investigation shows the executor does **not** expose the hook guardrails need:
`Executor::execute(&AuthorizedAction)` is the sole execution boundary
(`crates/argus-executor/src/executor.rs`), but `AuthorizedAction` carries only
`CapabilityRequest` + `PolicyDecision` and never the `CapabilityDescriptor`
(`crates/argus-executor/src/action.rs`); there is no guardrail field, hook, or
live-state read; `SystemdServiceController` performs no target check
(`crates/argus-executor/src/service.rs`). The plan executor
`control::authorize_and_run` (`crates/argus-daemon/src/control.rs`) loops all
actions with no fail-stop, rollback, or status transition and has no production
caller; `Plan.rollback` is free text (`crates/argus-domain/src/reasoning.rs`).
There is no plan/execution persistence, no loop driver, and no dedup anywhere.

The epic recorded this as an Unknown ("whether the executor's existing policy
boundary already exposes the hook the guardrail predicates need") — it does not.
Changing execution and plan contracts touches stable contracts (Constitution
Principle 15), so the decisions are recorded here.

## Decision

### 1. Guardrails are a per-capability predicate registry in the executor

A `Guardrail` predicate registry keyed by `CapabilityId`, owned by
`argus-executor`, is consulted inside the privileged executor **immediately
before the effect**. The executor is the single execution boundary (ADR-0021
§4), and keying by capability avoids changing `AuthorizedAction` /
`CapabilityRequest`. The daemon wires the registered predicates when it builds
the executor. Rejected: carrying the descriptor on `AuthorizedAction` (changes a
stable contract) and a guardrail-decorator `Executor` (indirection without a
second purpose).

### 2. The target is re-resolved at execution time (TOCTOU)

Immediately before the effect the target is re-derived from live state, and the
predicate runs on that value — never on a plan-time value.

### 3. Milestone-1 guardrails

- `never_target_argusd` — the daemon's own unit is refused.
- `valid_service_unit` — non-empty and a valid unit name (no control/whitespace).
- `allowed_targets` — the target must be a member of a daemon-supplied allowed
  set; when no set is configured the predicate is not registered.
- `pid_above_one` — a reusable predicate for a future `host.process.signal`,
  unit-tested in isolation (milestone 1 exposes no process capability, per SPEC).

A violation is a distinct, non-executing failure, never a silent skip.

### 4. The plan execution contract

- `PlanStatus` gains `RollingBack` and `NeedsManual`; the spec vocabulary maps
  onto it (Proposed ≈ pending, Executing = in-progress, Completed = succeeded,
  Failed = failed, RolledBack = rolled-back, plus `RollingBack`, `NeedsManual`).
  Per-step status is tracked.
- Fail-stop by default.
- Every step carries a **declarative rollback action**: `Plan.actions` +
  `Plan.rollback: Option<String>` become `Plan.steps: Vec<PlanStep>` with
  `PlanStep { action: Action, rollback: Option<Action> }` (a core-contract change
  recorded here).
- The planner never substitutes actions mid-failure.

### 5. Idempotency

- **Event dedup** — a repeated observation (same deterministic dedup key) does
  not spawn a new plan or execution.
- **Action idempotency** — desired state is checked before acting; "already in
  the desired state" is a recorded success no-op.

### 6. Scope boundary

1.7 delivers these mechanisms with tests. **Explicitly out of scope** (recorded
follow-ups): the production event-driven + periodic-tick loop driver; the
per-target execution lock (reuse `PrivilegedLimiter`); plan/execution
persistence.

## Consequences

### Positive

- The guardrail hook lives at the one execution boundary and adds no public
  surface to `AuthorizedAction`/`CapabilityRequest`.
- Fail-stop, rollback, and the state-machine vocabulary become explicit and
  testable without a live model.
- Idempotency prevents duplicate plans from repeated observations.

### Negative

- `Plan` changes shape (`steps` + declarative per-step rollback), a stable
  contract change; callers constructing `Plan` must migrate.
- Guardrails add a live-state read at execution time, so the executor gains a
  dependency/failure mode it did not have.
- The production loop driver and per-target lock remain unwired, so the loop is
  still exercised only through the test seam.

## Implementation Principles

1. Run guardrails at execution time on the re-resolved target; never on a
   plan-time value.
2. Fail closed: a guardrail violation or a failed step never silently continues.
3. Check desired state before acting; "already desired" is a success no-op.
4. Deduplicate repeated observations by a deterministic key before planning.
5. Route every effect through the executor boundary; reversal crosses the same
   boundary.

## Reference

- `_bmad-output/specs/spec-argus/tool-calling.md` — guardrails, plan state
  machine, idempotency, re-entrancy.
- `_bmad-output/specs/spec-argus/SPEC.md` — CAP-4, CAP-16; constraints on
  fail-stop, per-action idempotency, per-step rollback, guardrail predicates.
- `argus-ai/.specify/memory/constitution.md` — Principles 3, 9, 10, 14, 15.
- `argus-ai/docs/adr/0010-policy-engine.md`,
  `argus-ai/docs/adr/0016-plugin-architecture.md`,
  `argus-ai/docs/adr/0020-external-privileged-operations-authorization.md`,
  `argus-ai/docs/adr/0021-privileged-execution-and-privilege-declaration.md`.
- Code: `argus-ai/crates/argus-executor/src/{executor.rs,action.rs,privileged.rs,service.rs}`,
  `argus-ai/crates/argus-domain/src/reasoning.rs`,
  `argus-ai/crates/argus-daemon/src/{control.rs,runtime.rs,privileged.rs}`.
