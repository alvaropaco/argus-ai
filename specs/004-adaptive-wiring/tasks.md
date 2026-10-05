# Tasks: Spec 004 — Adaptive Wiring

> Implemented 2026-10-05: 1004 workspace tests green (17 new), clippy/fmt
> clean, kube feature compile-checked. AC mapping: T001→AC-001, T002→AC-002,
> T003+T005→AC-003, T004→AC-004, T005→AC-005, T006→AC-006.

## Tasks

- [X] T001 [P0] Persist episodes/facts/procedures behind `DomainRepository` (SQLite + in-memory)
- [X] T002 [P1] Runbook TOML wire format + deterministic directory loader
- [X] T003 [P0] Daemon self-observability hooks + `sentinel_snapshot()` from live state
- [X] T004 [P0] Escalation gate in the run path (L4/L5; L0–L3 unchanged)
- [X] T005 [P1] IPC `sentinel.get` + `report.generate` (protocol 0.6.0)
- [X] T006 [P1] TUI Sentinel tab
- [X] T007 [P0] Verify (fmt/clippy/tests) + spec docs + commit/push

## Verification Checklist

- [X] Tests pass (`cargo test --workspace`)
- [X] No new capability, privilege, or bypass (escalation only narrows)
- [X] Unknown figures stay unknown in every new surface
- [X] M3/M4 security suites unchanged and green
