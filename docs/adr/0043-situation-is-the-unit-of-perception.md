# ADR-0043: The Situation Is the Unit of Perception

- **Status:** Accepted
- **Date:** 2026-10-10

## Context

The brain reasons from exactly one situation type — failed systemd units —
while the vigilance machinery already reads memory, load, CPU, and vmstat
every tick (spec 003's kernel-native observation loop) and spec 008's
graduated authority can act on any situation a promoted runbook declares.
The gap is perception, not capability: runbooks authored for
`memory-pressure` or `disk-pressure` can neither earn gates nor drive
procedures because the brain never perceives those situations. Spec 011
closes the organism arc: perception → runbooks → procedures → gates →
graduated authority.

## Decision

1. **Typed situations with stable signatures.** A situation is
   `(signature, subject, readings)` — `host-health` (failed units, today),
   `memory-pressure` and `disk-pressure` (live kernel readings at
   configured threshold crossings, statvfs/meminfo — no new dependencies,
   no command execution). Signatures are the vocabulary runbooks declare
   against; the set is open by construction (a new situation kind is a new
   collector, nothing else changes).
2. **Threshold crossings, not baselines, are the first situation class.**
   Deterministic, config-clamped, no latching: the situation exists while
   the reading crosses and vanishes when it does not. Baseline/deviation
   situations (the BaselineManager) remain a future class — this ADR fixes
   the *shape* (typed collector → signature), not the exhaustive list.
3. **Per-situation reasoning.** Evidence, dedup, procedure plans, episodes,
   and procedures key on the situation's signature and subject. The
   provider sees all situations' evidence in one request when no procedure
   plan fires. Failed units keep the `host-health` signature and their
   exact today behavior; the default configuration observes exactly as
   today.
4. **The situation's reading is the evidence.** Pressure situations carry
   their readings (`memory-pressure: mem at 91.4%`) — an operator auditing
   a procedure plan sees the number that triggered it. Nothing is inferred
   beyond the reading and the threshold.

## Alternatives considered

- **Baseline-deviation situations first** — rejected as the opening class:
  deviations need warmed baselines and tuned sensitivity before they name a
  situation honestly; thresholds are deterministic from the first tick and
  cover the two most operable pressure kinds.
- **A generic key-value situation bus** (any component emits situations) —
  rejected for now: one collector with three typed situations keeps the
  honesty story concrete; the bus is the natural generalization once a
  second emitter exists.
- **Hysteresis/latching** — deferred: deterministic crossings first; add
  hysteresis when flapping is observed, not before.

## Consequences

- `argus-sensors` gains a statvfs-based disk-usage reader (kernel-native).
- The brain's cycle iterates situations; episodes and procedures record
  per-signature symptoms — the episode vocabulary grows from one string to
  the situation set (old `host-health` episodes remain valid evidence).
- Runbook authors gain real triggers; the promotion ladder, procedure
  plans, and graduated authority apply unchanged to the new signatures.
- The sentinel's pressure section is fed from the same collector the brain
  reads — one perception, two consumers, never two truths.
