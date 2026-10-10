---
title: 'Spec 011 — Situation vocabulary: the brain perceives more than failed units'
type: 'feature'
ticket: ''
created: '2026-10-10'
status: 'in-review'
baseline_revision: 'd74416d7b8831f70f4c9e7401ff950159f9cd443'
route: 'full'
route_source: 'auto'
review: 'quick'
review_source: 'pinned'
lenses_ran: ['quick']
review_loop_iteration: 0
followup_review_recommended: false
context: ['argus-ai/specs/011-situation-vocabulary/spec.md', 'argus-ai/docs/adr/0043-situation-is-the-unit-of-perception.md']
warnings: []
deferred: []
---

<intent-contract>

## Intent

**Problem:** The brain perceives exactly one situation type — failed systemd units — so runbooks authored for any other trigger can neither earn gates nor drive procedures, and the provider reasons from starved evidence while the observation loop already reads memory/load/CPU live every tick.

**Approach:** Each brain cycle collects a typed situation set — failed units (`host-health`, unchanged) plus threshold crossings from live kernel readings (`memory-pressure`, `disk-pressure`; thresholds in a new `[situations]` config) — and reasons per situation: evidence, dedup, procedure plans (promoted runbooks match per signature), episodes/procedures under the situation's signature; the provider receives all situations' evidence in one request when no procedure plan fires; situations surface in the trace and feed the sentinel's pressure section additively.

## Boundaries & Constraints

**Always:** readings are kernel-native (existing `/proc/meminfo` reader; a new statvfs disk-usage reader, no new heavyweight deps, no command execution); situations are present while the reading crosses the threshold and absent otherwise (no hysteresis, no latching); per-situation dedup keys `(signature, subject)` with the same claim/release semantics; failed units keep the exact `host-health` behavior; episode/procedure symptoms become the situation's signature (failed units keep `host-health`); fail-closed — a failed reading contributes no situation (logged), failed-unit collection never degraded by pressure reads; the default configuration with readings below thresholds behaves byte-identically to today; gate/episode honesty is spec 009/010's (evidence only, ladder order); every new user-facing string/log names the reading that triggered the situation.

**Never:** no baseline/deviation situations in this spec (the BaselineManager keeps its existing events); no hysteresis or latching; no k8s/container situations; no new action capabilities; no fabricated situations on read failure; no change to the promotion ladder, procedure-plan construction rules, or the autonomy machinery beyond feeding it situations; do not commit `.mimosa/` or user artifacts.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Memory crossing | `memory.used_percent ≥ memory_used_percent` | A `memory-pressure` situation (subject `mem`, reading in evidence); provider evidence includes it; a promoted `memory-pressure` runbook yields an attributed procedure plan | — |
| Disk crossing | disk usage ≥ threshold | A `disk-pressure` situation (subject the mount) | statvfs failure → no situation, logged |
| Below thresholds | default config, healthy readings | No pressure situations; behavior byte-identical to today | — |
| Failed unit + pressure | both in the same cycle | Both situations collected; reasoned in signature order; separate dedup keys; one provider request if no procedure plan | — |
| Reading source fails | meminfo/statvfs error | That situation absent (logged); failed-unit collection unaffected | Never fabricated |
| Pressure resolves | reading drops below threshold | Situation absent; dedup released; outcome recorded on the attributed runbook/episode as today | — |
| Promoted runbook per signature | `memory-pressure` runbook promoted | Its procedure plan fires on the crossing (arguments derive per signature; schema-invalid steps skipped per spec 010) | Zero valid steps → provider evidence path |

</intent-contract>

## Code Map

**argus-ai** (test: `export PATH="$HOME/.cargo/bin:$PATH"; cargo test --locked --workspace --all-targets`; baseline 1150 passed)
- `crates/argus-sensors/src/proc/` (meminfo.rs `read()` + `used_percent()`, loadavg, stat, vmstat, diskstats) — add `crates/argus-sensors/src/sys/disk.rs` (or proc/): `usage(mount) -> DiskUsage{mount, used_percent, total, free}` via statvfs through `libc` (add `libc` to argus-sensors deps — ubiquitous, no new tree); `/` the default mount. Unit test on the type + percent math (mock-free; statvfs on `/` in CI is fine).
- `crates/argus-daemon/src/brain.rs`:
  - `failed_units()` (`:431-455`) — keep as-is; returns `(subjects, consulted_live)`.
  - NEW `collect_situations(config) -> Vec<Situation>` beside it: failed units → `Situation{signature: "host-health", subject: unit, ...}` per unit (capped at 1 like today), plus memory (`argus_sensors::meminfo::read()` → used_percent ≥ threshold) and disk (`usage("/")` ≥ threshold) situations carrying their readings. A `Situation {signature: String, subject: String, detail: String}` struct in this module.
  - `run_cycle` (`:203-350`) — restructure: collect situations once; iterate them in collection order (failed units first, then memory, then disk — deterministic); per situation: evidence ContextBuilder (failed-unit evidence exactly as today; pressure evidence lines `"{signature}: {subject} at {reading}%"`), dedup key `format!("{signature}:{subject}")`-derived (same `observation_dedup_key` mechanism — investigate; if the key is purely evidence-derived it already differs per situation), procedure-plan consult (`promoted_runbook` matching the SITUATION's signature — generalize `promoted_runbook`/`procedure_plan` from spec 010 to take the signature and derive arguments per signature: `host-health` → `{"unit": …}`, pressure signatures → the situation's subject as the only argument candidate, `input_matches` decides), then `run_remediation` per situation's plan; provider path: ONE `propose_once` with all situations' evidence merged when no procedure plan fired (the existing flow, evidence superset).
  - `remember()` — symptom becomes the situation's signature (failed units keep `host-health`); the remediation signature `procedure: <name>` unchanged (spec 010's name-linkage relies on it).
  - The spawn-loop autonomy signals (`:620-640`): unchanged except `live_environment` — now true when ANY situation collection read succeeded (it already is: failed_units + pressure reads).
- `crates/argus-daemon/src/config.rs` — `[situations]` section (`SituationsConfig`: `memory_used_percent` default 90 clamp 50–99, `disk_used_percent` default 90 clamp 50–99) — clone the `AutonomyConfig` pattern; test absent-section = defaults.
- `crates/argus-daemon/src/sentinel.rs` + `runtime.rs sentinel_snapshot` — the pressure section may be fed additively from the collector's crossings (skip-if-none preserved); the cycle trace evidence already carries the readings via `BrainCycle.evidence`.
- Tests: `crates/argus-daemon/tests/brain.rs` (situations with a fake readings source — the collector should take readings as INPUT, pure and testable: `fn situations_from(readings: &Readings, config) -> Vec<Situation>`), runbooks attribution tests unchanged (signature-generic already), config clamp test.

**app-platform-argus-ai** — no required changes: the trace evidence and sentinel pressure ride existing additive surfaces; the web renders evidence lines already.

## Tasks & Acceptance

**Execution:**
- [ ] `crates/argus-sensors` — statvfs disk-usage reader + percent math + test
- [ ] `crates/argus-daemon/src/config.rs` — `[situations]` thresholds (clamped, absent = defaults)
- [ ] `crates/argus-daemon/src/brain.rs` — `Situation` + pure `situations_from(readings, config)` collector (failed units + memory + disk, deterministic order) + per-situation reasoning in `run_cycle` (procedure consult per signature, provider evidence merge, per-situation dedup, per-signature episodes/procedures) + tests
- [ ] `crates/argus-daemon/src/sentinel.rs` (+ runtime.rs) — pressure fed additively from collector crossings (skip-if-none preserved)
- [x] both repos — full verification; DEPLOYED 2026-10-10: argus-ai `79f027e` only (app-platform untouched — the surfaces ride existing jsonb); daemon built on the VPS, binary-swapped, connected. **Live-proven**: with `[situations] memory_used_percent = 50` (host at 60.7%), the trace shows `memory-pressure situation collected: mem at 60.7%` and the cycle reasons with evidence `memory-pressure: mem at 60.7%` (AC-001); config restored to defaults → **zero memory-pressure lines** (AC-002/006 live, both directions); the delivered-candidate restore and failed-unit path unaffected. A date-dependent flake in the 008 budget-roll test (failed daily at 23:xx UTC — `now+1h` crossed the UTC midnight boundary) was found during verification and fixed by anchoring mid-day UTC. Not yet live: AC-003 (a promoted memory-pressure runbook driving a procedure — needs a subject-capable capability, out of scope per §6).

**Acceptance Criteria:**
- Given `memory.used_percent` at/above the configured threshold, when the brain cycles, then a `memory-pressure` situation is collected with the reading in evidence and a promoted runbook authored for that signature yields an attributed procedure plan (AC-001).
- Given readings below thresholds, when the brain cycles, then no pressure situation exists and behavior is byte-identical to today (AC-002/006).
- Given a memory-pressure situation whose attributed procedure executes and validates, when the outcome lands, then Validation → Policy record on that runbook and the episode's symptom is `memory-pressure` (AC-003).
- Given a failed unit and a pressure crossing in the same cycle, when the brain cycles, then both situations are reasoned in deterministic order with separate dedup keys and one provider request if no procedure plan fires (AC-004).
- Given a failing reading source, when the collector runs, then that situation is absent (logged) and failed-unit collection proceeds (AC-005).

## Implementation Notes

## Plan Change Log

## Review Triage Log

### 2026-10-10 — Review pass (quick lens)
- verdicts: 4 findings — high 1, medium 2, low 0, false 1, maybe-false 0
- findings:
  - `[false]` `[reject]` "the diff omits disk.rs — the change does not compile as diffed" — true of the REVIEW ARTIFACT, not the tree: the new file was untracked and the staging step skipped it (`git add -N` missed); the reviewer confirmed the file exists, compiles, and is sound in the working tree. Staging fixed on the orchestrator side and the diff regenerated; no code change.
  - `[high]` `[patch]` the provider path's dedup key diverges from the procedure path's: `propose_procedure` claims the stable `(signature, subject)` key, but `propose_once` claims the evidence-derived canonical key — which now embeds the fluctuating pressure reading. Consequences (verified): (a) a mixed cycle with a promoted host-health runbook — the unit's procedure deduped, the provider re-plans the SAME unit under a fresh unstable key, repeating on every 0.1% reading jitter (double-acting on one situation); (b) a pressure-only crossing re-consults the metered provider afresh every cycle (`remediation_action` fails closed without a `unit` entry and releases). The spec's NFR "dedup prevents repeat reasoning on a stable crossing" held only on the procedure path. Fix: derive the provider request's dedup key from the STABLE situation identities (the joined situation dedup keys), not the raw evidence — stable across reading jitter, claim/release symmetric; host-health-only cycles keep a stable key (its form may change: the dedup is in-memory).
  - `[medium]` `[patch]` resolution releases a pressure dedup key while a plan for that situation is Pending — after a momentary below-threshold reading, a recrossing re-claims and proposes a second plan for the same situation while the first approval is open (host-health has no such path: its pending key stays claimed) — verified: `resolved_pressure_keys` releases unconditionally. Fix: skip the release while a pending plan carries that dedup key.
  - `[medium]` `[patch]` AC-001's acting half and AC-003 have no end-to-end test: both e2e pressure-runbook tests author `host.service.restart` (unit schema — can never accept `mem`/mount), exercising only the zero-valid-steps fall-through; nothing asserts an attributed ACTING pressure procedure plan, the `memory-pressure` episode symptom, or the gate flow on a pressure-signature runbook — verified. Fix: an e2e test with a subject-capable capability registered, asserting the attributed acting plan, the pressure episode symptom, and the Validation/Policy flow.

## Design Notes

- The collector takes READINGS as input and is pure — thresholds and situation shape testable without a kernel; the caller reads meminfo/statvfs.
- Signature order is collection order: failed units first (the established vocabulary), then memory, then disk — deterministic for episodes, traces, and provider evidence.
- The disk-pressure subject is the mount (`/` first) — capabilities that cannot express a mount subject are skipped at procedure construction per spec 010's schema validation.
- The situation's reading is the evidence: traces and approvals name the number that triggered everything (ADR-0043 §4).

## Verification

**Commands:**
- argus-ai: `export PATH="$HOME/.cargo/bin:$PATH"; cargo test --locked --workspace --all-targets` — expected: all pass (baseline 1150), `cargo clippy --workspace --all-targets` 0 warnings, `cargo fmt --check` clean.
- app-platform: `pnpm build && pnpm test` — expected: 8/8, 12/12.

**Manual checks (post-deploy, cloud first):**
- Lower `memory_used_percent` above the host's actual usage → the trace shows `memory-pressure: mem at N%` and a promoted memory-pressure runbook drives a procedure plan; restore the threshold → situations disappear; failed units keep `host-health` behavior throughout.
