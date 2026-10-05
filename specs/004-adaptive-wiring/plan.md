# Implementation Plan: Spec 004 — Adaptive Wiring

## Approach

Pure wiring: every behavior already exists as a tested function in
spec-003 crates. This spec connects them to the daemon's live state and
surfaces (state, config, IPC, TUI) without changing any decision logic —
the only behavioral change is FR-004's escalation gate, which strictly
narrows what may execute at L4/L5.

## Tasks

| # | Task | Touches |
|---|------|---------|
| T001 | Persist episodes/facts/procedures behind `DomainRepository` | `argus-state` |
| T002 | Runbook TOML wire format + directory loader | `argus-runbooks` |
| T003 | Daemon self-observability hooks + `sentinel_snapshot()` | `argus-daemon` |
| T004 | Escalation gate in the run path (L4/L5 only) | `argus-daemon::control` |
| T005 | IPC `sentinel.get` + `report.generate` (protocol 0.6.0) | `argus-ipc`, `argus-daemon::handler` |
| T006 | TUI Sentinel tab | `argus-ai-tui` |
| T007 | Verification + docs + commit | workspace |

## Key decisions

1. **Escalation applies at L4/L5 only.** L0–L3 contracts are already
   stricter or equal (L3 = policy-permitted Read/LowRisk). Wiring the full
   decision at L3 would silently change M3 semantics; L4 "bounded
   autonomous execution" is exactly where the decision belongs.
2. **Coarse self-observability recording at the daemon boundary.** Run
   outcomes (denials, failures, validations, decisions) are counted when
   `run_remediation`/`resume_remediation`/dispatch return — not threaded
   into `control.rs` internals. Real, honest, and one hop coarse.
3. **Runbook gates replay, not trust.** A TOML file may record earned
   gates; the loader replays them through the same in-order ladder, so a
   file cannot grant itself a promotion the ladder would refuse.
4. **Reports read the daemon's real tables** (executions, audit,
   approvals) and nothing else; empty sources render as honest zeros.

## Verification

`cargo test --workspace` (new suites in argus-state, argus-runbooks,
argus-daemon: sentinel IPC + escalation gating; TUI data test), clippy
`-D warnings`, fmt, plus the existing M3/M4 security suites staying green
(proof the escalation gate changed nothing at L0–L3).
