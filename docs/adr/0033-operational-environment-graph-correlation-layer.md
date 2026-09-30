# ADR-0033: Operational Environment Graph as the Correlation Layer

- **Status:** Accepted
- **Date:** 2026-09-30

## Context

`domain-model.md` §7 describes a unified correlation graph (Internet → Ingress →
Service → Deployment → Pod → Container → Cgroup → PID → Namespace → Socket) and
asks questions like "which application owns this process?" and "which deployment
owns this container?". Nothing implements it. CAP-5 and CAP-6 require a live
environment graph and event correlation into situations. The graph must correlate
layers, not treat them as independent telemetry.

## Decision

### 1. The graph is a new `argus-correlate` crate

`argus-correlate` owns (a) the in-memory entity graph with typed nodes and edges
and (b) the event-correlation engine that folds related events into
`Situation`s. It consumes `Observation` and `DomainEvent` inputs and produces
graph state plus `Situation` events.

### 2. Nodes and edges are typed domain values

Node kinds and relationship kinds live in `argus-domain` as enums (or a
`graph` module) — never as strings. The node set is the one in
`glossary.md`: Environment, Host, Kernel, Process, Cgroup, Namespace, Device,
Filesystem, Network, Service, Container, Workload, Endpoint, Dependency,
Application, Kubernetes Cluster/Node/Namespace/Deployment/ReplicaSet/Pod/
Container, Cloud Resource, Observation, Incident, Evidence, Plan, Action,
Execution, Agent, Plugin. Relationships: belongs_to, runs_on, depends_on,
provides, calls, contains, owns, managed_by, exposes, connected_to, caused_by,
affected_by, deployed_by, derived_from.

### 3. The graph is derived from immutable observations

Nodes/edges are derived and rebuildable from observations and events; the
observation log stays the source of truth. Graph state may be rebuilt from the
event/observation stream at any time (deterministic reconstruction).

### 4. Correlation is temporal + causal

Events correlate into a `Situation` by shared subject, temporal proximity, and
causal linkage (`caused_by`/`affected_by`/`deployed_by`). A single high sample
never becomes a situation on its own; a chain (deploy → restart → OOM →
latency → incident) does.

### 5. Persistence stays behind the repository abstraction

Graph state is persisted through `DomainRepository` (SQLite interim, ADR-0019);
`argus-correlate` never talks to a store API directly. The graph is not a
database — it is a projection over the canonical event/observation store.

## Consequences

### Positive

- The cross-layer "why is this pod slow" question becomes answerable in one
  place.
- Correlation is deterministic and testable without a host or a model.
- Graph state is reconstructible, so corruption is recoverable.

### Negative

- A new crate and a typed graph vocabulary to maintain.
- Graph maintenance cost grows with entity count; needs bounded retention.

## Implementation Principles

1. Typed nodes/edges; never stringly-typed graph labels.
2. Derive from immutable observations; never store inference in the observation
   log.
3. Correlate by subject + time + causality; a single sample is never a situation.
4. Persist through the repository abstraction only.

## Reference

- `docs/architecture/domain-model.md` — §7 Correlation Graph, §3.14 Dependency.
- `_bmad-output/specs/spec-autonomous-operations-brain/SPEC.md` — CAP-5, CAP-6.
- `_bmad-output/specs/spec-autonomous-operations-brain/glossary.md` — node and
  relationship catalog.
- `crates/argus-state/src/repository.rs` (abstraction), `crates/argus-events`.
