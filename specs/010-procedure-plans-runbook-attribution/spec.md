# Feature Specification: ARGUS Procedure Plans — Runbook Attribution by Construction

**Status:** Draft
**Spec ID:** 010
**Created:** 2026-10-10

> Completes ADR-0036's promise that promoted runbooks "drive procedures":
> today a promoted runbook affects nothing — the brain never consults the
> library, plans carry no runbook linkage, and spec 009's opportunistic
> gates key on the trigger vocabulary instead of attribution (the documented
> 009 approximation). Decision record: ADR-0042 (attribution by
> construction — a promoted runbook produces a deterministic procedure plan;
> inference is never involved).

## 1. Problem Statement

A promoted runbook — delivered, gate-earned, operator-approved — changes
nothing: the brain never reads the library, so the operator's promotion is
decorative. Spec 009's Validation gate records from *any* resolved
host-health cycle rather than from a plan that actually followed the
runbook, and runbooks authored for any other trigger can never climb. The
chain "operator trusted this procedure → the procedure runs → the run earns"
is broken at its first link.

## 2. Objective

1. **Promoted runbooks drive procedures**: when the brain's situation
   matches a promoted runbook's trigger, the daemon builds a deterministic
   procedure plan *from the runbook* — same policy, escalation, approval,
   budget, ledger boundary as every plan; the provider is not consulted for
   it.
2. **Attribution by construction**: procedure plans carry the runbook's
   name; spec 009's opportunistic Validation/Policy gates record only for
   the runbook whose procedure actually ran and validated — any trigger,
   not just host-health.
3. **Honest history**: procedure outcomes record onto the runbook
   (attempts/success rate) and into episodes with the runbook's name as the
   remediation signature, which in turn strengthens the Evaluation gate.

## 3. Functional Requirements

**FR-001 (Procedure plans)** On each brain cycle, before the provider path,
the daemon MUST consult the library for **promoted** runbooks whose trigger
matches the cycle's situation signature; the first match (name-ordered)
yields a deterministic procedure plan: objective `procedure: <name>`, steps
from the runbook's allowed actions (each paired with the runbook's rollback
entry where declared), blast radius the worst of its actions' registered
descriptors, confidence the runbook's historical success rate when known
(else a conservative fixed 0.5). The plan then crosses the ordinary
`run_remediation` boundary — policy, escalation, approval, budget, ledger,
validation — exactly like a provider plan. No matching promoted runbook →
the provider path runs unchanged.

**FR-002 (Attribution)** `Plan` MUST carry an additive, default-absent
`runbook: Option<String>` set on procedure plans and absent on provider
plans; the sentinel report, Activity ledger rows, and the instance page
surface it. The plan's objective carries `procedure: <name>` so approvals
name what is being approved.

**FR-003 (Exact opportunistic gates)** The opportunistic Validation/Policy
hook MUST key on plan attribution — a completed, validated procedure plan
records Validation, then Policy, for exactly its attributed runbook —
replacing the trigger-vocabulary fallback. A runbook whose procedure has
not run is never validated; any trigger climbs.

**FR-004 (Honest history)** A procedure plan's outcome MUST record onto its
runbook (`record_outcome`) — feeding `attempts`/`historical_success_rate` —
and into the episode record with the runbook's name as the remediation
signature. The Evaluation gate's evidence check MUST accept a matching
episode whose remediation carries the runbook's name (content linkage), in
addition to the trigger match.

**FR-005 (Failure behavior)** A procedure plan that fails or rolls back
records the unsuccessful outcome on its runbook and releases the dedup key
like any unresolved situation — the next cycle may try again (the runbook's
success rate falls; the operator sees it). No automatic runbook demotion:
the ladder's operator retains the decision (ADR-0040 discipline).

## 4. Non-Functional Requirements

- Procedure plans are deterministic and provider-independent: the boundary
  (policy → escalation → approval → budget → ledger → validation) is byte-
  identical to provider plans; only the plan's *origin* differs.
- `Plan`'s new field is additive with serde default — persisted plans,
  IPC 0.8.0 payloads, and the cloud snapshot tolerate absence.
- Library consultation is an in-process read (lock discipline per spec 009);
  no network, no new sensing.
- Fail-closed: any error building a procedure plan logs and falls through to
  the provider path — never a missed remediation.

## 5. Acceptance Criteria

- [ ] AC-001 — Given a promoted runbook whose trigger matches the cycle's
      situation, when the brain cycles, then a procedure plan attributed to
      that runbook is proposed (objective `procedure: <name>`, steps from
      its allowed actions) without consulting the provider.
- [ ] AC-002 — Given no matching promoted runbook, when the brain cycles,
      then the provider path behaves exactly as before (no attribution
      field).
- [ ] AC-003 — Given an executed and validated procedure plan, when the
      terminal outcome lands, then Validation and Policy gates record for
      exactly the attributed runbook (any trigger), with
      `record_outcome(true)` and an episode carrying the runbook's name.
- [ ] AC-004 — Given a failed/rolled-back procedure plan, when the terminal
      outcome lands, then `record_outcome(false)` lands on the runbook, the
      gates do not advance, and the dedup key releases.
- [ ] AC-005 — Given a promoted runbook with a resolved-and-named episode
      history, when it is re-delivered as a candidate (re-delivery
      supersedes), then the Evaluation gate's evidence accepts the
      name-linked episode.
- [ ] AC-006 — Given a procedure plan paused for approval, when the operator
      grants, then the resume executes the same attributed plan and the
      outcome flows to the runbook as usual.

## 6. Out of Scope

- Growing the brain's situation vocabulary beyond host-health (other
  triggers climb the moment their situations exist; the mechanism is
  trigger-agnostic).
- Provider plans consulting candidate runbooks as context (LLM prompting).
- Automatic demotion of runbooks with low success rates.
- Runbook-driven plans outside the brain loop (CLI/one-shot stays provider
  path).
