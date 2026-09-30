# ADR-0038: Operational Memory Layers

- **Status:** Accepted
- **Date:** 2026-09-30

## Context

CAP-16 requires five memory layers — working, operational, episodic, semantic,
and procedural — so ARGUS remembers incidents, root causes, remediations,
topology, baselines, runbooks, dependencies, and recurring failures, and can say
"this looks like three prior incidents." spec-argus pins retrieval to
deterministic keyed queries (no embeddings), and domain-model §12 says vector
similarity must never be the basis for an authorization or destructive
decision. The tension is whether "semantic memory" implies embeddings.

## Decision

### 1. All five layers are structured, deterministic records

- **Working memory** — in-process, per-investigation context (bounded evidence
  under investigation); not persisted long-term.
- **Operational memory** — current entity/graph state (the environment graph,
  `argus-correlate`).
- **Episodic memory** — incident, execution, and evidence records (the audit and
  incident history).
- **Semantic memory** — typed facts and graph relationships (entity attributes,
  known dependencies, baseline models); **not** embeddings.
- **Procedural memory** — runbooks and learned procedures (`argus-runbooks`).

### 2. Semantic memory is typed facts, never embeddings

"Semantic" here means structured, typed knowledge (facts, relationships,
baselines) retrieved by deterministic, keyed, typed queries. There is no
vector/semantic search in the core. Incident similarity is computed over typed
fields (subject, symptom signature, resource class, change proximity), not over
embeddings.

### 3. Persistence stays behind the repository abstraction

All layers persist through `DomainRepository` (SQLite interim, ADR-0019;
LanceDB remains the target). `argus-memory` never imports store APIs. If
LanceDB vector similarity is ever used, it is an optional, advisory similarity
*input* — never an authorization or destructive-decision basis.

## Consequences

### Positive

- "Deterministic-only retrieval" (spec-argus) is preserved; the memory model is
  testable without a vector store or a model.
- Memory is auditable: every "similar incident" match is a typed query with
  cited evidence.

### Negative

- No free-form semantic recall; similarity quality is bounded by the typed
  signature quality.

## Implementation Principles

1. Structured, deterministic records for every layer.
2. Semantic memory = typed facts and relationships, never embeddings.
3. Similarity over typed fields; vector similarity, if any, is advisory only.
4. Persist through the repository abstraction only.

## Reference

- `_bmad-output/specs/spec-argus/SPEC.md` — deterministic retrieval constraint.
- `_bmad-output/specs/spec-autonomous-operations-brain/SPEC.md` — CAP-16.
- `_bmad-output/specs/spec-autonomous-operations-brain/glossary.md` — memory
  layers.
- `docs/architecture/domain-model.md` — §12 Persistence Model.
- `docs/adr/0011-lancedb-state-storage.md`, `0019-sqlite-interim-state-backend.md`.
