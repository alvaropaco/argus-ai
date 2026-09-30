# ADR-0034: Autonomous Investigation Engine

- **Status:** Accepted
- **Date:** 2026-09-30

## Context

The existing brain produces a single `Hypothesis`/`Plan` per decision
(`argus-ai-core`), but CAP-8/9 require a multi-step investigation loop
(incident → evidence → hypothesis generation → testing → elimination → root
cause → remediation plan) that tracks supporting and contradicting evidence,
confidence, tests performed, and unresolved uncertainty — and that never
fabricates certainty. The loop must compose with the existing decision gateway
without introducing a second model-authority path.

## Decision

### 1. A deterministic investigation state machine in `argus-investigate`

The investigation engine is a Rust state machine (collect evidence → generate
hypotheses → test → eliminate or conclude → remediation plan). Every transition
is deterministic and driven by typed evidence queries and graph lookups.

### 2. Hypothesis generation is a structured decision; everything else is deterministic

Hypothesis *generation* is posed to the existing `DecisionEngine` gateway as a
typed `choice`/`score` decision over a bounded, redacted evidence context (the
same gateway and validation rules as the existing brain). Hypothesis *testing*,
*elimination*, and evidence gathering are deterministic Rust. The state machine
may pose multiple sequential decisions, each independently schema-validated and
provenance-bound.

### 3. Every hypothesis records evidence and uncertainty

Each `Hypothesis` in the investigation carries supporting evidence,
contradicting evidence, confidence, tests performed, a conclusion, and
unresolved uncertainty. When evidence is insufficient, the engine reports the
uncertainty and does not assert a root cause.

### 4. Root cause and remediation are separate outputs

The engine emits a root-cause analysis (what failed, why, what changed, what was
affected, blast radius, what can safely be done) and a *candidate* remediation
plan. The remediation plan is data — it still crosses policy and the typed
executor; the investigation engine never authorizes or executes.

## Consequences

### Positive

- Investigation is deterministic and testable with a fake decision engine.
- No new authority path: the only stochastic step is the existing decision
  gateway, and its output is validated before use.
- Evidence-backed root cause with explicit uncertainty replaces free-text
  diagnosis.

### Negative

- A new crate and a multi-decision orchestration path to test.
- Structured-decision authoring for hypotheses must follow the same
  decision-authoring rules (spec-002 FR-011) as the existing brain.

## Implementation Principles

1. Deterministic state machine; only hypothesis generation is a decision call.
2. Hypotheses are evidence-backed; never fabricate certainty.
3. Root cause is an output; remediation is data that still crosses policy.
4. Reuse the existing `DecisionEngine` trait and validation; add no model path.

## Reference

- `_bmad-output/specs/spec-argus/brain-architecture.md` — one stochastic step.
- `_bmad-output/specs/spec-argus/SPEC.md` — CAP-3/CAP-14 (decision gateway).
- `_bmad-output/specs/spec-autonomous-operations-brain/SPEC.md` — CAP-8, CAP-9.
- `_bmad-output/specs/spec-autonomous-operations-brain/state-machines.md` — the
  investigation loop.
- `specs/002-ai-runtime/spec.md` — FR-011 decision-authoring rules.
