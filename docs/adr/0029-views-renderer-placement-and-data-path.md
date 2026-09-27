# ADR-0029: Views Renderer — Placement and Data Path

- **Status:** Accepted
- **Date:** 2026-09-27

## Context

Story 1.9 (CAP-18) must render human-facing explanations and summaries
deterministically from typed artifacts and evidence, with no free-form generated
text and no path from a view to an Action.

Investigation shows: no renderer or shared view abstraction exists — the only
`View` is a private TUI navigation enum
(`crates/argus-ai-tui/src/app.rs`), and both the TUI and CLI print raw JSON;
the presentation crates depend only on `argus-ipc` + `argus-ai-core`
(structurally no view→Action); no free-form text is generated anywhere (the
decision engines return typed answers; `ModelProvider` exposes no generation
method). But `brain-architecture.md` places the renderer "in the CLI/TUI", while
the CLI/TUI never hold typed `Plan`/`Observation`/`Decision*` artifacts — they
receive JSON over IPC, and no artifact read operations or observation reads
exist. The persisted plan artifact is only a `DomainEvent` payload
`{ objective, executed, confidence, provenance }` — actions, the chosen answer,
and the evidence are not persisted.

The placement and the data path are architectural, so they are recorded here.

## Decision

### 1. The renderer is a pure, deterministic module in `argus-ai-core`

It lives in `argus-ai-core` as a `view` module of pure functions over typed
artifacts, because it needs both the `argus-domain` artifacts (`Plan`, `Action`,
`Execution`, `Observation`, `DomainEvent`, `PolicyDecision`) and the reasoning
types (`DecisionOutcome`, `DecisionResponse`, `DecisionProvenance`); the CLI and
TUI already depend on `argus-ai-core`, so both consume the one implementation.
This honours `brain-architecture.md`'s "deterministic template renderer (no LLM)
… consumed by the CLI/TUI" while keeping it testable without IPC or a live
daemon.

### 2. The renderer takes typed artifacts; it does not read the audit log

Views are built from the typed artifacts passed in (plan, decision outcome,
provenance, evidence). Persisting a richer payload and adding read-only IPC
operations to supply artifacts to the CLI/TUI are **out of scope** and recorded
as follow-ups (`T027`/`T028`). 1.9 does not extend the persisted payload.

### 3. Determinism is the renderer's responsibility

Given a collection, the renderer imposes a stable order (by timestamp and id)
before rendering, because the audit repository returns rows without `ORDER BY`.
Golden tests pin exact output for fixed typed fixtures.

### 4. Invariants

- **No view→Action:** the renderer takes read-only references to artifacts and
  never reaches `ActionPort`/`Executor`/`authorize_and_execute`; the presentation
  crates stay executor-free.
- **No free-form text:** all output is `format!` of typed values plus static
  labels; no generator is called (none exists).

## Consequences

### Positive

- One renderer, exhaustively testable, reused by any human surface.
- The invariants hold structurally rather than by convention.
- No data or wire changes are needed to deliver CAP-18's success criteria.

### Negative

- The CLI/TUI do not yet display the rendered views (wiring deferred), so the
  renderer ships without a production consumer.
- Rendering from in-memory artifacts means a view cannot reconstruct a plan from
  the audit log until the payload is extended.

## Implementation Principles

1. Render only from typed artifacts; never from free text.
2. Order collections deterministically before rendering.
3. Keep the renderer a pure function; no IO, no model calls.
4. Never take an executor/action handle in a view type.
5. Pin output with golden tests for every view and its empty/no-decision cases.

## Reference

- `_bmad-output/specs/spec-argus/SPEC.md` — CAP-18 and the no-generated-text /
  no-view→Action constraint.
- `_bmad-output/specs/spec-argus/brain-architecture.md` — Views placement.
- `_bmad-output/specs/spec-argus/architecture-diagrams.md` — views after
  validation.
- `argus-ai/.specify/memory/constitution.md` — Principles 9, 10, 15, 17.
- Code: `argus-ai/crates/argus-domain/src/{reasoning.rs,observation.rs,event.rs,authorization.rs}`,
  `argus-ai/crates/argus-ai-core/src/decision/{gateway.rs,types.rs,provenance.rs,context.rs}`,
  `argus-ai/crates/argus-ai-tui/src/app.rs`,
  `argus-ai/crates/argus-ipc/src/protocol.rs`.
