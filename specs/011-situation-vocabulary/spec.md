# Feature Specification: ARGUS Situation Vocabulary — the Brain Perceives More Than Failed Units

**Status:** Draft
**Spec ID:** 011
**Created:** 2026-10-10

> Concludes the organism arc: perception (011) → runbooks (009) → procedures
> (010) → graduated authority (008), all under the vigilance stream (007).
> Spec 010's out-of-scope note — "other triggers climb the moment their
> situations exist" — is this spec. Decision record: ADR-0043 (the situation
> is the unit of perception; signatures are the vocabulary runbooks declare
> against).

## 1. Problem Statement

The brain perceives exactly one situation type: failed systemd units
(`host-health`). Everything else the vigilance machinery already reads —
memory pressure, disk usage, load — never reaches reasoning, so runbooks
authored for any other trigger can neither earn gates nor drive procedures,
and the provider reasons from starved evidence. The organism explores a
world it can only partially perceive.

## 2. Objective

Each brain cycle collects a **typed situation set** — failed units (today)
plus resource-pressure situations read kernel-native at threshold crossings
(memory pressure, disk pressure) — and reasons per situation: evidence,
procedure plans (promoted runbooks match per signature), episodes, and
procedures all key on the situation's signature. The trigger vocabulary
becomes open by construction: a runbook authored for `memory-pressure`
climbs the moment a memory-pressure situation exists.

## 3. Functional Requirements

**FR-001 (Situation collection)** Each brain cycle MUST collect situations
from kernel-native sources: failed systemd units (signature `host-health`,
subject the unit — today's behavior, unchanged) and threshold crossings from
live readings — signature `memory-pressure` (subject `mem`) when
`memory.used_percent ≥ memory_used_percent`, signature `disk-pressure`
(subject the mount, `/` first) when disk usage percent ≥
`disk_used_percent`. Thresholds come from a new `[situations]` config
(`memory_used_percent`/`disk_used_percent`, default 90, clamped 50–99). A
situation is present while the reading crosses the threshold and absent
when it does not — no hysteresis, no latching.

**FR-002 (Per-situation reasoning)** The brain MUST reason per situation:
evidence lines from the situation's readings, a dedup key per (signature,
subject), procedure plans matching promoted runbooks against the
**situation's signature** (the spec-010 mechanism, now signature-generic —
step arguments derive per signature: `host-health` → `{"unit": …}`), and
episodes/procedures recorded under the situation's signature (failed units
keep `host-health`; pressure situations record their own). The provider
receives all situations' evidence in one request when no procedure plan
fires.

**FR-003 (Disk usage reader)** `argus-sensors` MUST gain a disk-usage
reader (statvfs on a mount, percent used, kernel-native, no new deps) used
by the collector; `/` is the default subject.

**FR-004 (Resolution and honesty)** A situation resolves when its reading
drops below the threshold: the cycle records the outcome onto the attributed
runbook and episode exactly as host-health does today; dedup releases so a
recrossing re-reasons. Gate/episode honesty is spec 009/010's: evidence
only, ladder order, never guessed.

**FR-005 (Visibility)** Situations surface additively: the cycle trace's
evidence names the signature and reading (`memory-pressure: mem at 91.4%`),
and the sentinel report's pressure section is fed from the same collector
(risks from pressure situations where policy surfaces them). The web needs
no change beyond what the sentinel already renders.

## 4. Non-Functional Requirements

- Readings are kernel-native (`/proc/meminfo` existing; statvfs for disk)
  with no new dependencies and no command execution.
- Collection adds microseconds per tick; thresholds are checked once per
  cycle; dedup prevents repeat reasoning on a stable crossing.
- Fail-closed: a failed reading contributes no situation and is logged —
  never a fabricated situation, never a missed failed-unit.
- The default configuration observes exactly as today until an operator
  sets thresholds or authors runbooks for the new signatures.

## 5. Acceptance Criteria

- [ ] AC-001 — Given `memory.used_percent` at/above the threshold, when the
      brain cycles, then a `memory-pressure` situation is collected with the
      reading as evidence, and a promoted runbook authored for that
      signature yields an attributed procedure plan.
- [ ] AC-002 — Given the reading below the threshold, when the brain
      cycles, then no pressure situation exists and the cycle behaves as
      today.
- [ ] AC-003 — Given a memory-pressure situation whose attributed procedure
      executes and validates, when the outcome lands, then the Validation →
      Policy gates record on that runbook (any signature) and the episode's
      symptom is `memory-pressure`.
- [ ] AC-004 — Given a failed unit and a pressure crossing in the same
      cycle, when the brain cycles, then both situations are collected and
      reasoned in signature order with separate dedup keys.
- [ ] AC-005 — Given a malformed or missing reading source, when the
      collector runs, then that situation is absent (logged), the rest
      proceed, and nothing is fabricated.
- [ ] AC-006 — Given the default configuration with readings below
      thresholds, when the brain cycles, then behavior is byte-identical to
      today (host-health only).

## 6. Out of Scope

- New action capabilities for pressure situations (cleanup, eviction) —
  the vocabulary is open; capabilities arrive as capabilities do.
- Baseline/deviation situations (the BaselineManager keeps feeding its
  existing events; threshold situations are the first class).
- Hysteresis/latching on crossings.
- k8s/container pressure situations.
