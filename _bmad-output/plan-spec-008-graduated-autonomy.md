---
title: 'Spec 008 — Assimilation & graduated autonomy (earned, bounded, revocable)'
type: 'feature'
ticket: ''
created: '2026-10-10'
status: 'built'
baseline_revision: '8fb0c1a60c276146f6d83a6f5b795226e8af4715'
route: 'full'
route_source: 'auto'
review: 'quick'
review_source: 'pinned'
lenses_ran: ['quick']
review_loop_iteration: 0
followup_review_recommended: true
context: ['argus-ai/specs/008-assimilation-and-graduated-autonomy/spec.md', 'argus-ai/docs/adr/0040-autonomy-earned-bounded-revocable.md']
warnings: []
deferred: []
---

<intent-contract>

## Intent

**Problem:** Autonomy is a static operator dial (L0–L5) applied blind: nothing verifies the daemon knows the environment before acting in it, nothing bounds how much it may execute per period, and nothing lowers the dial when it misbehaves — so operators must choose between L0 forever and granting L3+ on faith.

**Approach:** Add a persisted assimilation state machine (MAP → SHADOW → EARNED) driven by the brain tick with deterministic gates, an earned rung promoted rung-by-rung and demoted on failure signatures, an effective autonomy of `min(managed ceiling, earned rung)` consumed everywhere `run_plan` reads autonomy, and a rolling blast-radius budget checked at the Allow branch that degrades exhaustion to `requires_approval` — all reported through additive sentinel fields and a web autonomy line.

## Boundaries & Constraints

**Always:** effective autonomy = `min(ceiling, earned)` — no path may exceed the managed `brain.autonomy` ceiling; fail-closed (state/budget errors degrade downward, never upward); budget exhaustion pauses via the existing approval machinery (`budget.exhausted` policy id), never denies and never executes; promotions/demotions emit `autonomy.promoted`/`autonomy.demoted` domain events AND ledger traces; state (phase, rung, gates, budget counters) persists in the local repository and survives restart; deterministic local checks only — no network in the state machine; additive sentinel fields only (the `view` map rides existing loose validation; no protocol bump, no IPC change); production default unchanged — absent a ceiling, effective stays L0.

**Never:** no promotion to L5 (learning stays operator-granted) and the ladder skips L1; no change to approval round-trip semantics or escalation ordering; no cloud-side autonomy state (cloud only reads the sentinel); no per-capability budgets (risk-class × scope only); no Cedar migration of the matrix; do not alter `may_execute_without_approval`'s signature/semantics for existing callers (the budget check composes before it); do not commit `.mimosa/` or user artifacts.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Fresh environment | no persisted AutonomyState | phase `mapping`, earned L0; state persisted | Persistence failure → effective stays L0 (fail-closed), logged |
| Shadow gate passes | `shadow_min_cycles` clean cycles (provider healthy, no critical incident, no SafeMode, no failed validation, cloud paired) | phase `earned`, promotion trace + event, effective = min(ceiling, L2) | Missing evidence keeps phase `shadow`, counters keep counting |
| Rung promotion | `rung_clean_cycles` clean at L2 (or L3), ceiling ≥ next rung | earned rung +1, `autonomy.promoted` trace + event | Ceiling below next rung → rung gate accumulates but effective is capped by ceiling |
| Failed validation | plan terminates RolledBack at earned >L0 | earned −1 rung, `autonomy.demoted` trace + event | Already L0 → demotion is a no-op (stays L0) |
| SafeMode entry | StaleInputs/ExecutionSuspended set | earned → L0 immediately; re-earning re-runs gates | — |
| Budget exhausted | window counter at cap when a low-risk step reaches the Allow branch | step becomes `requires_approval` with policy id `budget.exhausted`; window rolls with time | Budget store failure → treat as exhausted (fail-closed) |
| Ceiling lowered | managed `brain.autonomy` applied live below earned | effective drops immediately; earned rung unchanged (re-caps on raise) | — |
| Restart | daemon restarts mid-assimilation | phase/rung/counters resume from persisted state | Corrupt state → fresh `mapping` at L0 (fail-closed), logged |

</intent-contract>

## Code Map

**argus-ai** (test: `export PATH="$HOME/.cargo/bin:$PATH"; cargo test --locked --workspace --all-targets`; baseline 1055 passed; lints strict)
- `crates/argus-domain/src/reasoning.rs:21 AutonomyMode` (L0Observe..L5Adaptive, serde `l0_observe`..); `crates/argus-domain/src/capability.rs:13 RiskClass{Read,LowRisk,Controlled,HighRisk,Destructive}`; `capability.rs:24 Reversibility`; `id.rs:13 EnvironmentId(Uuid)`.
- `crates/argus-ai-core/src/decision/autonomy.rs:16 may_execute_without_approval(mode, risk)` — the existing matrix; the budget gate composes BEFORE it in the Allow branch; do not change its signature.
- `crates/argus-policy/src/escalation.rs:97 decide_escalation` — unchanged; consumes the effective autonomy passed in.
- `crates/argus-daemon/src/control.rs`: `run_plan` Allow branch (`PolicyOutcome::Allow` arm, ~:709+) calls `may_execute_without_approval(autonomy, risk)` — insert the budget check there; on exhaustion push `requires_approval` with policy id `budget.exhausted` and continue like the autonomy-gate arm (ledger event via `ports.ledger`, no execution). `ExecutionOutcome.plan_id` exists (007).
- `crates/argus-daemon/src/brain.rs`: `spawn()` ticker `:269+` (per-tick work) — the state machine ticks here; the cycle's run outcome is what the gates read.
- `crates/argus-daemon/src/brain_state.rs`: `BrainControlHandle` (ceiling lever, live-applied) — read the ceiling here.
- `crates/argus-daemon/src/sentinel.rs:36 SentinelView` — add `autonomy: Option<Map<String,Value>>` (skip-if-none; carries phase, gate progress, earned, ceiling, effective, budgets); `snapshot()` builds it. `crates/argus-cloud/src/protocol/messages.rs:373 SentinelReportPayload.view` is a `Map<String, Value>` — additive fields ride through with NO protocol/TS-validator change (core `validateSentinel` is loose on the view shape); cloud-side work is web-only.
- `crates/argus-daemon/src/runtime.rs`: `Daemon::new` wires repos/levers (007 `ledger` field pattern `:327`); `record_run_outcome`/Pending branch (007 traces) — hook the failed-validation demotion signal at the plan-terminal trace site; add a `daemon.autonomy()` accessor like `daemon.ledger()`.
- `crates/argus-daemon/src/config.rs:70 ConfigFile` — add optional `[autonomy]` (`AutonomyConfig`: `shadow_min_cycles` default 10 clamp 1–1000, `rung_clean_cycles` default 20 clamp 1–10 000, `budgets` with `low_risk_per_hour=20`, `controlled_per_day=5`, `host_scope_per_day=3`, clamped ≥1) — clone the `LedgerConfig` custom-Deserialize pattern (`:204`).
- `crates/argus-daemon/src/ledger.rs`: `DaemonLedger::record_trace` — promotions/demotions ride it (outcome `promoted`/`demoted`); new constants `AUTONOMY_PROMOTED`/`AUTONOMY_DEMOTED` in `crates/argus-events/src/lib.rs` types module.
- `crates/argus-state`: `repository.rs` default-Err pattern + `sqlite.rs` DDL block (007 tables `:100-127`) + `memory.rs` — add `put_autonomy_state`/`get_autonomy_state` (single row per environment, serialized `AutonomyState`: phase, earned rung, cycle counters, budget buckets with window anchors). The budget evaluator: new `crates/argus-policy/src/budget.rs` (pure + deterministic: `check(budgets, risk_class, scope, now)` and `record(...)`; anchored UTC hour/day buckets).
- SafeMode source: `crates/argus-daemon/src/sentinel.rs` SafeMode evaluation (`safe_mode_evaluate` already degrades escalation) — the demotion hook reads the same signal; provider health: `SentinelView.provider_ready` / runtime provider handle.
- Tests layout: `crates/argus-daemon/tests/` (budget-gate test beside `ledger.rs` with a RecordingLedger-style sink), `crates/argus-policy` in-file unit tests (window rollover), `crates/argus-state` sqlite/memory round-trips in-file.

**app-platform-argus-ai** (pnpm+turbo; `pnpm build` 8/8, `pnpm test` 12/12; api published on host port 13400)
- No contracts/core/db changes REQUIRED: new fields ride `SentinelReportPayload.view` (loose validation; `sentinelSnapshots.snapshot` jsonb passthrough; `GET /instances/:id/sentinel` already returns it). Touch them only if the web needs a typed shape — prefer defensive reads.
- `apps/web/components/instance/sentinel-panel.tsx` — autonomy line: `effective of ceiling`, phase progress (`shadow 7/10 cycles`), budget remaining chips; primitives `ui/primitives.tsx` (`Panel`, `Badge`, `KeyVal`, `stateTone`); `lib/types.ts` sentinel type gains optional `autonomy`; `useLiveQuery` already refreshes on `instance.sentinel.updated`.
- `apps/web/test/` — vitest configured; cover any extracted pure helper (e.g. phase-progress formatting).

## Tasks & Acceptance

**Execution:**
- [ ] `crates/argus-state` (+argus-domain) — `AutonomyState` record (phase, earned rung, shadow/rung cycle counters, budget buckets with window anchors, environment key), `put/get_autonomy_state`, SQLite `autonomy_state` table + in-memory twin, round-trip tests
- [ ] `crates/argus-policy/src/budget.rs` — deterministic rolling-window budget evaluator (`check`/`record`, anchored hour/day buckets, clamped config struct) + unit tests including window rollover
- [ ] `crates/argus-daemon/src/config.rs` — `[autonomy]` section (clamped defaults, absent-section = defaults)
- [ ] `crates/argus-daemon/src/autonomy.rs` (new) — state machine: tick from the brain loop, gates per spec (fail-closed), promote/demote with `autonomy.promoted/demoted` events + ledger traces, effective = `min(ceiling, earned)`, SafeMode/provider-degraded hooks, persistence; unit tests for every matrix row
- [ ] `crates/argus-daemon/src/control.rs` — budget gate in the Allow branch before `may_execute_without_approval`; exhaustion → `requires_approval` (`budget.exhausted`) with ledger event; `record` only after an auto-executed step lands
- [ ] `crates/argus-daemon/src/{sentinel.rs,runtime.rs,brain.rs}` — SentinelView additive `autonomy` map + snapshot wiring; demotion hooks at the plan-terminal trace site; state-machine ticks from the brain spawn loop; ceiling read from BrainControl
- [ ] `crates/argus-events/src/lib.rs` — `AUTONOMY_PROMOTED`/`AUTONOMY_DEMOTED` constants
- [ ] app-platform web — autonomy line on the instance page (effective of ceiling, phase progress, budget chips), defensive reads + type
- [ ] both repos — full verification; deploy CLOUD FIRST then daemon; live AC checks

**Acceptance Criteria:**
- Given a fresh paired instance with ceiling L2, when `shadow_min_cycles` clean cycles pass, then phase `earned` and effective L2 appear in the sentinel report (AC-001).
- Given ceiling L3 and the rung gate satisfied at L2, when the tick runs, then effective reaches L3 with an `autonomy.promoted` trace (AC-002).
- Given a failed validation with rollback at earned L3, when the plan terminates, then earned demotes one rung with an `autonomy.demoted` trace (AC-003).
- Given an exhausted low-risk budget, when another low-risk step reaches the Allow branch, then it pauses `requires_approval` with policy id `budget.exhausted` and nothing executes (AC-004).
- Given the cloud applies a lower `brain.autonomy`, when it lands live, then effective autonomy drops immediately while earned is preserved (AC-005).
- Given SafeMode entry, when the next tick or sentinel evaluation runs, then effective is L0 until gates re-earn (AC-006).
- Given a daemon restart mid-assimilation, when the state is read, then phase/rung/budget counters continue (AC-007).

## Implementation Notes

- Implemented by the full-route subagent (2026-10-10). argus-ai: `argus-domain/src/autonomy.rs` (AssimilationPhase, BudgetWindows, AutonomyState + ladder helpers rank/lower_of/next_rung/demote_rung on AutonomyMode), `argus-policy/src/budget.rs` (pure anchored-bucket evaluator: reserve/refund/remaining; BudgetLimits clamped ≥1), `argus-state` put/get_autonomy_state (SQLite singleton + in-memory), `argus-daemon/src/autonomy.rs` (AutonomyManager: load fail-closed, on_cycle revocation-before-earning, on_validation_failed, view map, BudgetGate impl), `LoopPorts.budget` (NoopBudget default), budget reserve at the Allow branch, brain tick wiring (CycleSignals from provider health/cloud_paired/selfobs safe mode/run outcome), SentinelView additive `autonomy` map via SentinelInputs.autonomy_view, `[autonomy]` config, `autonomy.promoted/demoted` event constants + ledger traces, daemon integration test in escalation_wiring.rs (ticks → earned L2 → sentinel → reload).
- Review (quick lens, 2026-10-10): 9 findings — 5 medium (dishonest MAP signal; idle cycles as rung evidence; no-op budget consumption; non-atomic check/record; exhaustion as ungrantable skip + grant-resume livelock), 4 low (NeedsManual gap, Debug-string signal, test hygiene, ceiling copy) — ALL patched (see Review Triage Log); none rejected. Re-verification: 1097 passed (+3 new tests), clippy/fmt clean, 12/12 cloud tasks.
- Documented judgment calls: `open_critical_incidents` stays an honest zero (no live incident source wired; sentinel reports the same); MAP→SHADOW proxied by provider readiness + real evidence (no-new-sensing); provider degradation demotes one rung per degraded tick (level-triggered, walks to L0, re-earns); the shadow window needs `shadow_min_cycles + 1` clean ticks (the mapping cycle is not shadow evidence) — visible in the sentinel progress line.

## Auto Run Result

- **Summary:** Spec 008 "assimilation & graduated autonomy" implemented end-to-end: a persisted per-environment state machine (MAP → SHADOW → EARNED) driven by the brain tick gates authority behind clean cycles and validated work; the earned rung promotes L2→L3→L4 (never L5, skips L1) and demotes automatically on SafeMode (→L0), failed validation with rollback or manual intervention (−1 rung), and provider degradation (−1 rung/tick); effective autonomy is `min(managed ceiling, earned)` and every consumer reads it; a rolling blast-radius budget (atomic reserve/refund, anchored hour/day buckets) bounds auto-executions and degrades exhaustion to a real, grantable approval pause; grants bypass the budget; the sentinel report and the web instance page carry the autonomy line (phase progress, earned of ceiling, budget chips).
- **Files changed:** argus-ai — `argus-domain/src/autonomy.rs` (new), `argus-policy/src/budget.rs` (new), `argus-daemon/src/autonomy.rs` (new), control/brain/runtime/sentinel/config wiring, `argus-state` persistence, `argus-events` constants, tests across policy/state/daemon (incl. budget-gate integration beside `tests/ledger.rs` and a daemon integration test). app-platform — `sentinel-panel.tsx` autonomy line, `lib/autonomy.ts` (new) + `tests/autonomy.test.ts` (new), `autonomy-control.tsx` ceiling copy, `lib/types.ts`.
- **Review findings:** quick lens, 9 findings — 8 patched (5 medium, 3 low), 1 low patched alongside (test hygiene); 0 rejected, 0 deferred. All re-verified.
- **Follow-up review recommendation: true** — 5 medium entries patched. Specific unverified risks: the reserve/refund pause path and the grant-bypass interaction are unit/integration-tested only; the live promotion path (real 10+ cycles to earned, live ceiling drop, budget exhaustion under real load) is unverified until deploy.
- **Verification performed:** argus-ai `cargo test --locked --workspace --all-targets` 1097 passed / 2 ignored (pre-review 1094), `cargo clippy --workspace --all-targets` 0 warnings, `cargo fmt --check` clean. app-platform `pnpm build` 8/8, `pnpm test` 12/12 (web vitest 11, 7 autonomy). Matrix audit: all eight I/O rows covered by tests that ran and passed. Diff reviewed in full by the orchestrator + quick lens.
- **Residual risks:** budget exhaustion under a paused plan parks on the operator until granted or the window rolls (by design; visible in `pending_approvals`); the assimilation gates rely on documented honest proxies (no live critical-incident source yet — same zero the sentinel reports); a sustained provider degradation walks the rung to L0 by design (level-triggered); live AC-001..007 checks outstanding until deploy.

## Plan Change Log

## Review Triage Log

### 2026-10-10 — Review pass (quick lens)
- verdicts: 9 findings — high 0, medium 5, low 4, false 0, maybe-false 0
- findings:
  - `[medium]` `[patch]` the MAP gate signal lies: `live_environment: true` is set even for cycles that early-returned without a live read, so mapping → shadow can complete on no evidence — verified at `brain.rs:424-427`. Fix: derive the signal from whether the cycle actually consulted live state (e.g. non-empty evidence); the honest-zero `open_critical_incidents` stays (documented, no incident source wired).
  - `[medium]` `[patch]` idle cycles count as rung-promotion evidence — a quiet paired host walks to L4 in ~51 idle minutes, against ADR-0040's "a quiet host proves nothing" — verified: every completed tick advances the gates. Fix: rung promotions additionally require ≥1 validated execution since the last rung change (the shadow window stays behavior-based — observation is work).
  - `[medium]` `[patch]` budget consumed by already-desired no-ops (zero blast radius) — with `host_scope_per_day: 3`, three no-ops exhaust the day — verified: `record` fires unconditionally after `execute_allowed_step`. Fix: reserve/refund accounting (with F5).
  - `[medium]` `[patch]` budget `check` and `record` are not atomic — concurrent plans can overshoot the window cap by the number of in-flight steps — verified: two separate mutex acquisitions with execution between. Fix: shared with F3 — an atomic reserve/refund gate.
  - `[medium]` `[patch]` budget exhaustion is a skip, not an approvable pause: no `PendingPlan`/token is built, so the step never appears in `pending_approvals` and can never be granted; and on the resume path a granted step would re-hit the budget gate (livelock), since the Allow branch checks the budget before any grant-awareness — verified against the RequireApproval/escalation arms and the 007 resume test (`policy_id == "plan.approval"` rides the same Allow branch). Fix: exhaustion pauses the PLAN via the existing machinery; grant-backed decisions (`plan.approval`) bypass the budget reserve — the operator's grant is authority the budget does not ration.
  - `[low]` `[patch]` `PlanStatus::NeedsManual` (failed validation whose rollback also failed) never demotes — narrower than the failure surface; the adjacent selfobs code treats it as failed validation — verified at `runtime.rs:766-771`. Fix: demote on `RolledBack | NeedsManual`.
  - `[low]` `[patch]` the `failed_validation` cycle signal is keyed to the `Debug`-format string `"RolledBack"` — a string-typed duplicate of a typed fact that will rot — verified at `brain.rs:428`. Fix: a shared typed helper (`is_validation_failure(PlanStatus)`) used by both the signal and the runtime hook.
  - `[low]` `[patch]` the new wiring test leaks a PID-keyed SQLite file in `$TMPDIR` and its fresh-machine assertions go flaky on PID reuse — verified. Fix: unique path + cleanup.
  - `[low]` `[patch]` the autonomy control's copy still sells the pre-008 semantics ("Autonomy set to X") although ADR-0040's consequence requires ceiling framing — verified in `autonomy-control.tsx`. Fix: label/description/toast say ceiling.
- patches applied (re-engaged step-03 implementer; full re-verification green: argus-ai 1097 passed / clippy 0 warnings / fmt clean; app-platform 12/12 tasks + web vitest 11): `live_environment` derived from actual evidence (`!record.evidence.is_empty()`); rung promotions now require `validated_actions_since_rung ≥ 1` (fed from Completed executions at the plan-terminal site, reset on transition; shadow window stays behavior-based); budget gate became atomic `reserve`/`refund` (no-ops and post-reserve pauses refund; failed-rolled-back keeps consumption; concurrent plans cannot overshoot); exhaustion pauses the PLAN via the approval machinery (token issued, `pending_approvals` shows it) and grant-backed decisions (`plan.approval` / resumed) bypass the reserve; demotion covers `RolledBack | NeedsManual` via the shared typed `argus_domain::is_validation_failure` used by both the runtime hook and the brain signal (`BrainCycle.validation_failed: bool`); wiring test keyed on uuid with cleanup; autonomy-control copy now ceiling-framed.

## Design Notes

- One computation point: the new daemon autonomy module owns state + evaluator and exposes `effective()`, `on_cycle(...)`, `check_and_record(...)`; control.rs and brain.rs consume it — never recompute.
- Budget windows are anchored UTC hour/day buckets, not sliding: deterministic, restart-safe, cheap; a bucket is `record`ed only for steps that actually auto-executed.
- Gate signals are numbers the daemon already computes (provider_ready, open critical incidents, SafeMode, plan outcomes from the 007 ledger path) — no new sensing.
- The web reads `snapshot.autonomy` defensively (`?.`) so old snapshots render fine.

## Verification

**Commands:**
- argus-ai: `export PATH="$HOME/.cargo/bin:$PATH"; cargo test --locked --workspace --all-targets` — expected: all pass (baseline 1055), `cargo clippy --workspace --all-targets` 0 warnings, `cargo fmt --check` clean.
- app-platform: `pnpm build && pnpm test` — expected: 8/8 build, 12/12 tasks.

**Manual checks (post-deploy, cloud first):**
- Instance page shows the autonomy line with phase `shadow 0/10`; after ~10 brain cycles phase `earned` (ceiling permitting); lowering the ceiling via the autonomy control drops effective immediately.
