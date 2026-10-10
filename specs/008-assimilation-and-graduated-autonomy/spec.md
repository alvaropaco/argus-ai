# Feature Specification: ARGUS Assimilation & Graduated Autonomy

**Status:** Draft
**Spec ID:** 008
**Created:** 2026-10-10

> Completes the trust ladder: visibility (007) → approval (006) → **earned
> autonomy (008)**. Architectural decision: ADR-0040 (authority is earned per
> environment, bounded per period, revocable). Builds on the existing L0–L5
> modes, `may_execute_without_approval`, `decide_escalation`, SafeMode, the
> managed `brain.autonomy` ceiling, and the 007 ledger stream.

## 1. Problem Statement

Autonomy today is a static dial the operator sets blind. `brain.autonomy`
applies live, but nothing verifies the daemon actually knows the environment
before it acts in it; there is no blast-radius budget bounding how much it may
do per period; and when it misbehaves (failed validation, safe mode) nothing
lowers the dial. An operator must choose between L0 forever and handing over
L3+ on faith. The organism the owner described — explore, learn, earn control,
obey the law — needs authority to be *earned*, *bounded*, and *revocable*.

## 2. Objective

1. **Assimilate**: on entering an environment, run an acquisition state
   machine — MAP (live inventory/graph) → SHADOW (bounded cycles building
   baselines and proving clean behavior) → EARNED (rung-by-rung promotion
   toward the operator's ceiling, each rung gated by clean cycles).
2. **Bound**: every auto-execution must pass a blast-radius budget (per risk
   class, per rolling window); exhaustion degrades to approval, never to a
   denial of service.
3. **Revoke**: demote automatically on failed validation with rollback,
   SafeMode entry, provider fail-closed degradation, or a lowered cloud
   ceiling; re-earning runs the same gates. The effective autonomy is always
   `min(cloud ceiling, earned rung)`.

## 3. Functional Requirements

**FR-001 (Assimilation state)** The daemon MUST keep a persisted
`AutonomyState` per environment (phase `mapping|shadow|earned`, earned rung,
gate counters) that survives restart, driven by the brain loop tick. Fresh
environments start at `mapping` with earned rung L0.

**FR-002 (Gates)** MAP → SHADOW requires: a non-empty live environment graph
and known provider readiness. SHADOW → EARNED requires, over the shadow
window (default 10 completed cycles, config `shadow_min_cycles`): provider
healthy, no unresolved critical incident, no SafeMode, no failed validation,
and cloud paired. Each promotion rung (L2 → L3 → L4 tier) requires
`rung_clean_cycles` (default 20) clean cycles at the current rung. L1 is
skipped by the ladder (it is a labeling mode, not a trust rung); L5 is not
reachable by promotion — learning stays operator-granted.

**FR-003 (Effective autonomy)** The autonomy handed to `run_plan`/escalation
MUST be `min(managed ceiling, earned rung)`. Promotions and demotions emit
`autonomy.promoted`/`autonomy.demoted` domain events and a ledger trace row
each; demotion triggers: SafeMode entry (drop to L0), failed validation with
rollback (−1 rung), provider degraded (−1 rung). Re-earning after a demotion
re-runs the rung gates.

**FR-004 (Blast-radius budget)** Before auto-execution (the
`may_execute_without_approval` branch), the plan loop MUST check a rolling
budget keyed by risk class and scope: defaults `low_risk: 20/hour`,
`controlled: 5/day`, `host-scope: 3/day` (config `[autonomy] budgets`,
clamped ≥1). Exhaustion converts the step to `requires_approval` with policy
id `budget.exhausted` — a pause, not a denial. Counters persist across
restart and appear in the sentinel view with remaining amounts.

**FR-005 (Reporting)** The `SentinelView` and `sentinel.report` MUST carry:
assimilation phase with gate progress (`7/10 cycles`), earned rung, ceiling,
effective autonomy, and remaining budgets. Payload changes are additive
fields on the existing report (cloud validates loosely; no protocol bump).
The web instance page MUST show the autonomy line (effective of ceiling,
phase progress, budget bars) beside the existing autonomy control.

**FR-006 (Config)** New optional `[autonomy]` section:
`shadow_min_cycles` (default 10, clamped 1–1000), `rung_clean_cycles`
(default 20, clamped 1–10 000), `budgets` (per-key clamped ≥1). Absent
section → defaults; ceiling semantics and defaults of `brain.autonomy` are
unchanged (the machine works below whatever ceiling exists).

## 4. Non-Functional Requirements

- The production default is unchanged: absent an operator-set ceiling the
  daemon stays L0 — spec 008 builds the machinery, the operator grants.
- Budget/gate checks are deterministic, in-process, and add no network calls.
- No new wire protocol kinds (additive sentinel fields only); IPC untouched.
- All state lives in the local repository (SQLite + in-memory twins); the
  cloud holds no autonomy state beyond the ceiling it already manages.
- Fail-closed: any state-machine error degrades to *lower* effective autonomy,
  never higher.

## 5. Acceptance Criteria

- [ ] AC-001 — Given a fresh paired instance with ceiling L2, when 10 clean
      cycles pass, then phase reaches `earned` and effective autonomy L2,
      visible in the sentinel report.
- [ ] AC-002 — Given ceiling L3 and a clean L2 period, when the rung gate
      passes, then effective autonomy reaches L3 with a `autonomy.promoted`
      trace.
- [ ] AC-003 — Given a failed validation with rollback at L3, when the plan
      terminates rolled-back, then effective autonomy demotes one rung with a
      `autonomy.demoted` trace.
- [ ] AC-004 — Given an exhausted low-risk budget, when another low-risk
      action arrives, then the step pauses as `requires_approval`
      (`budget.exhausted`) and the sentinel shows the empty budget.
- [ ] AC-005 — Given the cloud deploys a lower `brain.autonomy` ceiling, when
      it applies live, then effective autonomy drops immediately.
- [ ] AC-006 — Given SafeMode entry at any rung, when the sentinel evaluates,
      then effective autonomy is L0 until gates re-earn it.
- [ ] AC-007 — Given a daemon restart mid-assimilation, when it comes back,
      then phase/rung/budget counters continue from persisted state.

## 6. Out of Scope

- Reaching L5 by promotion (learning stays operator-granted).
- Changing approval round-trip semantics or the escalation ladder ordering.
- Cloud-side assimilation dashboards beyond the sentinel card extension.
- Per-capability budgets (risk-class × scope granularity only).
- Cedar migration of the autonomy matrix (the Rust fn stays the source;
  "Cedar-ready" preserved).
