# ADR-0022: Cloud-Issued Operation Semantics — Idempotency, Timeout, Serialization, and Reversal

- **Status:** Accepted
- **Date:** 2026-09-21

## Context

The installation ↔ cloud channel is a reconnectable socket with at-least-once
delivery. Messages can be re-delivered, and they can be re-delivered *while the
first attempt is still running*. The process can also die mid-operation, after a
privileged change has already taken effect.

Privileged operations are frequently not idempotent — restarting a service twice is
not the same as restarting it once. So the usual "retry until acknowledged" instinct
is actively unsafe here, and "re-report the stored result" is undefined when no
result exists yet.

ADR-0020 governs *whether* an operation may run. This ADR governs what happens when
delivery, timing, or failure goes wrong around it.

## Decision

### 1. Idempotency is keyed on `command_id`

Every invocation carries a `command_id` that doubles as its idempotency key. The
command row is committed as `executing` **before** the operation begins, not after
it completes. A crash between "operation started" and "row written" would otherwise
erase all trace of a privileged change that may already have taken effect.

### 2. Re-delivery of a terminal command re-reports; it never re-executes

If a `command_id` is already in a terminal state (`refused`, `acknowledged`,
`failed`), the installation re-sends the stored result and does not execute again.
This is what makes at-least-once delivery safe.

### 3. Re-delivery while still running is suppressed silently

If a `command_id` is still `received` or `executing`, the duplicate is suppressed:
it is **not** re-executed, and it is **not** given a result, because no result
exists yet and emitting one would be a fabrication. The in-flight attempt reports
when it finishes.

This case is the reason rule 2 alone is insufficient, and it was found by review
during planning rather than in production.

### 4. Crash-interrupted commands are resolved, never dropped

On startup, every command row found in `executing` is resolved to `failed` with an
explicit **unknown-state** reason and reported to the cloud once the connection is
established.

Silence is not an acceptable outcome: an operator must be told that a privileged
change of unknown outcome may have occurred. "Outcome unknown, verify manually" is
the only honest answer.

### 5. Every invocation is bounded by a timeout

On expiry the invocation is reported as `failed` with a timeout reason — never as
`acknowledged`. The operation MUST NOT be left indeterminate: the implementation
either confirms the change did not take effect or reports that its state is unknown,
and the latter is surfaced explicitly in the report. A timeout never drops or
corrupts the connection.

### 6. Privileged work is capped and serialized per resource

At most `max_concurrent_privileged` privileged invocations run concurrently.
Invocations targeting the same resource are serialized, so two conflicting
operations on one resource never interleave. Serialization is per resource, not
global, so unrelated work is not needlessly blocked. A queued invocation that
exceeds its wait budget is reported as `failed`, never silently dropped.

### 7. Reversal is explicit, bounded, and reported precisely

When a capability declares itself reversible and the operation fails *after* taking
effect, the installation attempts reversal. The reversal crosses the same policy
and executor boundary as the original action (ADR-0021 §7).

The reported outcome distinguishes three cases: reversed successfully, reversal
attempted and failed, and reversal not possible. A failed reversal is reported as
`failed` with the reversal status explicit — never as `acknowledged`. Irreversible
capabilities are never reversed; their failure is reported for human follow-up.

## Consequences

### Positive

- At-least-once delivery cannot cause a privileged side effect to run twice.
- A crash during a privileged operation produces a reported, actionable unknown
  state rather than silence.
- Operators can distinguish "did not run", "ran and failed", and "ran, outcome
  unknown" — which is the difference between a safe retry and a dangerous one.
- Conflicting operations on one resource cannot interleave.

### Negative

- The write-ahead commitment adds a persistence round trip before each privileged
  operation; accepted as the cost of recoverability.
- Per-resource serialization can delay an operation behind an unrelated one on the
  same resource; accepted as the cost of determinism.
- Reversal is best-effort: a failed reversal still leaves the environment changed
  and is reported as such rather than retried indefinitely.

## Implementation Principles

1. Treat every invocation as idempotent by `command_id`, and enforce it before
   execution rather than after.
2. Commit `executing` before the side effect, never after.
3. Suppress in-flight duplicates; never fabricate a result.
4. Resolve interrupted commands to a reported unknown state on startup.
5. Bound every invocation; report timeouts as failures, never as successes.
6. Cap concurrency and serialize per resource.
7. Report reversal outcomes precisely, and route reversal through policy.

## Reference

- `specs/001-argus-cloud-sync/contracts/privileged-execution.md` §2 (checks 1a/1b/1c
  and interrupted commands), §5, §6, §7, §11
- `specs/001-argus-cloud-sync/spec.md` FR-046, FR-047, FR-048; SC-007, SC-016, SC-018
- `specs/001-argus-cloud-sync/research.md` R15
- ADR-0020, ADR-0021
- Constitution Principles 3, 10, 15, 16
