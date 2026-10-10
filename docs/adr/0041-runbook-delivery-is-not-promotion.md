# ADR-0041: Runbook Delivery Is Not Promotion — the Ladder Stays Local and Earned

- **Status:** Accepted
- **Date:** 2026-10-10

## Context

Spec 006 deferred "runbook distribution over the config channel" because the
promotion ladder needed operator surface. The ladder (ADR-0036 §3:
evaluation → simulation → validation → policy → approval → promotion, in
order, nothing self-promotes) exists and is tested but has no production
caller. The managed-configuration channel delivers versioned documents and
applies them live (spec 006), and the command channel carries operator
decisions into local machinery under fail-closed policy (spec 006's
approvals). The question: when the cloud can deliver a learned procedure to
a fleet, what prevents delivery from becoming authority?

## Decision

1. **Delivery is data.** A `runbook` configuration kind carries TOML
   content; the daemon parses it with the existing loader and enters it as a
   `Candidate` with cloud provenance. A delivered runbook authorizes
   nothing: its `allowed_actions` remain candidates-not-grants, and
   malformed content loads nothing (fail-closed config result).
2. **Gates are earned locally, from evidence that already exists.**
   Evaluation (steps against recorded episodes via the runbooks criterion)
   and simulation (candidate actions' impact via the risk engine) are
   deterministic local checks recordable at delivery review. Validation
   requires the runbook's procedure to actually run and validate in
   operation — opportunistic, never simulated into existence. Policy is
   recorded after validation, in ladder order. Approval and promotion are
   operator acts crossing the command channel into the local ladder, whose
   existing errors reject out-of-order decisions.
3. **The cloud authors; the host earns; the operator decides.** No cloud
   component stores or computes ladder state (mirroring ADR-0040's local
   autonomy state): the ladder lives in the daemon's library, is persisted
   locally, and reaches the operator through the sentinel report.

## Alternatives considered

- **Cloud-promoted runbooks** (deliver pre-promoted) — rejected: it would
  make the ladder decorative and hand the learning path to whoever holds the
  cloud tenant, inverting the constitution's boundary (cloud commands are
  inputs, never authority).
- **Skip the technical gates for delivered content** (author already
  tested it elsewhere) — rejected: "worked on another host" is not evidence
  this host's episodes, registry, and policy agree; the gates are cheap and
  local.
- **A separate runbook sync channel** — rejected: the managed-configuration
  channel already delivers versioned documents with apply/live/reply
  semantics; a second channel would duplicate them.

## Consequences

- The `runbook` config kind joins `provider`/`brain` (apply) — its content
  is a document, not dotted settings, so its validation path is the runbook
  loader, not the settings clamps.
- Promotion decisions ride the command channel and are audited like plan
  approvals.
- L5's learning path becomes reachable: a delivered, ladder-earned,
  operator-promoted runbook drives procedures — the first honest route from
  "learned elsewhere" to "trusted here".
