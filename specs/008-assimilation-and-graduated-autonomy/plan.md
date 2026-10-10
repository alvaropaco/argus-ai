# Implementation Plan: Spec 008 — Assimilation & Graduated Autonomy

| # | Task | Repo | Touches |
|---|------|------|---------|
| T001 | `AutonomyState` record + repository put/get (phase, earned rung, gate counters, budget buckets; SQLite table `autonomy_state` + in-memory twin) | argus-ai | argus-domain, argus-state |
| T002 | Budget evaluator (rolling windows per risk class/scope, deterministic, clamped config `[autonomy]`) | argus-ai | new argus-policy `budget.rs` or domain module, config.rs |
| T003 | Assimilation state machine driven by the brain tick (gate checks, promote/demote with `autonomy.promoted/demoted` events + traces, effective = min(ceiling, earned), SafeMode/failure hooks) | argus-ai | argus-daemon new `autonomy.rs`, brain.rs, runtime.rs, sentinel snapshot |
| T004 | Budget gate in `run_plan` Allow branch (before `may_execute_without_approval`; exhaustion → requires_approval `budget.exhausted`) + counters persisted | argus-ai | control.rs |
| T005 | SentinelView + sentinel report additive fields (phase/progress, earned, ceiling, effective, budgets) | argus-ai | sentinel.rs, cloud.rs payload |
| T006 | Cloud: loose validation of new sentinel fields; API passthrough | app-platform | core validateSentinel, api |
| T007 | Web: autonomy line on the instance page (effective of ceiling, phase progress, budget bars) | app-platform | web instance page/sentinel panel |
| T008 | Verify both repos; deploy cloud FIRST then daemon; live AC checks | both | — |

Key decisions:

1. **Effective, not replaced.** `brain.autonomy` stays the operator's ceiling
   and keeps its live-apply semantics; the daemon earns up to it. Every
   consumer of autonomy reads the effective value — one computation point in
   the daemon.
2. **The brain tick is the state machine's clock.** Gates are cheap local
   checks per cycle; no timers, no network. A demotion is evaluated at the
   same tick plus at the failure sites (validate/rollback, SafeMode entry,
   provider degradation).
3. **Budget is a pause, not a denial** — exhaustion routes the step to the
   existing approval machinery (`budget.exhausted` policy id), so the operator
   sees what wanted to run and why it waited.
4. **State is local-first**: one persisted `AutonomyState` row; the cloud only
   ever reads it through the sentinel report.
5. **Ordering on deploy: cloud first, then daemon** (007 lesson) — though this
   spec adds no wire kinds, the additive sentinel fields land before the
   daemon starts emitting them.

## Verification

**Commands:**
- argus-ai: `cargo test --locked --workspace --all-targets` (baseline 1055), `cargo clippy --workspace --all-targets`, `cargo fmt --check`.
- app-platform: `pnpm build` (8/8) + `pnpm test` (12/12).

**Manual checks (post-deploy):**
- Instance page shows the autonomy line (phase shadow N/10) after the new
  daemon is live; raising the cloud ceiling to L2 and waiting out the shadow
  window flips phase → earned with effective L2 in the sentinel report; a
  forced failed-validation (test capability) demotes one rung with a trace.
