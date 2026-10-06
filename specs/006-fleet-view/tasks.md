# Tasks: Spec 006 — Fleet View

> AC mapping: T001+T004+T005→AC-001/002, T003+T004+T005→AC-003, T002→AC-004/005.

- [ ] T001 [P0] BrainState + sentinel report (payload, mapping, ReportKind, SentinelSource)
- [ ] T002 [P0] `brain` managed-config kind applied live (BrainControl)
- [ ] T003 [P0] approval.decision/result messages + daemon handler
- [ ] T004 [P0] Cloud: contracts + validator + snapshot table + gateway ingest + API routes
- [ ] T005 [P1] Web instance page: sentinel card, plans, approvals, autonomy control
- [ ] T006 [P0] Verify + deploy + live AC-001..005

## Verification Checklist

- [ ] Tests pass in both repos
- [ ] No secrets in reports; plans capped at 10
- [ ] Cloud approval crosses the same local boundary as the CLI
- [ ] Invalid autonomy rejected with the reason
