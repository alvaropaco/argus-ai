# Tasks: Spec 007 — Nervous System

> AC mapping: T001+T004+T005+T006+T007→AC-001/002, T002+T006+T007→AC-003,
> T003+T006→AC-004, T004→AC-005, T005+T006→AC-006/008, T006+T007→AC-007.

- [ ] T001 [P0] ActionEvent type, emission hook at policy/executor boundary, append-only ledger + retention
- [ ] T002 [P0] Brain trace event per diagnose cycle, correlated to actions
- [ ] T003 [P0] Provider `usage` parsing → per-call events + cycle/period rollups
- [ ] T004 [P0] Redaction module (tested, fail-safe) + upload mapping
- [ ] T005 [P0] Protocol kinds + buffered at-least-once streaming with backpressure policy
- [ ] T006 [P0] Cloud: contracts, ledger tables + migrations, ingest, REST APIs, SSE fan-out, silence tracker
- [ ] T007 [P1] Web: Activity tab (live tail, filters, detail drawer), silence alarm
- [ ] T008 [P0] Verify + deploy + live AC-001..008

## Verification Checklist

- [ ] Tests pass in both repos
- [ ] Denied attempts appear in the ledger (not only successful actions)
- [ ] Redaction: secret-shaped args never reach the cloud; raw stays local
- [ ] Daemon kill/restart loses no buffered events; ordering preserved
- [ ] Burst test: zero ledger loss; traces coalesce, usage batches
- [ ] Missing provider `usage` renders as unknown, never zero or invented
- [ ] Silenced instance alarms on the dashboard and clears on recovery
- [ ] No new infra required to run this spec end-to-end
