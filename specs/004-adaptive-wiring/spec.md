# Feature Specification: ARGUS Adaptive Wiring — Live Surfaces for the Brain

**Status:** Draft
**Spec ID:** 004
**Created:** 2026-10-05

## 1. Problem Statement

Spec 003 delivered the adaptive brain as tested libraries and decision
functions: five memory layers, gated runbooks, impact simulation, seven
report kinds, the L0–L5 autonomy model, the escalation decision, and the
sentinel evaluation. Almost none of it is reachable from the running
system. The memory layers are in-process structures with no persistence;
runbooks have no on-disk format or loader; the sentinel evaluation is never
run against live daemon state and has no IPC or TUI surface; the escalation
decision is never consulted by the plan run path; reports cannot be
requested. An operator who installs ARGUS still cannot watch the sentinel,
load a runbook, or ask for a report.

## 2. Objective

Wire the adaptive layer into the live runtime without weakening any
existing boundary:

1. persist the memory layers (episodic episodes, semantic facts,
   procedures) behind `DomainRepository` (ADR-0038 §3);
2. load declarative runbooks from a directory of TOML files;
3. record the daemon's self-observability counters at real boundaries and
   expose a live sentinel snapshot over IPC and the TUI;
4. consult the escalation decision in the plan run path — at L4/L5 an
   allowed step additionally requires an AUTO-FIX escalation, while
   L0–L3 semantics are unchanged;
5. render deterministic reports from live daemon state over IPC.

## 3. Functional Requirements

**FR-001** The system MUST persist episodic episodes, semantic facts, and
procedural records through `DomainRepository` (put + list per kind,
insertion order, default-unsupported for adapters that opt out), with a
SQLite implementation (ADR-0019 interim) and the in-memory adapter.

**FR-002** The system MUST load runbooks from `*.toml` files in a
configured directory: deterministic order (sorted file names), duplicate
names rejected across files, invalid files skipped with the reason
collected (graceful degradation), and every loaded runbook starting as a
Candidate unless the file records earned gates (which are replayed in
order; an out-of-order gate list fails the file).

**FR-003** The daemon MUST hold a `SelfObservability` instance, record
into it at the daemon boundary (decisions completed, executor errors,
failed validations, policy denials, remediation outcomes), and expose a
`sentinel_snapshot()` built from live state: provider readiness, pending
approvals, executed/validated action counts, self-health, and the safe
mode. Figures with no live source yet (risks, predictions, incidents)
render as empty/zero — never invented.

**FR-004** In the plan run path, when policy **allows** a step and the
autonomy level would permit execution without approval, the system MUST —
at L4/L5 only — additionally require the escalation decision to return
AUTO-FIX for that step (typed inputs from the capability descriptor and
plan confidence; evidence quality Adequate; no invented history). Any
other escalation outcome pauses the plan for approval. At L0–L3 the
existing gating is unchanged. A policy deny or require-approval keeps
winning before the escalation is ever consulted.

**FR-005** The IPC protocol (bumped to 0.6.0) MUST add `sentinel.get`
(returning the serialized `SentinelView`) and `report.generate` (taking a
kind, rendering from live daemon state, returning the report text).

**FR-006** The TUI MUST add a Sentinel tab rendering the live view
(health, safe mode, provider, counts, pressure) and the latest generated
report.

## 4. Non-Functional Requirements

- Determinism: loaders, snapshots, and reports are pure functions of
  supplied state; no wall-clock figures beyond generated-at timestamps.
- Honesty: unknown figures stay unknown (zero-with-empty-source, `None`,
  or "unknown" prose); no metric is fabricated to fill a view.
- Security posture unchanged: no new capability, no new privilege, no
  bypass; the escalation gate can only *reduce* what executes.
- All new code mock-testable without a host, cluster, or model.

## 5. Acceptance Criteria

- [ ] AC-001 — Episodes, facts, and procedures round-trip through the
      SQLite repository and list in insertion order.
- [ ] AC-002 — A runbook directory loads deterministically; duplicate
      names and invalid files are handled per FR-002.
- [ ] AC-003 — `sentinel.get` returns a view whose fields match live
      daemon state; the safe mode reflects recorded self-health.
- [ ] AC-004 — At L4, a low-confidence allowed step pauses for approval;
      a high-confidence reversible bounded step executes; at L3 the
      pre-existing behavior is byte-for-byte unchanged.
- [ ] AC-005 — `report.generate` returns deterministic text containing
      only recorded figures.
- [ ] AC-006 — The TUI renders the Sentinel tab from `sentinel.get`.

## 6. Out of Scope

- Live sensor/kubernetes validation (needs a host/cluster).
- The learning pass that *proposes* runbook candidates (ADR-0031).
- Cloud-side sentinel/approval surfacing.
- Any persistence beyond the interim SQLite adapter.
