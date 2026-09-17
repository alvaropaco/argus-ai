# ADR-011: LanceDB for State Storage

- **Status:** Accepted
- **Date:** 2026-09-16

## Decision

ARGUS will use **LanceDB** as its primary embedded state and knowledge storage layer.

LanceDB will support ARGUS use cases involving structured operational state, historical information, semantic retrieval, embeddings, incident knowledge, runbooks, environment knowledge, and AI-oriented memory.

The architecture must keep storage access behind an ARGUS storage abstraction so that external or distributed backends can be introduced later without coupling the core to LanceDB internals.

## Rationale

A unified local storage technology capable of supporting both operational data and AI/semantic workloads reduces the number of mandatory external services in a single-host installation.

## Consequences

- ARGUS can operate without requiring a separate database server.
- Semantic and vector-oriented workloads can be colocated with local state.
- Storage schemas and lifecycle management become ARGUS responsibilities.
- The project must validate LanceDB's suitability for transactional/audit requirements and avoid relying on it for guarantees it does not provide.

Security-sensitive audit requirements must remain append-safe and independently validated by the ARGUS storage layer.