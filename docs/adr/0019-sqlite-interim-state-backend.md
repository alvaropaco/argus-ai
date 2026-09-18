# ADR-019: SQLite as Interim State Backend

- **Status:** Accepted
- **Date:** 2026-09-16

## Context

ADR-011 selects LanceDB as the primary embedded operational state store. During
bootstrap implementation (spec `001-bootstrap`), the LanceDB Rust SDK was evaluated
for use as the initial persistence backend.

The evaluation found:

1. **Upstream compile bug** in the latest release (`lancedb 0.38.0`): `job.rs`
   references `Error::Http` unconditionally, but that variant is gated behind the
   `remote` feature, so the crate does not compile with default features.
2. **Very heavy dependency tree**: LanceDB pulls in DataFusion, `lance`, Arrow, and
   related crates, making the first build take on the order of tens of minutes.

This matches the ADR-011 caveat: "The project must validate LanceDB's suitability
for transactional/audit requirements."

## Decision

ARGUS uses **SQLite as the interim operational state backend** during the bootstrap
and until the LanceDB Rust SDK is production-ready.

- The `argus-state` crate exposes the `DomainRepository` trait (unchanged from the
  architecture) with two implementations:
  - `SqliteRepository` — the interim **default** backend (via `rusqlite`, bundled).
  - `LanceDbRepository` — retained as an **optional** implementation behind the
    `lancedb` cargo feature (disabled by default).
- The domain model remains backend-agnostic; no SQLite or LanceDB types cross the
  repository boundary.

## Consequences

### Positive

- Fast, deterministic local builds (no DataFusion/Arrow in the default graph).
- SQLite provides ACID/append-safe semantics sufficient for bootstrap audit state.
- The `DomainRepository` abstraction is exercised by a second, real backend, which
  strengthens the persistence boundary.
- LanceDB remains a drop-in replacement when its SDK stabilizes.

### Negative

- SQLite is explicitly out of scope in spec § 3 ("SQLite is NOT used") — this ADR
  temporarily supersedes that constraint.
- Vector/embedding workloads are deferred until LanceDB is re-enabled.

## Scope

This ADR records an **interim** decision. It does not change ADR-011's long-term
selection of LanceDB as the primary state store; it only defers LanceDB adoption
until the SDK is production-ready. When LanceDB is re-enabled, this ADR should be
superseded or amended.

## References

- ADR-011 (LanceDB for State Storage)
- `specs/001-bootstrap/plan.md` § 6, § 11, § 12
- `specs/001-bootstrap/research.md` § 4
