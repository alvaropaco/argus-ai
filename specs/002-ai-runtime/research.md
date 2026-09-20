# Research: ARGUS AI Runtime — Reasoning Gateway & Autonomous Control Loop

**Spec:** `specs/002-ai-runtime/spec.md`
**Status:** Draft
**Date:** 2026-09-19

Resolves every open question from the feature spec and the technical unknowns
listed in `plan.md` § Technical Context. Format: **Decision → Rationale →
Alternatives considered**.

---

## 1. Decision engine runtime: local `simple-jev` vs hosted

**Decision:** Support **both** behind a single `DecisionProvider` adapter, but make
the **local `simple-jev` sidecar the default and the only path required for the
first milestone**. The adapter speaks the JEV `/v1/classifier` protocol
(`contracts/decision-protocol.md`), so a local sidecar and a hosted endpoint are
interchangeable.

**Rationale:**
- Principle 13 (testability) and Principle 11 (graceful degradation): a local
  sidecar is deterministic, offline, and reproducible; a hosted endpoint is
  optional and can be swapped in without changing the core.
- The JEV protocol is language-independent (prompt/scoring contract lives in the
  `simple-jev` `common/` folder), so the Rust core depends only on the wire
  contract, not on the Python server.
- Keeps the Rust core free of any ML runtime (Principle 1: Rust core; AI-specific
  inference is an external integration).

**Alternatives considered:**
- *Hosted-only*: simplest to wire but breaks offline testability and local-first
  operation; rejected as the default, kept as an adapter.
- *Rust-native inference (candle/mistral.rs/llama.cpp)*: heavy (~2–6 weeks), MoE
  (Gemma 4 A4B) and Laya support uncertain, and duplicates what `simple-jev`
  already does; rejected (also see `plan.md` §3).

## 2. First executable action set

**Decision:** Ship `host.service.restart`, `host.service.stop`,
`host.service.start` only, implemented over **systemd/D-Bus** (the structured,
kernel-native service API), each `LowRisk` and reversible. Everything else is
deny-by-default.

**Rationale:**
- Matches the capability names already enumerated in `AGENTS.md` and the domain
  model's typed-action examples.
- systemd/D-Bus is the authoritative interface (Principle 5); no shell parsing.
- Reversible, low-blast-radius actions are the only safe first step for an
  autonomous loop (Principle 3, Principle 14).

**Alternatives considered:**
- *`host.process.signal`* and richer actions: deferred; higher blast radius and
  irreversibility require additional policy/rollback work.
- *Arbitrary command execution*: rejected outright (Principle 2).

## 3. Decision data model: adopt `typesafe-ai-sdk` vs native types

**Decision:** Define **native** `DecisionQuestion`/`DecisionAnswer` types in
`argus-ai-core` and implement the thin HTTP client ourselves. Do **not** depend on
the `typesafe-ai-sdk` crate.

**Rationale:**
- `typesafe-ai-sdk` (v0.2.0, ~42 downloads) and `jev-repl` are brand new and
  client-side only; depending on them couples the core to an immature API.
- The JEV question/answer shapes (`choice`/`score`/`noul`, probabilities,
  confidence) are small and stable enough to model natively, keeping the core
  dependency-light (Principle 15: stable contracts).
- `jev-repl`'s Ratatui sketch format is a *future* ergonomic option for authoring
  decisions in the TUI, not a runtime dependency.

**Alternatives considered:**
- *Vendor `typesafe-ai-sdk`*: faster to start but adds a young dependency; can be
  revisited once the crate stabilizes.

## 4. Secret handling for decision providers

**Decision:** Reuse the existing `argus.secrets.toml` (mode `0600`) mechanism;
extend it to hold decision-provider credentials. The `DecisionProvider` adapter
receives credentials from the secret store at call time, never via configuration
or the IPC `config.get` surface.

**Rationale:**
- Preserves Spec 001's "config and secrets are separate sources" invariant
  (research.md §3 of Spec 001).
- A local `simple-jev` sidecar typically needs no credential; hosted endpoints do,
  and the secret store already redacts by construction.

**Alternatives considered:**
- *Linux kernel keyring*: deferred to a later ADR (unchanged from Spec 001).

## 5. Deterministic testing without a live model

**Decision:** The `DecisionProvider` is a trait with a **deterministic fake** that
returns precomputed `DecisionAnswer` fixtures. Policy, schema validation, confidence
thresholding, and the control loop are all tested against the fake; a real
`simple-jev` sidecar is used only in opt-in integration tests.

**Rationale:**
- Principle 9 (deterministic control plane) and Principle 13 (testability): no
  correctness/safety test may depend on a live model.
- Mirrors how the bootstrap tests policy/executor deterministically.

**Alternatives considered:**
- *Record/replay of real JEV responses*: useful for goldens, added alongside the
  fake, not instead of it.

## 6. Decision → typed-entity mapping and confidence thresholds

**Decision:** The gateway composes N structured decisions per loop step and maps
them to domain entities with an explicit, configurable **confidence threshold**:

- a `noul` gates hypothesis formation ("is this condition present?");
- a `score` over candidate remediations selects the proposed action (highest
  level whose conditions hold, per FR-011);
- a `choice` selects the action/target when the options are discrete;
- `confidence` (largest label probability) below the threshold → no plan, escalate
  to observe/operator.

**Rationale:**
- Makes the "validate structured output" requirement concrete and testable.
- Confidence is an uncalibrated heuristic (per `simple-jev` docs), so it is used
  for gating/ranking only, never as an authorization signal — policy remains the
  sole authority.

**Alternatives considered:**
- *Calibrated probabilities*: not provided by JEV; rejected as a correctness
  requirement, kept as an optional future improvement.

## 7. Decision-quality evaluation harness

**Decision:** Ship a small eval harness (deterministic fixtures + a fake engine)
that re-runs decision-authoring regressions, mirroring the JEV prompt guide's
lessons: measure agreement/noise against fixed fixtures, and never trust a fixture
set that omits a known failure family.

**Rationale:**
- FR-011 (decision-authoring quality) needs a repeatable way to catch regressions
  when prompts/levels change (Principle 9).
- Adopts the guide's own methodology (re-run to estimate noise; enumerate failure
  families).

**Alternatives considered:**
- *No harness*: leaves FR-011 untestable; rejected.
