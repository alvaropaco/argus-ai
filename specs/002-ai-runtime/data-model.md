# Data Model: ARGUS AI Runtime

**Spec:** `specs/002-ai-runtime/spec.md`
**Status:** Draft
**Date:** 2026-09-19

Entities materialized by the reasoning gateway and control loop. Convention
continues from Spec 001: strongly-typed identifiers, no infrastructure-specific
types in `argus-domain`, persistence behind `DomainRepository`.

---

## 1. Decision primitives (native `argus-ai-core`)

### `DecisionQuestion`

- **Kind:** `Choice | Score | Noul` (discriminated).
- **Fields:** `id`, `instructions`, `criteria` (choice labels w/ optional
  descriptions, ordered rubric levels, or `true`/`false` descriptions).
- **Validation:** 2–50 criteria per `simple-jev` contract; `Score` levels are
  ordered lowest→highest; labels map to exactly one token each.

### `DecisionAnswer`

- **Fields:** `question_id`, `kind`, `value` (`choice` label | numeric `score` |
  `noul` truth/support), `confidence` (`0.0–1.0`), `probabilities` (distribution
  over allowed labels).
- **Rules:** `confidence` is an uncalibrated heuristic; used for gating/ranking
  only, never as an authorization signal.

## 2. Reasoning entities

### `Intent`

- A desired condition (e.g. "service nginx is running").
- **Fields:** `id`, `resource`, `attribute`, `desired` (typed value), `source`.

### `Hypothesis`

- A proposed explanation produced from one or more `DecisionAnswer`s.
- **Fields:** `id`, `incident_id`, `statement`, `supporting_evidence`
  (`ResourceId[]`), `confidence`, `status` (`open | confirmed | rejected`).

### `Plan`

- An ordered set of typed actions to move observed state toward desired state.
- **Fields:** `id`, `intent_id`, `objective`, `preconditions`, `actions`
  (ordered), `dependencies`, `expected_outcomes`, `rollback`, `blast_radius`,
  `confidence`, `status`.

### `Action`

- A typed, capability-backed operation.
- **Fields:** `id`, `capability` (`CapabilityId`, e.g. `host.service.restart`),
  `resource`, `arguments` (typed).

### `Execution`

- One attempted action.
- **Fields:** `id`, `plan_id`, `action`, `policy_decision`, `started_at`,
  `ended_at`, `result`, `evidence`, `rollback`.

### `AutonomyMode`

- `ObserveOnly | Propose | Assisted`. Default `Propose`.

### `DecisionProviderConfig`

- Extends the existing model-provider selection (`provider`, `model`,
  `fallback_models`, `base_url`) with `credential_ref` (secret-store reference)
  and `health`.

## 3. State transitions

- **Hypothesis:** `open → confirmed | rejected`.
- **Plan:** `proposed → approved | denied | superseded`;
  `approved → executing → completed | failed | rolled_back`.
- **Incident:** `open → investigating → mitigated → resolved | closed`.

## 4. Relationships

- `Incident 1—N Hypothesis`, `Hypothesis 1—1 Plan` (proposal),
  `Plan 1—N Action`, `Action 1—N Execution`,
  `Decision 1—N DecisionAnswer` (per request).
- `Execution → Evidence` (produced before/after each action).
