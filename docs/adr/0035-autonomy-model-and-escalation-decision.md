# ADR-0035: Autonomy Model L0–L5 and the Escalation Decision

- **Status:** Accepted
- **Date:** 2026-09-30

## Context

spec-002 defines three autonomy modes (`observe-only`, `propose`, `assisted`).
The Autonomous Operations Brain requests a richer model — L0 Observe through L5
Adaptive — plus an explicit per-incident escalation decision (OBSERVE / EXPLAIN /
RECOMMEND / ASK-HUMAN / AUTO-FIX). The two must be reconciled without weakening
the "AI output is data, never authority" invariant or granting higher levels
more privilege.

## Decision

### 1. L0–L5 is the canonical autonomy model

`AutonomyMode` becomes L0 Observe, L1 Explain, L2 Recommend, L3 Assisted, L4
Autonomous, L5 Adaptive. This supersedes spec-002's three modes with the mapping:
L0 = observe-only, L2 = propose (recommend), L3 = assisted. L1 (explain-only),
L4 (bounded autonomous execution), and L5 (adaptive with gated learning) are new.

### 2. Higher level = more autonomy, never more privilege

Raising the level changes *what may run automatically*, never *what the runtime
may touch*. The security boundary, policy engine, executor, validation, and
audit are identical at every level. A capability's risk class and blast radius
still bound what L4/L5 may do; high-risk or irreversible capabilities remain
deny-by-default or require approval at every level.

### 3. The escalation decision is per-incident and bounded by the level

The escalation decision (OBSERVE / EXPLAIN / RECOMMEND / ASK-HUMAN / AUTO-FIX) is
a deterministic function over evidence quality, confidence, reversibility, blast
radius, criticality, environment, historical success, autonomy mode, and policy.
The configured level bounds the maximum escalation (e.g. L2 never reaches
AUTO-FIX). Policy is final: a `deny` or `require approval` always wins over any
escalation.

### 4. The level defaults to the most conservative setting

L0 is the default until an operator raises it explicitly.

## Consequences

### Positive

- A single, principled autonomy ladder with a clear, testable escalation
  function.
- No privilege creep at higher levels; audit/validation strengthen, not weaken.
- Reconciles spec-002 and the new request in one place.

### Negative

- A migration of `AutonomyMode` from three variants to six; existing code/tests
  reference the old variants.

## Implementation Principles

1. Autonomy bounds action, never privilege.
2. Escalation is bounded by the configured level; policy is final.
3. Default conservative; raising a level is an explicit operator decision.
4. Keep every level deterministic and testable without a live model.

## Reference

- `specs/002-ai-runtime/spec.md` — FR-007 (old autonomy modes).
- `_bmad-output/specs/spec-autonomous-operations-brain/SPEC.md` — CAP-22, CAP-23.
- `_bmad-output/specs/spec-autonomous-operations-brain/state-machines.md` — the
  escalation decision and level ladder.
- `crates/argus-domain/src/reasoning.rs` (AutonomyMode), `crates/argus-policy`.
