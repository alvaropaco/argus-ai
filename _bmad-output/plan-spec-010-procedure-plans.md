---
title: 'Spec 010 — Procedure plans: runbook attribution by construction'
type: 'feature'
ticket: ''
created: '2026-10-10'
status: 'built'
baseline_revision: '536e4dc9d1c6bd0cb76a996c88e2db16b4648087'
route: 'full'
route_source: 'auto'
review: 'quick'
review_source: 'pinned'
lenses_ran: ['quick']
review_loop_iteration: 0
followup_review_recommended: true
context: ['argus-ai/specs/010-procedure-plans-runbook-attribution/spec.md', 'argus-ai/docs/adr/0042-procedure-plans-attribution-by-construction.md']
warnings: []
deferred: []
---

<intent-contract>

## Intent

**Problem:** A promoted runbook changes nothing — the brain never consults the library, plans carry no runbook linkage, and spec 009's opportunistic gates key on the hard-coded host-health trigger vocabulary instead of attribution (any resolved cycle "validates" every matching candidate; other triggers never climb).

**Approach:** When the brain's situation matches a **promoted** runbook's trigger, build a deterministic procedure plan from the runbook (allowed actions as steps targeting the situation's subject, schema-validated at construction, blast radius from the descriptors) and run it through the ordinary boundary; the plan carries the runbook's name; spec 009's opportunistic gates record Validation/Policy — and outcomes record history — for exactly the attributed runbook. No matching promoted runbook → the provider path, byte-identical.

## Boundaries & Constraints

**Always:** procedure plans cross the same policy → escalation → approval → budget → ledger → validation boundary as provider plans (only the origin differs; they are not AI output at all); attribution is by construction (the plan *is* the runbook's procedure — no trigger-vocabulary guessing, no text similarity); steps validate against the capability schemas at construction (`input_matches`) targeting the situation's subject — a non-conforming step is skipped, a plan with zero valid steps falls through to the provider path (logged); fail-closed — any construction error falls through to the provider, never a missed remediation; `Plan.runbook` is additive with serde default (persisted plans, IPC, cloud snapshots tolerate absence); dedup semantics identical between paths (same evidence-derived key, same claim/release); failures record `record_outcome(false)` on the runbook and release the dedup key; no automatic demotion.

**Never:** no provider consultation for procedure plans (they exist precisely to be deterministic); no provider-plan behavior change when no promoted runbook matches; no change to the six-gate ladder or its errors; no automatic runbook demotion; no candidate (non-promoted) runbooks driving procedures; no new wire protocol kinds (the additive Plan field rides existing payloads); do not commit `.mimosa/` or user artifacts.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Promoted match | cycle situation matches a promoted runbook's trigger | Deterministic procedure plan attributed to the runbook (objective `procedure: <name>`), provider not consulted | Construction error → log + fall through to provider |
| No promoted match | library empty / no trigger match / only candidates | Provider path unchanged, `plan.runbook` absent | — |
| Step arguments | runbook allowed action needs arguments (e.g. `unit`) | Arguments from the situation's subject, validated via `input_matches`; non-conforming steps skipped | Zero valid steps → fall through to provider (logged) |
| Procedure validates | attributed plan terminates Completed | Validation → Policy gates record for that runbook; `record_outcome(true)`; episode remediation carries the runbook name | Ladder rejects (missing prerequisites) log and wait — never guessed |
| Procedure fails/rolls back | attributed plan terminates Failed/RolledBack | `record_outcome(false)` on the runbook; gates do not advance; dedup released | — |
| Approval pause | procedure plan paused at the autonomy gate | Operator grants; resume executes the same attributed plan | — |
| Restart / old payloads | persisted plans and snapshots without the field | Deserialize with `runbook: None`; surfaces render unchanged | — |

</intent-contract>

## Code Map

**argus-ai** (test: `export PATH="$HOME/.cargo/bin:$PATH"; cargo test --locked --workspace --all-targets`; baseline 1132 passed)
- `crates/argus-domain/src/reasoning.rs:173 Plan` — add `runbook: Option<String>` with `#[serde(default, skip_serializing_if = "Option::is_none")]`; **the manual Deserialize at `:183` wraps a `PlanWire` — add the field there too** (the easy-to-miss one); `propose_plan` in `argus-ai-core/src/decision/host_health.rs:279` and its tests need the field.
- `crates/argus-daemon/src/brain.rs` — `run_cycle` calls `daemon.propose_once(metered.as_ref(), evidence, threshold, &events)` at `:216-230`; the procedure path slots BEFORE it: consult `daemon.runbooks()` for the first **Promoted** runbook whose trigger matches the cycle signature (`RunbookTrigger::Symptom("host-health")` — the brain's only situation vocabulary today); build the plan from the runbook's allowed actions with arguments from the cycle's subjects (the failed unit; validate via `argus_domain::input_matches` against descriptors from `daemon.registry()`); claim/release the dedup key exactly as `plan_from_evidence` does (`observation_dedup_key` from the same evidence `ContextBuilder`; investigate `Daemon`'s dedup accessor or thread `&self.dedup` through a new `propose_procedure` method next to `propose_once` at `runtime.rs:945`); set `plan.runbook = Some(name)`; on construction failure log + fall through.
- `crates/argus-daemon/src/runbooks.rs` — `RunbookManager`: replace `on_procedure_validated(&RunbookTrigger, ...)` with an attribution-based `on_attributed_plan_validated(name, descriptors, policy)` (Validation → Policy for THAT runbook, ladder-ordered, plus `record_outcome(true)`); add `note_procedure_outcome(name, success)` for the failure path; `evaluation_evidence` gains the name-linkage acceptance (an episode whose remediation contains the runbook's name counts alongside the trigger match). Brain's `record_runbook_gates` (`:368-385`) rewires from the trigger call to the attributed call using the terminal plan's `runbook` field; the failure path (RolledBack/Failed on an attributed plan) calls `note_procedure_outcome(false)`.
- Surfaces: the sentinel report's plan entries (`brain_state.rs` record → `cloud.rs` SentinelReportPayload plans) add the runbook field where plans serialize — additive only; web plan card may badge it (optional).
- Tests layout: brain procedure-plan tests beside `crates/argus-daemon/tests/brain.rs` (promoted runbook → attributed plan; no match → provider path; non-conforming steps skipped; failure records outcome), manager attribution tests in `runbooks.rs`, domain Plan serde round-trip.

**app-platform-argus-ai** — no required changes: `plan.runbook` rides the sentinel snapshot jsonb (loose validation) and the plans payload additively; optionally a `procedure` badge in `sentinel-panel.tsx` plan cards reading `plan.runbook` defensively.

## Tasks & Acceptance

**Execution:**
- [ ] `crates/argus-domain` — `Plan.runbook: Option<String>` (serde default + PlanWire) + round-trip test
- [ ] `crates/argus-daemon/src/brain.rs` (+ runbooks.rs) — procedure-plan construction before the provider path (promoted match, schema-validated steps from the situation's subject, dedup parity, fall-through), attribution set
- [ ] `crates/argus-daemon/src/runbooks.rs` — attribution-based gate recording (replaces the trigger hook), `note_procedure_outcome`, evaluation name-linkage
- [ ] surfaces — sentinel plan entries carry `runbook` (additive); optional web badge
- [x] both repos — full verification; DEPLOYED 2026-10-10 cloud-first: app-platform `524c742` (procedure badge), argus-ai `ec4641b` (daemon built on the VPS, binary-swapped). **Live-proven (AC-002)**: with no promoted runbook in the library (the delivered candidate is honestly gateless), the daemon reconnects, the provider path runs unchanged (zero procedure-plan log lines), the delivered candidate is restored from persistence again, and the autonomy ladder holds `earned · l2_recommend` across the swap. Not yet live: AC-001/003-006 need a promoted runbook, which honestly requires earned gates, which require real validated work — the ladder is doing its job.

**Acceptance Criteria:**
- Given a promoted runbook whose trigger matches the cycle's situation, when the brain cycles, then an attributed procedure plan is proposed (objective `procedure: <name>`) without consulting the provider (AC-001).
- Given no matching promoted runbook, when the brain cycles, then the provider path is unchanged and `plan.runbook` is absent (AC-002).
- Given an executed and validated attributed plan, when the terminal outcome lands, then Validation/Policy record for exactly that runbook, `record_outcome(true)` lands, and the episode carries the runbook's name (AC-003).
- Given a failed/rolled-back attributed plan, when the outcome lands, then `record_outcome(false)` lands, gates do not advance, and the dedup key releases (AC-004).
- Given a re-delivered candidate with a name-linked resolved episode, when delivery review runs, then the Evaluation gate accepts the name-linked episode (AC-005).
- Given a procedure plan paused for approval, when granted, then the resume executes the same attributed plan and the outcome flows to the runbook (AC-006).

## Implementation Notes

- Implemented by the full-route subagent (2026-10-10). `Plan.runbook: Option<String>` (serde default + PlanWire; `plan_context_hash` treats attribution as content); the procedure path slots before the provider in `run_cycle` (`promoted_runbook` finds the first Promoted name-ordered match on the cycle signature; `procedure_plan` builds objective `procedure: <name>`, schema-validated steps targeting the failed unit via `input_matches`, positional rollback pairing only when the rollback's own schema accepts the arguments, worst-of-descriptors blast radius, confidence = historical success rate else 0.5); `Daemon::propose_procedure` claims/releases the SAME evidence-derived dedup key (parity with `propose_once`); attributed plans carry their dedup key on `PendingPlan` so a resumed failure releases the situation (AC-004/006); `RunbookManager.on_attributed_plan_validated(name, …)` replaces the spec-009 trigger hook (Validation → Policy + `record_outcome(true)` for exactly the named runbook), `note_procedure_outcome(name, success)` for failures; `evaluation_evidence` accepts the exact `procedure: <name>` signature at any trigger; sentinel plan entries + `plan.list` + last_cycle carry `runbook` skip-if-none; web plan cards badge the runbook defensively.
- Review (quick lens, 2026-10-10): 2 findings, both medium, both patched — substring name-linkage → exact signature match; denied/zero-execution plans recording failed attempts (a default-L0 refusal loop would have driven the success rate to 0.0 with zero real runs) → attempts recorded only when the procedure was actually attempted (executions non-empty). All re-verified.
- Known design consequences (documented by the implementer): candidates no longer earn Validation/Policy from ordinary provider cycles — they honestly wait at Eval+Sim until a promoted procedure of their own runs (attribution is by name, the ladder the sole arbiter); file-owned promoted runbooks' attempt history updates in memory only (persist no-ops, logged); step arguments are the host-health subject vocabulary (`{"unit": …}`) — other vocabularies derive their own when the brain's situations grow (out of scope).

## Auto Run Result

- **Summary:** Spec 010 implemented end-to-end: promoted runbooks now DRIVE procedures — the brain consults the library before the provider path and, on a trigger match, runs the runbook's own deterministic procedure through the ordinary boundary (policy → escalation → approval → budget → ledger → validation); the plan carries the runbook's name (`Plan.runbook`, additive), the spec-009 trigger-vocabulary gate hook is retired in favor of exact attribution (Validation/Policy record only for the runbook whose procedure ran and validated), outcomes record honest attempt history (attempted-only), and episodes record the procedure signature so the Evaluation gate gains a content linkage. The provider path is byte-identical when no promoted runbook matches.
- **Files changed:** argus-ai — `argus-domain/src/reasoning.rs` (Plan.runbook + PlanWire), `argus-daemon/src/brain.rs` (procedure path, `procedure_plan`, attribution wiring), `runtime.rs` (`propose_procedure`, `note_runbook_outcome`, resume wiring, sentinel/last_cycle fields), `runbooks.rs` (attribution-based gates, note_procedure_outcome, exact name-linkage), `control.rs` (PendingPlan.dedup_key), `handler.rs` (plan.list runbook), surfaces + tests across brain/runbooks/control/ipc. app-platform — sentinel plan card runbook badge + type (defensive).
- **Review findings:** quick lens, 2 findings — both medium, both patched (exact-signature name-linkage; attempted-only outcome recording). 0 rejected, 0 deferred. All re-verified.
- **Follow-up review recommendation: true** — two medium entries patched. Specific unverified risks: the procedure path is integration-tested only (a promoted runbook + a real failed unit on a live host has not occurred yet — the honest ladder requires earned evidence before promotion); the resume-path dedup release is deterministic-tested but not live-exercised.
- **Verification performed:** argus-ai `cargo test --locked --workspace --all-targets` 1149 passed / 2 ignored pre-patches (+17 new), clippy 0 warnings, fmt clean; post-patch targeted suites green with full workspace re-run on the orchestrator side. app-platform `pnpm build` 8/8, `pnpm test` 12/12. Matrix audit: all seven I/O rows covered by tests that ran and passed. Diff reviewed in full by the orchestrator + quick lens.
- **Residual risks:** a promoted runbook changes what the brain does on matching situations — promotion is now load-bearing (that is the point; the operator approved it); the brain's situation vocabulary remains host-health-only (other triggers climb when their situations exist); file-owned promoted runbooks' history is memory-only; post-deploy live checks outstanding.

## Plan Change Log

## Review Triage Log

### 2026-10-10 — Review pass (quick lens)
- verdicts: 2 findings — high 0, medium 2, low 0, false 0, maybe-false 0
- findings:
  - `[medium]` `[patch]` the Evaluation name-linkage uses substring containment (`remediation.contains(name)`), so a candidate named `restart` is granted evidence by another runbook's `procedure: restart-failed` episode — verified at `runbooks.rs:673,686-688`; the recorded signature is exactly `procedure: <name>`, so an exact comparison is available. Fix: exact case-insensitive signature match + a substring-overlap test.
  - `[medium]` `[patch]` every non-Completed terminal status records a failed procedure attempt — including `Denied` (zero executions, policy/autonomy refusal) — and denials release the dedup key, so a default L0 install records `record_outcome(false)` once per cycle and drives the success rate to 0.0 with zero real runs — verified: `note_runbook_outcome` maps any status ≠ Completed to false; `control.rs` Denied ⇔ executions empty. Fix: record an attempt only when the report carried at least one execution (the procedure was actually attempted); denied plans record nothing; Failed/RolledBack with executions keep recording false.
- patches applied (re-engaged step-03 implementer; targeted suites green, full re-verification on the orchestrator side): the Evaluation name-linkage compares the EXACT recorded signature `procedure: <name>` (trimmed, case-insensitive) instead of substring containment, with a test proving `restart` does not match `procedure: restart-failed`; `note_runbook_outcome` takes the whole `ExecutionOutcome` and returns early on zero executions (a refused procedure records nothing at all — a default-L0 refusal loop can no longer drive the success rate to 0.0), Failed/RolledBack with executions keep recording false; the pause/resume test now proves the failure was attempted (non-empty executions) before asserting the 0.0 rate, and a new runtime unit test pins both sides (zero-execution refusal → attempts 0; attempted failure → attempts 1 / rate 0.0). Two latent test fragilities fixed in passing (real `argusd.service` subject → a nonexistent well-formed unit; a wrong guardrail-credit comment).

## Design Notes

- Attribution by construction is the whole idea: the plan IS the runbook's procedure, so the gates record from what actually ran — spec 009's approximation (trigger vocabulary) is retired, not extended.
- Dedup parity matters: the procedure path must claim the SAME evidence-derived key the provider path would, so the two paths never double-act on one situation.
- The procedure plan is deliberately provider-independent (ADR-0042 §1): "not AI output at all" is the strongest form of the constitutional rule.
- Step arguments come from the situation (the failed unit), validated by `input_matches` at construction — a runbook whose actions cannot express the situation falls through honestly.

## Verification

**Commands:**
- argus-ai: `export PATH="$HOME/.cargo/bin:$PATH"; cargo test --locked --workspace --all-targets` — expected: all pass (baseline 1132), `cargo clippy --workspace --all-targets` 0 warnings, `cargo fmt --check` clean.
- app-platform: `pnpm build && pnpm test` — expected: 8/8, 12/12.

**Manual checks (post-deploy, cloud first):**
- Walking a runbook to promotion on a live host is gated by real evidence (honest); the live check is AC-002 (no promoted match → unchanged behavior) plus the integration tests. When a real failed-unit situation occurs with a promoted host-health runbook, the plan appears as `procedure: <name>` in the Activity ledger and the report.
