# Tasks: Spec 008 — Assimilation & Graduated Autonomy

> AC mapping: T003+T005→AC-001/002/006, T003→AC-003, T004→AC-004,
> T005+T006→AC-005, T001→AC-007.

- [ ] T001 [P0] AutonomyState persistence (repository + SQLite + in-memory)
- [ ] T002 [P0] Budget evaluator + `[autonomy]` config (clamped defaults)
- [ ] T003 [P0] Assimilation state machine (brain tick, gates, promote/demote, events + traces, effective autonomy)
- [ ] T004 [P0] Budget gate in run_plan (exhaustion → requires_approval `budget.exhausted`)
- [ ] T005 [P0] SentinelView/report additive fields (phase, rungs, budgets)
- [ ] T006 [P1] Cloud validation + API passthrough of new fields
- [ ] T007 [P1] Web autonomy line (effective of ceiling, phase progress, budget bars)
- [ ] T008 [P0] Verify + deploy (cloud first) + live AC-001..007

## Verification Checklist

- [ ] Tests pass in both repos (baseline 1055 / 12 tasks)
- [ ] No execution path can exceed the managed ceiling
- [ ] Budget exhaustion pauses (never denies) with `budget.exhausted`
- [ ] Demotions emit traces + events and are visible in the sentinel report
- [ ] Restart continues assimilation from persisted state
- [ ] Production default unchanged: absent ceiling, effective stays L0
