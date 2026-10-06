# Tasks: Spec 005 — Running Brain

> AC mapping: T001→AC-001/002, T002→AC-003, T003→AC-004/005, T004/T005→AC-001/006.

- [X] T001 [P0] `DeepSeekProvider` adapter (fail-closed, fallback models) + fixture tests
- [X] T002 [P0] `[model]`/`[brain]` config + provider factory + Daemon provider handle
- [X] T003 [P0] Brain loop: evidence → diagnose_once → run_remediation → memory + brain state; spawn in argusd
- [X] T004 [P1] IPC `brain.diagnose` + `runbooks.list` (0.7.0); runbook loading at init
- [X] T005 [P1] `argus diagnose` / `argus runbooks` CLI
- [X] T006 [P0] Verify + commit/push

## Verification Checklist

- [X] Tests pass (`cargo test --workspace`)
- [X] Model output never bypasses policy/executor (security suites green)
- [X] No secret reaches logs/events/IPC
- [X] Loop autonomy defaults to l0_observe
