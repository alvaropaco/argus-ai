# Feature Specification: ARGUS Nervous System — Action Ledger, Live Fleet Stream & Cost Accounting

**Status:** Draft
**Spec ID:** 007
**Created:** 2026-10-09

> Builds directly on spec 006 (fleet view: sentinel reports, cloud approvals,
> managed autonomy) and spec 005 (running brain). The streaming-backbone
> decision is ADR-0039. This spec is the precondition for spec 008 (graduated
> autonomy): authority is only granted to what can be observed.

## 1. Problem Statement

The brain runs (005) and reports (006), but the cloud sees *samples*, not the
stream. The sentinel report carries the last brain cycle and at most 10 plans
on a reporting cadence; the individual operations agents execute — the actual
commands, their arguments, the policy verdict on each, the outcome, the
duration — never leave the host. The reasoning behind a decision survives only
as a summary. Token and cost accounting does not exist: the DeepSeek adapter
drops the `usage` block the provider returns. An operator cannot answer "what
did my agents do in the last hour, why, and what did it cost" from the
dashboard. That visibility is the trust basis on which wider autonomy (008)
can ever be conceded.

## 2. Objective

1. **Ledger**: every operation that crosses the policy/executor boundary —
   allowed, denied, or escalated — becomes an immutable, correlated
   `ActionEvent`, persisted locally at full fidelity.
2. **Trace**: every brain cycle emits a bounded trace event (evidence
   referenced, decision, plan, outcome) causally linked to the actions it
   caused.
3. **Meter**: every provider call records token usage (prompt/completion/
   total, model, duration); rollups per cycle and per period.
4. **Stream**: events flow to the cloud over the existing WebSocket envelope
   protocol, buffered offline, batched, redacted before upload (raw stays
   local).
5. **See**: the dashboard gains a live Activity view per instance — live
   tail, filters, action detail linking reasoning and tokens — and a
   silence alarm (a sentinel that stops talking is itself an incident).

## 3. Functional Requirements

**FR-001 (ActionEvent)** Every execution attempt MUST emit an `ActionEvent`
at the policy/executor boundary, regardless of verdict. Fields: event id,
correlation/causation ids, instance/host identity, timestamp, action kind and
target, redacted argument summary, policy verdict (allow / deny /
requires-approval) and the policy rule that produced it, terminal outcome
(ok / failed / rolled-back / timeout / denied / awaiting-approval), duration,
validation result when the action ran. Events are persisted append-only in
local state (full fidelity, including raw arguments) and are never rewritten.

**FR-002 (Brain trace)** Each diagnose cycle MUST emit one bounded trace
event: cycle id, evidence referenced (ids/counts — not full payloads),
decision summary, plan objective and step list, outcome. The trace's
correlation id MUST match the ActionEvents it caused, so a dashboard can walk
reasoning → plan → actions → validation in one chain.

**FR-003 (Token metering)** The provider adapter MUST parse the `usage` block
(prompt/completion/total tokens) from every chat-completions response and
emit a usage event per call (model id, duration, cycle correlation). Missing
usage is recorded as unknown — never estimated silently. Usage events
roll up per cycle and per reporting period.

**FR-004 (Redaction before upload)** The upload path MUST redact
secret-shaped values: token/key/password/secret-named fields, authorization
headers, environment values embedded in arguments, and high-entropy strings.
Redaction is deterministic, unit-tested, and fail-safe: on any redaction
error the field is dropped, never uploaded raw. The local ledger keeps full
fidelity; the cloud never receives raw arguments or secrets.

**FR-005 (Streaming)** New envelope message kinds `action.event`,
`brain.trace`, and `token.usage` ride the existing daemon↔cloud WebSocket.
Delivery is at-least-once with per-instance ordering; the offline buffer is
bounded and, under sustained backpressure, *ledger events are never dropped* —
traces coalesce and usage rollups batch. Events are batched per flush.

**FR-006 (Cloud ingest & API)** The cloud MUST ingest the three event kinds
into a per-instance ledger (append-only tables + migration), expose
`GET /instances/:id/actions?since=&kind=&status=`, `GET
/instances/:id/usage?period=`, and `GET /instances/:id/traces/:cycle_id`,
and fan out each ingested event live to connected dashboard clients (SSE)
within ~1 s of ingest.

**FR-007 (Dashboard Activity view)** The instance page MUST gain an Activity
tab: live tail (auto-appending), filters (kind, status, period, free text),
and an action detail drawer showing the full event plus the linked trace
(reasoning) and token spend of the same cycle. Denied and rolled-back
actions are first-class citizens — the operator sees what was *prevented*.

**FR-008 (Silence alarm)** The cloud MUST track last-seen per instance
(events and sentinel reports) and surface a `silent` state on the instance
page and fleet list once quiet time exceeds a configurable threshold
(default: 3× the reporting interval). Recovery clears the state.

## 4. Non-Functional Requirements

- **No new infrastructure in this spec**: the daemon leg rides the existing
  WebSocket channel; NATS adoption is phased per ADR-0039 (cloud-side first).
- Ledger writes are local-first and asynchronous — emission never blocks or
  fails an execution.
- Bounded growth: local ledger has configurable retention; cloud history is
  retained per the instance's data policy.
- No secrets anywhere in cloud payloads; redaction failure drops, never leaks.
- The lean single-host build gains no hard dependency (websocket events are
  optional-to-disable via config; the ledger is always local).

## 5. Acceptance Criteria

- [ ] AC-001 — An executed remediation action appears in the local ledger
      with all FR-001 fields and on the dashboard Activity tail within ~2 s.
- [ ] AC-002 — A policy-denied action produces an event with verdict `deny`
      and is visible in the Activity view.
- [ ] AC-003 — A diagnose cycle's trace is queryable and links to the
      ActionEvents it caused (one chain in the detail drawer).
- [ ] AC-004 — Usage events sum to the per-period rollup returned by the
      usage API; a response lacking `usage` shows as unknown, not zero.
- [ ] AC-005 — An action whose arguments contain a secret-shaped value is
      uploaded redacted; the local ledger retains the raw value.
- [ ] AC-006 — Killing the daemon mid-stream and restarting delivers the
      buffered events exactly once, in order.
- [ ] AC-007 — A silenced instance trips the silence alarm after the
      threshold and clears on recovery.
- [ ] AC-008 — A sustained event burst causes trace coalescing / usage
      batching, with zero ledger-event loss.

## 6. Out of Scope

- Cross-instance fleet aggregation views (deferred from 006; still later).
- NATS on the daemon side / replacing LocalEventBus (ADR-0039 phase 3).
- Full evidence payload upload (traces carry ids/counts; evidence stays local).
- Cloud-side authoring or editing of remediations.
- Provider cost tables / dollar estimates (config surface deferred; the
  usage data this spec produces is its prerequisite).
