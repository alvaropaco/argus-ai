# Implementation Plan: Spec 006 — Fleet View

| # | Task | Repo | Touches |
|---|------|------|---------|
| T001 | BrainState shared handle; sentinel report payload + mapping + ReportKind + SentinelSource in SupervisorDeps | argus-ai | cloud.rs, brain.rs, mapping/, protocol |
| T002 | `brain` config kind applied live (BrainControl: autonomy/threshold/interval behind RwLock; loop reads per tick) | argus-ai | config.rs, cloud.rs, brain.rs |
| T003 | `approval.decision`/`approval.result` messages + daemon handler (grant → resume → result) | argus-ai | protocol, cloud.rs |
| T004 | Cloud: contracts types, sentinel validator, snapshot table + migration, gateway ingest + approval control push, API routes (GET sentinel, POST approvals) | app-platform | contracts, core, db, gateway, api |
| T005 | Web: instance page sentinel card, plans, approvals, autonomy control | app-platform | web |
| T006 | Verify both repos; deploy; live AC-001..005 on the VPS | both | — |

Key decisions:
1. **One new report, not five**: the sentinel report is a single coherent
   snapshot (view + brain + plans + approvals) — one ingest path, one
   table, one UI fetch.
2. **Autonomy is applied, not just stored**: `apply_configuration` writes
   managed settings as today and then updates the live `BrainControl`; the
   loop reads it per tick. A restart loses nothing (settings re-applied
   from the store at startup... actually the store already persists).
3. **Cloud approvals reuse the local grant machinery**: the daemon handler
   calls the same `grant_approval` + `resume_remediation` the CLI uses —
   one code path, one audit story. The cloud never touches tokens it was
   not shown in a sentinel report.
4. **Plans capped at 10 in reports**; the authoritative history stays in
   the instance's SQLite (queryable via `argus plans`).
