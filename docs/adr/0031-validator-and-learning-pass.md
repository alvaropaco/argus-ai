# ADR-0031: Post-Execution Validator and the Separate Learning Pass

- **Status:** Accepted
- **Date:** 2026-09-27

## Context

Story 1.10 (CAP-4/CAP-14) must re-observe after execution, validate against
desired state, record evidence, and keep learning in a separate pass that cannot
mutate an in-flight plan. Nothing of the sort exists:

- There is no machine-checkable desired state: `Intent`
  (`crates/argus-domain/src/reasoning.rs:26`) is dead code (never constructed and
  missing `id`/`source`), `Plan.expected_outcomes` is `Vec<String>` and
  `propose_plan` (`crates/argus-ai-core/src/decision/gateway.rs:108`) always sets
  it empty, and the only checkable value is the capability→bool map
  `desired_state(capability)` (`crates/argus-daemon/src/control.rs:501`).
- No validator exists; `VALIDATION_PASSED`/`VALIDATION_FAILED`
  (`crates/argus-events/src/lib.rs:28-29`) are never published.
- Observations are write-only: `put_observation` has no read counterpart
  (`crates/argus-state/src/repository.rs:29-46`), and the only live-state read is
  `ServiceController::is_active` (`crates/argus-executor/src/service.rs:27`).
- Learning does not exist, and SPEC.md:127 makes outcome-based skill re-scoring a
  v1 non-goal (deferred to a separate audited batch pass).
- The production execution path is `run_host_health`
  (`crates/argus-ai-core/src/decision/host_health.rs:185`) via `diagnose_with`;
  `control::authorize_and_run`/`resume_and_run` are test-only.
- `brain-architecture.md` names a new `argus-validate` crate.

These choices are architectural, so they are recorded here.

## Decision

### 1. The validator is a new `argus-validate` crate

`argus-validate` owns the deterministic post-execution validator and the separate
learning pass. The validator is a pure function over desired and observed state
(`validate(desired, observed) -> ValidationOutcome`); the daemon supplies the
re-observed state. This keeps the crate free of IO and testable without a host.

### 2. The machine-checkable desired state is the capability-state map

The desired value validated against is `desired_state(capability) -> Option<bool>`
(START/RESTART → active, STOP → inactive), reusing the semantics already in
`control.rs:501`. `Plan.expected_outcomes` stays free-form and is not used for
checks; making it typed, and constructing a first-class `Intent`, is a follow-up.

### 3. Re-observation records an `Observation`

After a step executes, the daemon reads live state through
`ServiceController::is_active` and records it as an `Observation` (with
`Provenance`) via `put_observation`. `DomainRepository` gains `list_observations`
so the validator and the learning pass can read evidence; the SQLite, in-memory,
and LanceDB backends implement it (memory/LanceDB may keep the explicit
`Unavailable` pattern only if they already do).

### 4. Validation publishes the validation events

The validator's outcome is published as `VALIDATION_PASSED` /
`VALIDATION_FAILED` with the payloads in `specs/002-ai-runtime/contracts/events.md`.

### 5. The learning pass is a separate, read-only pass

v1 delivers the separation boundary only: `argus-validate`'s learning pass reads
persisted audit events and execution decisions and emits a deterministic report.
It is given no plan handle, and in-flight plans are reached only as `&Plan` /
`PendingApprovals`, so it cannot mutate an already-authorized plan. Outcome-based
re-scoring remains deferred (SPEC.md:127).

### 6. The validator hooks both execution paths

It runs after each executed step in `control::run_plan` and after the loop in
`run_host_health`, so it executes in production (host-health) and on the plan
path.

## Consequences

### Positive

- Desired-vs-observed validation becomes explicit and testable without a host.
- The learning pass is separated by construction (read-only inputs), honoring
  tool-calling.md's re-entrancy rule.
- The already-defined validation events become live.

### Negative

- A new crate; a new repository read method (and backends must implement it).
- Desired state is still capability-scoped, not a general `Intent`; a rich
  desired-state model remains a follow-up.

## Implementation Principles

1. Validate against a machine-checkable desired value; never infer it from prose.
2. Re-observe through a structured live-state read and record it as evidence.
3. Keep the validator pure; keep the learning pass read-only.
4. Never mutate an in-flight or already-authorized plan.
5. Publish validation outcomes; do not fabricate a pass when the read fails.

## Reference

- `_bmad-output/specs/spec-argus/SPEC.md` — CAP-4, CAP-14; the learning-pass
  constraint and the re-scoring non-goal.
- `_bmad-output/specs/spec-argus/brain-architecture.md` — step 6 validator;
  `argus-validate`.
- `_bmad-output/specs/spec-argus/tool-calling.md` — re-entrancy.
- `argus-ai/specs/002-ai-runtime/contracts/events.md` — validation payloads.
- `argus-ai/docs/adr/0028-execution-time-guardrails-and-plan-execution-contract.md`.
- Code: `crates/argus-ai-core/src/decision/host_health.rs`,
  `crates/argus-daemon/src/control.rs`, `crates/argus-domain/src/{reasoning.rs,observation.rs}`,
  `crates/argus-state/src/repository.rs`, `crates/argus-executor/src/service.rs`,
  `crates/argus-events/src/lib.rs`.
