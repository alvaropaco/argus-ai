# Feature Specification: ARGUS Runbook Distribution & the Promotion Ladder

**Status:** Draft
**Spec ID:** 009
**Created:** 2026-10-10

> Closes the deferral declared in spec 006 ("runbook distribution over the
> config channel — the promotion ladder needs UI of its own") and completes
> the learning path ADR-0040 reserved for L5: the cloud may *author and
> deliver*, the host *earns* through the ladder, the operator *approves and
> promotes*. Decision record: ADR-0041 (delivery is not promotion).

## 1. Problem Statement

The promotion ladder exists and is tested — evaluation → simulation →
validation → policy → approval → promotion, one gate at a time, nothing
self-promotes — but it has no production caller: runbooks load from a local
directory as candidates and stay candidates forever. An operator with ten
instances cannot deliver a learned procedure to any of them, cannot see where
a candidate stands on the ladder, and cannot approve or promote anything.
L5's promised learning path is unreachable.

## 2. Objective

1. **Deliver**: runbooks travel the managed-configuration channel as a new
   `runbook` kind (content = runbook TOML). The daemon validates and loads
   them as **candidates** with cloud provenance — delivery never gates,
   never promotes.
2. **Earn**: the daemon records the technical gates honestly and
   opportunistically — evaluation (steps against recorded episodes) and
   simulation (candidate actions' impact simulated and accepted) at
   delivery review; validation when the runbook's procedure actually runs
   and validates in operation; policy once policy-compatibility is checked
   (in ladder order, after validation). Gates are never invented.
3. **Decide**: approval and promotion are operator acts, surfaced on the
   instance page (candidates with gate progress + approve/promote) and
   crossing the existing command channel into the local ladder — the same
   round-trip discipline as plan approvals.
4. **See**: the sentinel report carries the runbook table; `argus runbook
   list` shows gates.

## 3. Functional Requirements

**FR-001 (Delivery)** The managed-configuration channel MUST accept a
`runbook` kind whose content carries the runbook TOML (name, size bounds).
The daemon parses it with the existing loader: a valid runbook enters the
library as a `Candidate` tagged with the configuration version (cloud
provenance); a malformed one replies `config.result failed` with the
loader's reason — fail-closed, nothing loaded. A re-delivery of the same
name supersedes the previous candidate version.

**FR-002 (Persistence)** Delivered runbooks MUST persist in the local
repository and reload at startup beside the directory-loaded ones; a
candidate's gate progress survives restart.

**FR-003 (Honest gates)** At delivery review the daemon records — in ladder
order and only when they pass — the **evaluation** gate (the runbook's steps
checked against recorded episodes via the runbooks criterion) and the
**simulation** gate (candidate actions' impact simulated via the risk
engine and accepted); failures leave the candidate gateless with the reason
logged, never fabricated. The **validation** gate is recorded only when the
runbook's trigger fires and its procedure executes and validates in
operation; the **policy** gate is recorded when the candidate capabilities
pass the policy evaluator, which by ladder order happens only after
validation. No gate is ever recorded out of order or without its evidence.

**FR-004 (Operator decision)** A candidate that reached the policy gate
MUST be surfaced for an operator decision: approve (`Candidate → Approved`)
and promote (`Approved → Promoted`) cross the command channel (cloud →
daemon) and execute through the local ladder — `approve`/`promote` fail
closed with their existing errors; the CLI keeps parity (`argus runbook
list` gains gate progress; `argus runbook approve|promote <name>`).

**FR-005 (Visibility)** The `sentinel.report` MUST carry the runbook table
(name, status, gates, provenance) as an additive field; the instance page
MUST render it as a runbook card (status, gate progress, approve/promote
for decision-ready candidates); old snapshots render unchanged.

## 4. Non-Functional Requirements

- Delivery is data: a delivered runbook authorizes nothing at delivery time
  (constitution P2); its `allowed_actions` remain candidates-not-grants.
- No new wire protocol kinds beyond the additive sentinel field; the command
  channel validates the promotion command against the published schema.
- Gate evaluation is deterministic and local (episodes, risk engine, policy
  evaluator, registry — all existing machinery); no network at gate time.
- Fail-closed: any gate/persistence error leaves the runbook at its current
  rung, logged, never higher.
- The lean single-host build is unaffected: without cloud-delivered runbooks
  the library behaves exactly as before.

## 5. Acceptance Criteria

- [ ] AC-001 — Deploying a `runbook` configuration with valid TOML lands the
      candidate in the daemon's library with cloud provenance and an
      `applied` config result; malformed TOML replies `failed` with the
      reason and loads nothing.
- [ ] AC-002 — At delivery, passing evaluation and simulation records those
      two gates in order; a candidate failing simulation stays gateless with
      the reason logged.
- [ ] AC-003 — A candidate whose trigger fires and whose procedure executes
      and validates gains the validation gate, then the policy gate, in
      order.
- [ ] AC-004 — Approve and promote from the dashboard complete the
      round-trip: the runbook lands `Promoted` and drives procedures; the
      ladder rejects out-of-order decisions with the existing errors.
- [ ] AC-005 — The instance page shows the runbook card (status, gates,
      provenance) and the sentinel report carries the additive field; old
      snapshots render unchanged.
- [ ] AC-006 — A daemon restart reloads delivered candidates with their
      gate progress intact.

## 6. Out of Scope

- Automatic promotion or unattended learning (L5 stays operator-granted).
- Runbook authoring/editing UI (the cloud delivers content; authoring stays
  in files/repositories).
- Opportunistic gate recording for directory-loaded runbooks (unchanged
  behavior; the ladder machinery is shared, the wiring starts with
  delivered candidates).
- Runbook deletion/revocation flows.
