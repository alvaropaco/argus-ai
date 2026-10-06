# Feature Specification: ARGUS Fleet View — Cloud Observability & Management for the Brain

**Status:** Draft
**Spec ID:** 006
**Created:** 2026-10-06

## 1. Problem Statement

The brain runs on the host and the cloud is its control plane, but the two
halves barely meet: the telemetry report carries health and counters only,
so the dashboard cannot show what an agent is seeing, deciding, or doing;
the one lever that governs an agent (its autonomy level) is a local file
edit; and when the brain pauses a plan for approval, only someone on the
host can grant it. An operator with ten instances sees heartbeats, not a
fleet.

## 2. Objective

1. report the brain: a periodic `sentinel.report` carrying the live
   `SentinelView`, the brain's last cycle (evidence, decision, plan,
   outcome), recent plans, and pending approvals;
2. manage the brain from the cloud: `brain.autonomy`,
   `brain.confidence_threshold`, and `brain.interval_seconds` become
   managed settings applied **live** (no restart) through the existing
   versioned-configuration channel;
3. decide from anywhere: a cloud operator can grant or deny a paused plan;
   the decision crosses the same local boundary (grant consumed once,
   context-hash binding) and the outcome is reported back;
4. see it all on the instance page: sentinel card, brain activity, and
   pending approvals with grant/deny actions.

## 3. Functional Requirements

**FR-001** The daemon MUST enqueue a `sentinel.report` on its reporting
schedule carrying: the serialized `SentinelView` (health rollup, safe
mode, provider readiness, counts, pressure), the last brain cycle
(evidence, decision summary, plan objective, outcome), the most recent
plans (id, objective, status, confidence, step count, capped at 10), and
the pending approvals (token, objective, context hash). Everything is
read from real daemon state; absent state renders empty (never invented).

**FR-002** The managed-configuration channel MUST accept the `brain`
kind: settings `brain.autonomy` (L0–L5, validated), `brain.confidence_
threshold` (clamped 0–1), and `brain.interval_seconds` (clamped ≥5) are
persisted in the managed settings store **and applied live** to the
running loop — the next tick uses them, no restart. Invalid values are
rejected with the reason in the config result. The cloud can narrow or
widen autonomy exactly as a local edit would; every execution still
crosses policy, escalation, and approvals.

**FR-003** The protocol MUST carry `approval.decision` (cloud → daemon:
token + grant/deny + actor) and `approval.result` (daemon → cloud:
outcome). A grant behaves identically to the local CLI grant: the pending
plan is released, the grant is consumed exactly once, the plan resumes
through `run_remediation`, and the result reports the terminal status.
An unknown/stale token is refused with the reason.

**FR-004** The cloud MUST ingest `sentinel.report` into a per-instance
snapshot store (latest served first, history retained), expose
`GET /instances/:id/sentinel`, and provide `POST /instances/:id/approvals`
(token + decision) that dispatches `approval.decision` over the control
channel and records the decision in the audit trail.

**FR-005** The web instance page MUST render the sentinel card (health,
safe mode, provider, counts), the brain's recent plans with status and
confidence, pending approvals with grant/deny buttons, and an autonomy
control that deploys a `brain` configuration version through the existing
configuration API.

## 4. Non-Functional Requirements

- Nothing authorizes across the boundary: a cloud approval is input to the
  local round-trip (same store, same single-use token, same context hash);
  the cloud cannot invent a grant for a plan it has not seen.
- Reports stay bounded: plans capped at 10, no evidence payloads, no
  secrets in any field.
- The lean single-host build is unaffected: the sentinel report reuses the
  daemon's existing state; no new dependencies.

## 5. Acceptance Criteria

- [ ] AC-001 — A paired instance's `sentinel.report` lands in the cloud
      and the instance page shows the live sentinel state.
- [ ] AC-002 — A brain cycle's plan appears in the dashboard with status
      and confidence after the next report.
- [ ] AC-003 — Granting a paused plan from the dashboard completes the
      round-trip; the outcome is visible in the UI and the audit trail.
- [ ] AC-004 — Deploying a `brain` configuration with a new autonomy
      level changes the running loop's behavior on the next tick, visible
      in the config-state and the report.
- [ ] AC-005 — An invalid autonomy value is rejected by the daemon with
      the reason surfaced in the deployment status.

## 6. Out of Scope

- Runbook distribution over the config channel (the promotion ladder needs
  UI of its own; deferred to 0.2.1).
- Cross-instance fleet aggregation views.
- Cloud-side editing of runbook content.
