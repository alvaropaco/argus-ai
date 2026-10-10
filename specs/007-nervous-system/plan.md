# Implementation Plan: Spec 007 — Nervous System

| # | Task | Repo | Touches |
|---|------|------|---------|
| T001 | `ActionEvent` type + emission hook at the policy/executor boundary + append-only local ledger (repository table, retention) | argus-ai | executor, policy, domain, state |
| T002 | Brain trace event per diagnose cycle (evidence refs, decision, plan, outcome; correlation wiring) | argus-ai | brain loop, ai-core |
| T003 | Token metering: parse `usage` in the provider adapter, per-call event + cycle/period rollups | argus-ai | ai-core decision adapters |
| T004 | Redaction module (tested patterns, fail-safe drop) + upload mapping for all three event kinds | argus-ai | new `redact`, argus-reporting mapping |
| T005 | Protocol message kinds `action.event`/`brain.trace`/`token.usage` + bounded buffered streaming, batching, backpressure policy | argus-ai | argus-cloud protocol, daemon cloud.rs |
| T006 | Cloud: contracts + validator, ledger tables + migrations, ingest, REST APIs (actions/usage/traces), SSE live fan-out, last-seen + silence state | app-platform | contracts, core, db, gateway, api |
| T007 | Web: Activity tab (live tail, filters, detail drawer with trace + usage), silence alarm on instance page and fleet list | app-platform | web |
| T008 | Verify both repos; deploy; live AC-001..008 on the VPS | both | — |

Key decisions:

1. **Emit at the boundary, not in the brain.** The policy/executor chokepoint
   sees every privileged operation regardless of origin (brain, CLI, cloud
   approval) — one emission point, no gaps. Denied attempts are events too.
2. **Local raw, cloud redacted.** The ledger is the source of truth on the
   host at full fidelity; the upload mapping applies deterministic redaction.
   The cloud can audit everything except secrets.
3. **Ride the existing WebSocket first (ADR-0039).** No new infrastructure in
   this spec. The event schema is transport-independent so the daemon leg can
   move to NATS later without changing payloads. NATS lands cloud-side in a
   following phase, when a second consumer actually exists.
4. **Ledger events are never dropped under pressure.** Backpressure degrades
   traces (coalesce) and usage (batch rollups), never the ledger. At-least-once
   with per-instance ordering via the existing offline buffer pattern.
5. **Usage is metered, never estimated.** No `usage` in a provider response is
   recorded as unknown. Silent estimation would corrupt the cost basis the
   operator is supposed to trust.
6. **Correlation is the product.** Cycle id threads trace → plan → actions →
   validation; the UI's job is to walk that chain in one click, mirroring the
   evidence-linked-answer pattern (cf. OpenSRE) for auditability.
