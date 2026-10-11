---
title: 'Spec 013 — Fleet aggregation & provider cost tables'
type: 'feature'
ticket: ''
created: '2026-10-11'
status: 'ready-for-dev'
baseline_revision: '2b6dbd6e1f0dae2f1e57d9d0b4c9d0f96dc8a94c'
route: 'full'
route_source: 'auto'
review: ''
review_source: ''
lenses_ran: []
review_loop_iteration: 0
followup_review_recommended: false
context: []
warnings: []
deferred: []
---

<intent-contract>

## Intent

**Problem:** The fleet dashboard aggregates by N+1 browser fan-out (per-instance sentinel/activities/events fetches), there is no server-side cross-instance summary, and spec 007's token metering never becomes the number operators budget with — cost.

**Approach:** A server-side `GET /v1/tenants/:tenant_id/fleet/summary?window=` rollup over the existing tables (instances, sentinel_snapshots, action_events, token_usage, brain_traces — no daemon changes, no new activity tables); a per-tenant model price map (seeded catalog, admin-editable, audited) applied to usage rollups as `cost_usd` (exact from metered tokens, `null` for unknown models); the dashboard renders the summary panel in one request and the instance Activity panel shows cost beside tokens. NOTE: this spec lives in the app-platform repo (cloud+web only, by design) — the spec file is `app-platform-argus-ai/specs/013-fleet-aggregation-cost-tables/spec.md`.

## Boundaries & Constraints

**Always:** cost math is deterministic — prompt_tokens × input price + completion_tokens × output price (USD per million tokens), rounded half-up to 4 decimals; unknown model → `cost_usd: null` (never zero, never invented); aggregation is server-side SQL over existing tables with bounded windows and the existing indexes; reads are viewer-role, the price write is admin-role and audited (`AuditAction` new entry); the summary is ONE request; silence counts server-side use the same rule as the instance page (3×30s since last_seen_at, a stale-but-connected instance counts by its data); live-update behavior (SSE-triggered refetch) is preserved; existing panels/tables are untouched beyond additive rendering.

**Never:** no daemon changes; no per-instance fan-out for the summary; no new activity tables (only the price map); no historical trend charts beyond window totals; no budget ceilings/alerting; no cross-instance procedure/runbook surfaces; no cost invented for unknown models; do not commit `.mimosa/` or user artifacts.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Summary query | `?window=24h` (default) or `7d` | One response: instance counts (total/healthy/silent), pending approvals total + per-instance rows, activity by outcome, tokens by model with cost | SQL over existing tables; empty tenant → zeroed totals, empty arrays |
| Known model priced | usage rows + price map entry | `cost_usd` = prompt×in + completion×out / 1M, rounded 4dp | Price absent → null |
| Unknown model | usage rows for a model off the map | `cost_usd: null`; tokens still counted | Never invented |
| Price edit | admin PUT model-prices | Map replaced; audited; subsequent rollups use it | Non-admin → 403; malformed → 400 |
| Empty fleet | tenant with no instances | Zeroed totals, empty arrays; dashboard renders zeros honestly | — |
| Silent instances | last_seen_at older than 3×30s | Counted in `silent`; per-instance rows link to the instance page | — |

</intent-contract>

## Code Map

**app-platform-argus-ai** (pnpm+turbo; `pnpm build` 8/8, `pnpm test` 12/12; the spec file is in THIS repo)
- `packages/db/src/schema/` — new `modelPrices` table (`packages/db/src/schema/observability.ts` or a new `pricing.ts`: tenant_id, model unique-per-tenant, inputPerMtok/outputPerMtok numeric) + `infra/migrations/0007_model_prices.sql` (the 0006 pattern; auto-applies on api boot).
- Seed: the default catalog (deepseek-chat etc.) inserts via the migration or on-first-read in the api (migration is simpler and deterministic — seed INSERT ON CONFLICT DO NOTHING).
- `apps/api/src/routes/` — NEW `fleet.ts`: `registerFleetRoutes` → `GET /v1/tenants/:tenant_id/fleet/summary` (viewer): one withTenant tx with the aggregate queries — instances (count/group by health_state/status + silent via last_seen_at < now()-interval '90 seconds'), pending approvals (from latest sentinel_snapshots per instance: `pending_approvals` column), action_events by outcome for the window, token_usage by model for the window (tokens + cost join the price map), pressure situations (latest sentinel snapshot per instance: `snapshot->'view'->'pressure'`), per-instance rows carrying pending approvals/pressure with id+name. Register in `app.ts` beside the other tenant routes. `GET`/`PUT /v1/tenants/:tenant_id/model-prices` (admin for PUT; audited `ModelPricesUpdated`) — the commands.ts audit pattern.
- `apps/api/src/routes/instance-detail.ts` usage route — extend the rollup with `cost_usd` joining the price map (LEFT JOIN on model; null when absent).
- `apps/web` — dashboard (`apps/web/app/(app)/dashboard/page.tsx`) gains a fleet summary panel fed by one `useLiveQuery` of the summary endpoint (count tiles reusing `StatTile`, activity chips, spend + tokens, pending-approval links to instance pages); `apps/web/components/instance/activity-panel.tsx` usage header shows `cost_usd` beside tokens (defensive); `lib/types.ts` types.
- Tests: api route tests (summary equals seeded per-instance sums; cost math exact; unknown model null; price PUT admin-gated + audited), per the existing route-test layout.

## Tasks & Acceptance

**Execution:**
- [ ] db — model_prices table + 0007 migration + seeded default catalog
- [ ] api — fleet/summary route (aggregates + silence + pending + pressure rows, one tx)
- [ ] api — model-prices GET/PUT (admin, audited) + usage rollup `cost_usd`
- [ ] web — dashboard fleet summary panel (one request, live refetch) + Activity cost display + types
- [ ] verify + deploy cloud (no daemon changes) + live checks

**Acceptance Criteria:**
- Given seeded usage and instances, when the summary is queried for 24h, then counts and totals equal the per-instance sums in one request (AC-001).
- Given a priced model, when usage is rolled up, then `cost_usd` is exact per the formula; an unknown model carries `null` (AC-002).
- Given an admin PUT of a price for an unknown model, when rollups re-run, then cost appears and the change is audited (AC-003).
- Given the dashboard open, when a fleet event fires, then the summary panel refreshes and shows counts/activity/spend/links (AC-004).
- Given a silent instance, when the summary is queried, then it is counted and linked (AC-005).

## Implementation Notes

- Implemented by the full-route subagent (2026-10-11), cloud+web only per the plan: `model_prices` table (per-tenant composite PK, numeric(12,4)) + migration 0007 (table, seeded catalog, RLS on the four ledger/sentinel tables, occurred_at indexes); `GET /fleet/summary` (one tx, zero fan-out); `GET|PUT model-prices` (admin, audited); usage rollup `cost_usd`; dashboard `fleet-summary-panel.tsx` + Activity `$` display. Route tests against a real Postgres 17 under RLS (testcontainers/TEST_DATABASE_URL pattern).
- Review (quick lens, 2026-10-11): 6 findings — the HIGH was a cross-tenant data leak (no RLS on the 0006/0004 tables, exposed by the first tenant-wide aggregation); 5 patched (RLS + explicit predicates + regression test; tenant-creation seeding; unbounded totals; unpriced-usage renders unavailable; occurred_at indexes), 1 deferred with reasoning (Docker-gated tests — repo convention). All re-verified against a real database.
- Deploy + live (2026-10-11): pushed and deployed via the cloud pipeline; migration 0007 auto-applies on api boot.

## Auto Run Result

- **Summary:** Spec 013 closed the last scoped deferrals of the arc: the fleet summary is a one-request server-side rollup (instance counts/silence, pending approvals + pressure situations from the latest snapshots, activity by outcome, tokens by model with cost from the per-tenant price map — seeded at migration and at tenant creation, admin-editable with audit); unknown models cost null (never zero) and unpriced usage renders the spend as unavailable with the unpriced list; the dashboard renders it all in one live request. No daemon changes.
- **Review findings:** 6 — 5 patched (1 HIGH cross-tenant leak + 3 medium + 1 low), 1 deferred with reasoning (Docker-gated tests). All re-verified against a real database.
- **Follow-up review recommendation: false** — the high and all mediums patched with database-verified regression tests.
- **Verification performed:** pnpm build 8/8, test 12/12, typecheck 13/13, lint clean; the new api suite ran against a real Postgres 17 with RLS verified as the non-superuser role; the leak regression fails on the pre-fix path. Diff reviewed by the orchestrator + quick lens.
- **Residual risks:** the per-model list is bounded at 200 for rendering (totals are unbounded and authoritative); Docker-gated tests skip in CI without Docker (repo convention).

## Plan Change Log

## Review Triage Log

### 2026-10-11 — Review pass (quick lens)
- verdicts: 6 findings — high 1, medium 3, low 1, deferred 1
- findings:
  - `[high]` `[patch]` CROSS-TENANT LEAK: the 0006/0004 ledger tables carry no RLS and the fleet summary is the first tenant-wide aggregation over them — its queries filter only by `occurred_at` — verified empirically by the reviewer against a two-tenant database. Fix (both layers): RLS policies for `sentinel_snapshots`/`action_events`/`brain_traces`/`token_usage` in migration 0007 (the 0002 pattern) + explicit `tenant_id` predicates in the fleet queries (defense in depth) + a two-tenant regression test that fails on the pre-fix path.
  - `[medium]` `[patch]` the seeded price catalog never reaches tenants created after the migration (the cross JOIN covers existing tenants only) — verified empirically. Fix: `createTenantWithAdmin` seeds the same default catalog.
  - `[medium]` `[patch]` fleet totals summed `LIMIT 200`-truncated rows — silent undercount past 200 distinct models. Fix: unbounded aggregate queries for the totals; the per-model list keeps its bound.
  - `[medium]` `[patch]` partial pricing rendered an understated spend as complete (unpriced models contributed 0, no uncertainty surfaced), inverting "unknown is never zero" at fleet level. Fix: `total_cost_usd` null + `unpriced_models` when unpriced usage exists; the panel renders the uncertainty.
  - `[low]` `[patch]` the window queries cannot use the composite indexes (leading column absent) and `brain_traces` has no occurred_at index at all (sequential scans). Fix: `(occurred_at DESC)` indexes in the same migration.
  - `[defer]` the AC-covering tests skip without Docker (`skipIf` — repo-wide convention, not introduced here); the implementer verified against a real Postgres manually. Making Docker mandatory is a repo-wide CI decision, deferred with reasoning.
- patches applied (re-engaged step-03 implementer; verified against a real Postgres 17 under RLS as the non-superuser app role: RLS+FORCE active on all five tables, the two-tenant regression fails on the pre-fix path, totals/list agreement, unpriced-usage semantics).

### 2026-10-11 — Review pass (quick lens)
- verdicts: 6 findings — high 1, medium 3, low 1, false 0, maybe-false 1 (test-infrastructure, deferred with reasoning) — wait, kept as low-patch-adjacent below
- findings:
  - `[high]` `[patch]` CROSS-TENANT LEAK: `action_events`/`brain_traces`/`token_usage` (0006) and `sentinel_snapshots` (0004) carry NO row-level security, and the fleet summary is the first tenant-wide aggregation over them — its queries filter only by `occurred_at`, so one tenant's summary counts/prices another tenant's actions, traces, tokens and spend (empirically confirmed by the reviewer against a two-tenant database; the sentinel query escapes only by accident via its `JOIN instances`, which does have RLS). Pre-existing gap EXPOSED by this change — the exposure is caused by it. Fix: RLS policies for the four tables in migration 0007 (the 0002 pattern, `app.tenant_id` context) AND explicit `tenant_id` predicates in the fleet queries (defense in depth), plus a two-tenant regression test.
  - `[medium]` `[patch]` the seeded price catalog never reaches tenants created after migration 0007 — `createTenantWithAdmin` inserts no `model_prices` rows, so every post-deploy tenant has costs `null` forever — verified empirically. Fix: seed the default catalog in `createTenantWithAdmin` as well.
  - `[medium]` `[patch]` fleet totals sum `LIMIT 200`-truncated row sets: with >200 distinct models the headline tokens/spend silently exclude the tail — verified. Fix: totals via separate unbounded aggregate queries (count/sum without LIMIT); the per-model list keeps its bound.
  - `[medium]` `[patch]` partial pricing renders an understated spend as complete: when unpriced models have usage, the fleet total sums priced rows only and the panel shows `$X · priced map` with no uncertainty — inverting the "unknown is never zero" rule at fleet level — verified. Fix: when any unpriced model has usage in the window, `total_cost_usd` is `null` with an `unpriced_models` list/count, and the panel renders the uncertainty explicitly.
  - `[low]` `[patch]` the window queries cannot use the `(instance_id, occurred_at)` indexes (leading column absent) and `brain_traces` has no `occurred_at` index at all (sequential scans on an append-only table) — verified. Fix: add `(occurred_at)` indexes for `action_events`/`token_usage`/`brain_traces` in the same migration.
  - `[low]` `[defer]` the only tests covering the ACs skip without Docker (`skipIf`) — repo-wide convention, not introduced here; making Docker mandatory is a repo-wide CI decision beyond this spec's scope, and the implementer verified the suite against a real Postgres manually. Deferred with reasoning; noted in Auto Run Result.

## Design Notes

- One request, zero fan-out: the summary is the product; the web panel is its renderer.
- The price map is per-tenant (multi-tenant pricing is a business reality); the DEFAULT catalog is seeded so the first rollup already carries deepseek costs.
- Cost is derived from METERED tokens only (spec 007's rule, now monetized): absent prices → null, never estimated from nothing.
- Silence is computed server-side with the same 3×30s rule the web helper uses — one rule, two surfaces, no drift.

## Verification

**Commands:**
- app-platform: `pnpm build && pnpm test` — expected: 8/8, 12/12 (+ new route tests).
- argus-ai: untouched — no verification needed beyond confirming no cross-repo drift.

**Manual checks (post-deploy):**
- The dashboard shows the fleet summary panel with real counts/spend; the instance Activity header shows $; `PUT model-prices` for a new model moves its cost from null to computed.
