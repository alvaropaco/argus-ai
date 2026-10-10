# ADR-0042: Promoted Runbooks Drive Procedures — Attribution by Construction

- **Status:** Accepted
- **Date:** 2026-10-10

## Context

ADR-0036 says promoted runbooks "drive procedures"; ADR-0041 made the
learning path reachable (deliver → earn → operator promotes) but stopped at
promotion: a promoted runbook still affects nothing, because the brain never
consults the library. Spec 009's opportunistic Validation/Policy gates
therefore keyed on the trigger vocabulary (the documented approximation):
any resolved host-health cycle validated every matching candidate —
attribution was guessed — and runbooks authored for other triggers could
never climb. The missing piece is the link from "operator promoted this
procedure" to "this procedure is what runs".

## Decision

1. **Promoted runbooks produce deterministic procedure plans.** When the
   brain's situation matches a promoted runbook's trigger, the daemon
   builds the plan *from the runbook* (allowed actions as steps, the
   runbook's rollbacks paired, blast radius from the registered
   descriptors) and runs it through the ordinary boundary — policy,
   escalation, approval, budget, ledger, validation. The provider is not
   consulted for procedure plans: they are not AI output at all, which is
   the strongest possible reading of "AI output is data, never authority".
2. **Attribution is by construction, never inference.** A procedure plan
   carries its runbook's name as a field on the plan; spec 009's
   opportunistic gates record Validation/Policy only for the attributed
   runbook of a completed, validated plan. No trigger-vocabulary matching,
   no text similarity, no guessing — the plan *is* the runbook's procedure.
3. **Failure is history, not demotion.** Failed procedure runs record
   `record_outcome(false)` (the success rate the operator saw at approval
   stays honest) and release the situation for the next cycle. Demoting a
   runbook stays an operator decision (ADR-0040 discipline) — the ladder's
   trust was earned once; a falling success rate is the operator's signal
   to revoke, not an automatic one.
4. **Provider path unchanged.** With no matching promoted runbook, the
   brain behaves byte-for-byte as before. Runbooks widen what the daemon
   can do on its own; they never narrow or reroute the reasoning fallback.

## Alternatives considered

- **Inject candidate runbooks into the provider's decision context** —
  rejected as the primary mechanism: "the model used the runbook" is not
  verifiable from a free-text decision, so attribution would regress to
  inference; kept as a possible future *addition* for candidate (non-
  promoted) runbooks.
- **Trigger-vocabulary validation (spec 009's approximation) as
  permanent** — rejected: it validates procedures that never ran and locks
  the ladder to one hard-coded trigger.
- **Automatic demotion on low success rates** — rejected: demotion is the
  operator's revocation right (ADR-0040); the success rate is the signal,
  not the trigger.

## Consequences

- Promotion becomes load-bearing: promoting a runbook changes what the
  brain does on matching situations. The instance page's promotion button
  carries that weight, and the plan approval flow still gates every
  execution per autonomy level.
- `Plan` gains an additive `runbook` field; persisted plans and both
  protocol surfaces tolerate its absence.
- The Evaluation gate gains a content linkage over time (episodes record
  the runbook's name as the remediation signature), tightening spec 009's
  trigger-corroboration subset without new memory semantics.
- Spec-010 retires spec 009's documented approximation; the ladder is
  honest for any trigger the brain's situations can express.
