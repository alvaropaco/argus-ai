# Feature Specification: ARGUS Running Brain — Provider Wiring & the Diagnose Loop

**Status:** Draft
**Spec ID:** 005
**Created:** 2026-10-06

## 1. Problem Statement

Specs 001–004 delivered every organ of the brain as tested, live-validated
components — observation loop, anomaly, incidents, remediation with
governance and approvals, memory, runbooks, sentinel, escalation — but the
daemon never thinks. `Daemon::diagnose_once` (spec 002's full
evidence → structured decisions → plan → policy → executor → validation
loop) has no production caller; the `[model]` provider configuration is
stored but unconsumed; the observation loop's deviations feed nothing.

## 2. Objective

Make the shipped daemon a running brain:

1. construct the decision provider at startup from `[model]` + the secret
   store, including a DeepSeek (OpenAI-compatible) adapter behind the
   `DecisionProvider` trait;
2. run a brain loop that turns observation into evidence, evidence into
   decisions, and decisions into plans executed through the existing
   safety boundary at the operator's configured autonomy;
3. let the operator trigger and watch it: `argus diagnose`, IPC
   `brain.diagnose`, runbooks loaded at startup;
4. record what the brain does: episodic memory and procedure outcomes are
   written, and the sentinel reflects live brain state.

## 3. Functional Requirements

**FR-001** The system MUST provide a `DeepSeekProvider` (OpenAI-compatible
chat-completions, JSON mode) implementing `DecisionProvider`: the request
is serialized as the decision contract, the model's JSON answer is parsed
into typed `DecisionAnswer`s, and validation is fail-closed — every
requested question must be answered, choices must be among the offered
criteria, `noul` values clamp/reject outside `[0.01, 0.99]`, and any
malformed output is a `DecisionError`, never an invented answer. Fallback
models are tried in order on transport/5xx failure. Free-form model text
is never treated as an action.

**FR-002** The daemon MUST construct its provider at init from `[model]`
(`ModelProviderConfig`) plus the provider credential from the `0600` secret
store; an absent/unreadable provider yields no provider (the brain runs
observe-only) — startup never fails on it. `laya` keeps its sidecar
adapter.

**FR-003** A brain loop MUST run in `argusd`: each tick gathers typed
evidence from live state (failed systemd units, non-running containers,
memory pressure), calls `diagnose_once` with the configured provider, and
executes any returned plan through `run_remediation` at the configured
`[brain] autonomy` (default `l0_observe` — the loop observes and explains
but never executes until an operator raises it). Completed/failed
remediations are recorded as episodes (episodic memory) and procedure
outcomes (procedural memory), and a shared brain state (last hypothesis,
active risks, open-loop incidents count, prediction prose) is surfaced
through `sentinel.get`.

**FR-004** IPC `brain.diagnose` MUST trigger one brain cycle on demand
(returning the evidence, decision summary, plan, and outcome; `NotReady`
when no provider is configured), and the CLI MUST expose `argus diagnose`.
`[brain]` config: `autonomy`, `interval_seconds`, `confidence_threshold`.

**FR-005** The daemon MUST load runbooks at startup from a configured
directory (default `<config dir>/runbooks`) through the spec-004 loader,
log the load summary, and expose `runbooks.list` over IPC + `argus
runbooks` in the CLI.

## 4. Non-Functional Requirements

- The security boundary is untouched: model output remains data; every
  plan crosses policy → typed executor → validation exactly as before;
  the loop's autonomy defaults to the most conservative level.
- Provider calls are bounded (timeout), and no secret ever appears in
  logs, events, reports, or IPC responses.
- Everything remains testable without a host, cluster, or live API key
  (fake provider + canned HTTP fixtures).

## 5. Acceptance Criteria

- [ ] AC-001 — With a configured provider, `argus diagnose` returns a
      decision made by the provider (fake or sidecar in tests), a plan,
      and an execution outcome routed through policy.
- [ ] AC-002 — Malformed model output (missing question, bad choice,
      out-of-range noul) fails closed; nothing executes.
- [ ] AC-003 — Without a provider, the daemon and loop start normally and
      `brain.diagnose` reports NotReady rather than guessing.
- [ ] AC-004 — At `l0_observe` the loop executes nothing; raising
      `[brain] autonomy` changes execution only through the existing
      gates (policy, escalation, approvals).
- [ ] AC-005 — A completed remediation writes an episode and a procedure
      outcome; `sentinel.get` reflects the live brain state.
- [ ] AC-006 — Runbooks load at startup; `argus runbooks` lists them.

## 6. Out of Scope

- The learning pass that proposes runbook candidates (ADR-0031, gated).
- Kubernetes finishing (v1.1) and LanceDB.
- Cloud-side web views of the brain.
