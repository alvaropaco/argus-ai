# ADR-0040: Autonomy Is Earned, Bounded, and Revocable

- **Status:** Accepted
- **Date:** 2026-10-10

## Context

The L0–L5 autonomy modes are a static operator dial: `may_execute_without_approval`
maps (mode × risk class) to a boolean, `brain.autonomy` applies it live, and
spec 006 let the cloud move it remotely. Nothing verifies the daemon knows the
environment before acting in it, nothing bounds how much it may do per period,
and nothing lowers the dial when it misbehaves. The owner's model — a god who
sets the law, a creature with free will inside it — requires authority to be
*earned* per environment, *bounded* per period, and *revocable* without the
god's constant attention (spec 008). The trust ladder from specs 006/007
(approval → visibility → autonomy) is the sequence that makes this safe.

## Decision

### 1. The managed ceiling and an earned rung are two different numbers

`brain.autonomy` (managed config, live-applied) remains the **ceiling**: the
most the operator is willing to grant, never a command to act at that level.
The daemon maintains an **earned rung**, promoted rung-by-rung through clean
cycles (gates: provider healthy, no unresolved critical incident, no SafeMode,
no failed validations, cloud paired) and demoted automatically on failure
signatures (SafeMode → L0; failed-validation-with-rollback or provider
degradation → −1 rung). Every consumer reads the **effective autonomy** =
`min(ceiling, earned)`. The operator's dial therefore means "how far may this
instance grow", not "act at this level now" — a semantics change recorded
here.

### 2. A blast-radius budget bounds earned authority per period

Rung gates prove behavior quality; budgets bound quantity. Before any
auto-execution, the plan loop checks a rolling counter per risk class and
scope (`low_risk: 20/hour`, `controlled: 5/day`, `host-scope: 3/day` by
default). Exhaustion converts the step to `requires_approval` (`budget.exhausted`)
— a pause that surfaces in the approval flow, never a silent denial and never
an execution. Counters persist across restarts.

### 3. The ladder skips L1 and stops at L4

L1 (explain) is a labeling mode, not a trust rung; the promotion ladder runs
L2 → L3 → L4. L5 (learning) is reachable only by explicit operator grant:
candidates that write policy are a different risk class of authority entirely.

### 4. Fail-closed by construction

Any error in the state machine, budget evaluator, or their persistence
degrades the effective autonomy (treat the gate as failed, the budget as
exhausted), never the reverse. The production default is unchanged: absent an
operator-set ceiling, a daemon sits at L0 forever — spec 008 builds the
machinery; the god decides how much authority to grant.

## Alternatives considered

- **Trust-on-config (status quo)** — rejected: handing L3+ on faith is the
  failure mode this spec exists to close, and L0-forever forfeits the product.
- **Time-based promotion only** — simpler, but clean *cycles* (work actually
  done and validated) are the evidence that matters; a quiet host proves
  nothing.
- **Cloud-side state machine** — rejected: it would put authority decisions on
  the far side of a flaky link; assimilation is a property of the host and its
  local evidence (constitution: the boundary is local).
- **Per-capability budgets** — deferred: risk-class × scope granularity
  covers the blast-radius concern without freezing policy to a capability
  list.

## Consequences

- `brain.autonomy` UI copy and docs must say "ceiling"; the sentinel report
  and dashboard show ceiling vs earned vs effective (spec 008 FR-005).
- Promotions/demotions are ledger traces and domain events — auditable via the
  007 stream, no new wire kinds.
- Re-earning after demotion is automatic through the same gates; the operator
  intervenes only where they always did: the ceiling and the approval queue.
