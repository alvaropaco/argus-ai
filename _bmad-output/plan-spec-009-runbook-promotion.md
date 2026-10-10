---
title: 'Spec 009 — Runbook distribution & the promotion ladder'
type: 'feature'
ticket: ''
created: '2026-10-10'
status: 'built'
baseline_revision: 'b7203e0e0d3af67a082b26266ed8b0ec9f4dc81d'
route: 'full'
route_source: 'auto'
review: 'quick'
review_source: 'pinned'
lenses_ran: ['quick']
review_loop_iteration: 0
followup_review_recommended: true
context: ['argus-ai/specs/009-runbook-distribution-promotion-ladder/spec.md', 'argus-ai/docs/adr/0041-runbook-delivery-is-not-promotion.md']
warnings: []
deferred: []
---

<intent-contract>

## Intent

**Problem:** The promotion ladder (evaluation → simulation → validation → policy → approval → promotion) exists and is tested but has no production caller: runbooks load from a local directory as candidates and stay candidates forever; operators cannot deliver learned procedures to a fleet, see ladder progress, or approve/promote anything.

**Approach:** Deliver runbooks as a new managed-config `runbook` kind (TOML content → loader → library Candidate with cloud provenance, persisted across restart); record the technical gates honestly and in order (evaluation against recorded episodes + simulation via the risk engine at delivery review; validation opportunistically when the runbook's procedure runs and validates; policy after, in ladder order); surface approve/promote to the operator through a `runbook.decision` control message modeled on `approval.decision` plus CLI parity; report the runbook table through an additive sentinel field and a web runbook card.

## Boundaries & Constraints

**Always:** delivery is data — a delivered runbook authorizes nothing and its allowed_actions stay candidates-not-grants; gates are recorded only in ladder order and only from real evidence (never invented, never out of order, existing `GateError`/`PromotionError` semantics untouched); malformed delivery replies `config.result failed` with the loader reason and loads nothing; gate/persistence failures degrade downward (logged, never higher); approval and promotion are operator acts crossing into the local ladder; state persists locally and survives restart; the sentinel field is additive skip-if-none (old snapshots unchanged); the lean build without delivered runbooks behaves exactly as before.

**Never:** no automatic promotion or unattended learning; no cloud-side ladder state; no runbook authoring/deletion UI; no new execution authority at delivery time; no change to the six-gate order or the ladder's errors; no runbook sync channel beside the config channel; do not commit `.mimosa/` or user artifacts.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Valid delivery | `runbook` config, well-formed TOML | Candidate in library with cloud provenance, Evaluation+Simulation gates recorded if they pass, `config.result applied` | — |
| Malformed delivery | TOML parse/validation failure | Nothing loaded; `config.result failed` with the loader reason | Logged |
| Duplicate delivery | same runbook name re-delivered | New version supersedes the previous candidate | — |
| Evaluation/Simulation fail | episodes disagree or impact estimate has Infeasible rollback | Gates not recorded; candidate delivered gateless; reason logged | Never fabricated |
| Trigger fires, procedure validates | runbook-matching situation → plan → executed+validated | Validation gate, then Policy gate (in order) recorded; logged | Out-of-order attempts are rejected by the ladder (existing errors) |
| Operator decision | approve at ≥Policy / promote at Approved via control message or CLI | Status transitions through the local ladder; result reported; audited | Ladder errors (NotReadyForApproval/NotApproved/OutOfOrder) replied as the failure reason |
| Restart | daemon restarts | Delivered candidates + gate progress reload from persistence beside dir-loaded ones | Persist failure → logged, library falls back to dir-loaded only |
| Old snapshot/web | snapshot without `runbooks` field | Web renders unchanged (defensive reads) | — |

</intent-contract>

## Code Map

**argus-ai** (test: `export PATH="$HOME/.cargo/bin:$PATH"; cargo test --locked --workspace --all-targets`; baseline 1097 passed)
- `crates/argus-runbooks/src/loader.rs:24 RunbookFile` (deny_unknown_fields; has a `gates` field replayed through the real ladder at `:132` — files can never grant unearned status), `:85 load_runbooks(dir) -> LoadResult{library, skipped}`; `library.rs:13 RunbookLibrary` (`register` name-checked, `get/get_mut`, `list`, `matching(&trigger)`, `by_status`, `promotable`); `runbook.rs:88 Runbook::candidate` (9 args: id, name, trigger, required_evidence, investigation_steps, decision_criteria, allowed_actions, rollback, validation), `record_outcome`/`historical_success_rate`; `promotion.rs:13 Gate`, `record_gate/has_reached/approve/promote`.
- `crates/argus-risk/src/impact.rs:73 simulate_impact(&SimulationInput)` — per-action advisory (assemble inputs from capability-registry descriptors); RollbackFeasibility Feasible/Partial/Infeasible. Simulation-gate acceptance: every allowed_action simulated with rollback not Infeasible and risk not Destructive.
- Episodes: `argus-state repository.rs:104 list_episodes()`; `argus-memory/src/episodic.rs:25 Episode` (subject, symptom, resource_class, remediation, outcome Resolved/Recurred/Unresolved); `similarity.rs:45 similarity_score`. Evaluation gate: the runbook's declared steps/remediation matching recorded episodes with Resolved outcomes (deterministic match; a host with no episodes fails honestly).
- Config channel: `cloud.rs:918 apply_configuration` (parse → kind → disposition → validate-then-store-then-apply, brain pattern `:983-1046`), `reply_config_result :1090`; `config.rs:409 KIND_* consts` + `:446 disposition()` — add `KIND_RUNBOOK = "runbook"` (Apply disposition). Dispatch arm `cloud.rs:2063`.
- Daemon library: `runtime.rs:99 runbooks: RunbookLibrary` plain field, `:272 load_runbooks(dir)`, accessor `:865` read-only — the library must move behind `Arc<std::sync::RwLock<RunbookLibrary>>` (or an owned manager like 008's `AutonomyManager`) so delivery/gates/promotion can mutate; startup merges dir-loaded + persisted-delivered.
- Procedure site: `brain.rs:305-355 remember()` records `ProcedureRecord` per acting cycle — the opportunistic Validation/Policy gate hook lives beside it (only when the cycle's plan is attributable to the runbook: the brain's procedure name vs the runbook name — investigate the exact linkage; if none exists today, record Validation/Policy when the brain's remediation for a matching trigger validates, keyed by trigger, never invented).
- Persistence: no runbook storage exists; `sqlite.rs put_json/get_json` singleton pattern (`autonomy_state` at `:125`) — new `runbooks` table keyed by name storing the serialized runbook + provenance (config version). If `Runbook` lacks Serialize on the needed fields, persist the delivery (TOML string + gates + provenance) and re-parse on load.
- Commands/control: `cloud.rs:1219 handle_command_invoke` (host-capability ladder — NOT the route for runbooks); the model to copy is `approval.decision`/`approval.result` (control channel, `cloud.rs:1841 handle_approval_decision`): new `runbook.decision` (cloud → daemon: name + approve|promote + actor) and `runbook.result` replies, additive protocol minor bump in `crates/argus-cloud/src/protocol/{envelope,messages,version}.rs` (1.1.0 → 1.2.0).
- IPC/CLI: `argus-ipc/src/protocol.rs:19 Operation` (+ `RunbooksApprove`/`RunbooksPromote`; MINOR bump; `RunbooksList` exists at :36); CLI `argus-ai-cli/src/main.rs:55 Command::Runbooks` (list exists; add approve/promote modeled on `run_approval :193`); daemon `handler.rs:63` dispatch, `runbooks_list :459`.
- Sentinel: mirror the 008 pattern — `sentinel.rs SentinelView.runbooks: Option<Map>` (skip-if-none) via `SentinelInputs.runbooks_view`, built in `runtime.rs sentinel_snapshot` from the library (name, status, gates, provenance).

**app-platform-argus-ai** (pnpm+turbo; `pnpm build` 8/8, `pnpm test` 12/12)
- API: the sentinel snapshot's raw jsonb surfaces `view.runbooks` with ZERO api change; the decision write is a new `POST /v1/tenants/:tenant_id/instances/:id/runbook-decision` modeled on `registerApprovalRoutes` (`apps/api/src/routes/commands.ts:140`): validate body (name + decision enum + actor) → `recordAudit` (new AuditAction `RunbookDecided` in `packages/contracts/src/api/audit.ts`) → `sendInstanceControl(pool, {type: "runbook.decision", ...})` → 202; UUID guards per the house pattern.
- Gateway: forward `runbook.decision` control to the daemon + handle the `runbook.result` reply if emitted (`apps/gateway/src/push/control.ts` consumer + `connection/handshake.ts` — copy the approval.decision path).
- Web: runbook card as a section in `apps/web/components/instance/sentinel-panel.tsx` (defensive `view.runbooks` read, like `view.autonomy`) with Approve/Promote buttons for decision-ready candidates posting the new endpoint; types in `lib/types.ts`; primitives `ui/primitives.tsx`.

## Tasks & Acceptance

**Execution:**
- [ ] argus-ai: gate evidence helpers — episode-based Evaluation check (deterministic, fail-closed on empty episodes) and risk-impact Simulation check (per allowed_action; Infeasible rollback or Destructive risk fails) with unit tests
- [ ] argus-ai: `KIND_RUNBOOK` delivery — disposition, apply_configuration branch (parse TOML → loader → library Candidate + provenance → Evaluation/Simulation gates if they pass → persist → config result), supersede-on-redelivery, malformed-fails-closed
- [ ] argus-ai: runbook persistence (sqlite/memory) + library behind a lock + startup merge (dir + delivered)
- [ ] argus-ai: opportunistic Validation/Policy gates at the procedure outcome site (in-order, evidence-backed, never invented)
- [ ] argus-ai: `runbook.decision`/`runbook.result` control messages + daemon handler through the local ladder; IPC `RunbooksApprove`/`RunbooksPromote` + CLI approve/promote + list with gates; protocol minor bump
- [ ] argus-ai: sentinel additive `runbooks` field (008 pattern)
- [ ] app-platform: audit action + POST runbook-decision route + gateway control forwarding
- [ ] app-platform web: runbook card with gate progress + approve/promote (defensive reads)
- [x] both repos — full verification; DEPLOYED 2026-10-10 cloud-first: app-platform `4641d8c` + protocol fix (contracts 1.2.0), argus-ai `41682dd` + delivery-refusal logging (daemon built on the VPS, binary-swapped). **Deploy-live fix**: the TS side of the protocol bump was missing — the plan bumped the Rust protocol to 1.2.0 but `packages/contracts` stayed at 1.1.0, so the 1.2.0 daemon negotiated no overlap and sat `connecting` forever (the 007 version-bump lesson, both sides this time); fixed in `packages/contracts/src/agent/errors.ts` and deployed. **Live-proven**: a real `config.apply` (runbook kind) delivered via the control channel → candidate `live-check-procedure` landed with provenance v1, gateless (honest: no recorded episodes yet at L0), persisted and **restored across a daemon restart** (AC-006 live); a same-version re-delivery was refused by the held-version guard; `argus runbook list` shows the candidate with provenance; the sentinel snapshot carries `view.runbooks {count: 1, items[0].name: live-check-procedure}` (AC-005 live). Bonus live proof from 008: the autonomy ladder climbed **shadow → earned L2** on the production host (ceiling L4, effective L2, budgets visible — 008's AC-001/002 now live-proven). Not yet live: approve/promote round-trip (no candidate has reached the policy gate — honest, awaiting validated work).

**Acceptance Criteria:**
- Given a valid runbook TOML deployed as a `runbook` configuration, when it applies, then the candidate lands in the library with cloud provenance, Evaluation+Simulation recorded if they pass, and `config.result applied` (AC-001).
- Given malformed TOML, when the config applies, then nothing loads and `config.result failed` carries the loader reason (AC-001).
- Given a candidate whose evaluation or simulation fails, when delivery review runs, then the candidate is gateless with the reason logged and nothing fabricated (AC-002).
- Given the runbook's trigger fires and its procedure executes and validates, when the outcome lands, then Validation then Policy gates are recorded in order (AC-003).
- Given a candidate at/after the policy gate, when the operator approves then promotes from the dashboard, then the runbook reaches `Promoted` through the local ladder and the result is audited; out-of-order decisions are refused with the ladder's errors (AC-004).
- Given the instance page open, when a sentinel report arrives, then the runbook card shows status/gates/provenance and old snapshots render unchanged (AC-005).
- Given a daemon restart, when the library loads, then delivered candidates and gate progress are intact beside dir-loaded runbooks (AC-006).

## Implementation Notes

- Implemented by the full-route subagent (2026-10-10). argus-ai: `argus-runbooks` gained `delivery.rs` (`DeliveredRunbook`: candidate + provenance, serde round-trip), `parse_runbook` (the file path extracted for deliveries), `by_name/by_name_mut/remove` on the library; `argus-state` gained `put_runbook`/`list_runbooks` (SQLite `runbooks` table + in-memory, keyed by name, re-put supersedes); new `argus-daemon/src/runbooks.rs` `RunbookManager` (library RwLock + delivered map + repository; deliver → parse → Evaluation/Simulation evidence recorded in order or withheld with logged reasons → persist-before-register → supersede; decide through the local ladder with audits for successes AND refusals; opportunistic Validation/Policy on completed runs; view skip-if-none); `KIND_RUNBOOK` config kind (Apply) with its own delivery branch in `apply_configuration`; `runbook.decision`/`runbook.result` control messages (protocol 1.2.0); IPC `runbooks.approve/promote` (IPC 0.8.0) + CLI `argus runbook list|approve|promote`; sentinel additive `runbooks` field; brain hook records Validation/Policy for delivered candidates on Completed runs keyed by the host-health trigger vocabulary.
- Review (quick lens, 2026-10-10): 9 findings — 8 patched, 1 rejected (see triage log). The HIGH: a delivered TOML could declare all six gates and the loader's replay would land it `Promoted` — automatic promotion by delivery, the ADR-0041-rejected path; fixed by refusing deliveries that declare gates. Also patched: file-owned runbooks decidable-then-reverting, missing held-version monotonicity guard, trigger-only gate attribution tightened (declared validation criteria required + live attempt history), mutate-vs-persist race, status-spelling mismatch. Rejected-low documented: the Evaluation gate is trigger corroboration (resolved episode with remediation), not per-step episode linkage — the memory layer holds no such linkage; strengthening is spec-010 material.
- Review fixes: all applied and re-verified; the declared-gates test delivers a TOML with all six gates and asserts wholesale refusal.

## Auto Run Result

- **Summary:** Spec 009 implemented end-to-end: runbooks travel the managed-config channel as a `runbook` kind (TOML → loader → library Candidate with cloud provenance, persisted, reloaded beside dir-loaded runbooks); the promotion ladder has its production caller — Evaluation/Simulation recorded at delivery review from local evidence (episodes, risk-engine simulation), Validation/Policy opportunistically from completed runs (declared validation criteria required), approve/promote as operator decisions via the `runbook.decision` control message, IPC 0.8.0 ops and CLI parity, all audited (`runbook.decided`, refusals included); the sentinel report carries the additive runbooks table and the web instance page renders the ladder card with decision buttons for decision-ready candidates.
- **Files changed:** argus-ai — runbooks crate (delivery.rs new, loader parse_runbook, library by_name/remove), argus-state (runbooks table + put/list), new argus-daemon/src/runbooks.rs (RunbookManager), cloud.rs (delivery branch + runbook.decision/result), config.rs (KIND_RUNBOOK), brain.rs (opportunistic hook), handler.rs + ipc + cli (ladder ops), sentinel.rs (additive field), protocol 1.2.0. app-platform — contracts (message types + payloads + audit action), api (POST runbook-decision + control type), gateway (control forwarding + runbook.result realtime), web (runbook card + ladder chips + decision buttons).
- **Review findings:** quick lens, 9 findings — 8 patched (1 high: declared-gates self-promotion by delivery; 3 medium: file-owned decisions reverting at restart, missing held-version guard, trigger-only attribution tightened; 4 low: race, spelling, dead history, folded test), 1 rejected-low with reasoning (Evaluation gate is trigger corroboration — the honest subset; per-step episode linkage is spec-010 material). All re-verified.
- **Follow-up review recommendation: true** — the high and three medium entries were patched. Specific unverified risks: the delivery→decision round-trip is unit/integration-tested only; the live chain (config deploy → candidate on the dashboard → approve/promote → Promoted) is unverified until deploy.
- **Verification performed:** argus-ai `cargo test --locked --workspace --all-targets` 1129 passed / 2 ignored pre-patches (+32 new), clippy 0 warnings, fmt clean; post-patch targeted suites green with full workspace re-run on the orchestrator side. app-platform `pnpm build` 8/8, `pnpm test` 12/12. Matrix audit: all eight I/O rows covered by tests that ran and passed. Diff reviewed in full by the orchestrator + quick lens.
- **Residual risks:** the opportunistic Validation gate is trigger-vocabulary-attributed (host-health) and requires declared validation criteria — a delivered runbook with any other trigger waits at Simulation by design until plan→runbook attribution exists (spec-010); delivered content survives `cloud.forget` (deletion is out of scope); corrupt persistence rows are skipped with a log (no integrity check beyond serde); post-deploy manual checks outstanding.

## Plan Change Log

## Review Triage Log

### 2026-10-10 — Review pass (quick lens)
- verdicts: 9 findings — high 1, medium 3, low 5, false 0, maybe-false 0
- findings:
  - `[high]` `[patch]` a delivered TOML can declare `gates = [...all six]` and the loader's replay grants them — including `approve()`/`promote()` arms — landing a `Promoted` runbook by delivery: automatic promotion, the exact alternative ADR-0041 rejected; the fabricated policy gate lights the dashboard's Approve button and the state persists — verified: `deliver` calls `parse_runbook` and never strips/rejects declared gates; `record_gate` enforces order, not evidence. Fix: a delivered candidate must land with exactly the gates the local evidence review records — reject deliveries that declare gates ("a delivered runbook cannot declare gates") and test it.
  - `[medium]` `[patch]` `decide` accepts approve/promote for directory-loaded runbooks: the decision succeeds, is audited, is reported — and silently reverts at restart because `persist_delivered` is a no-op for non-delivered names — verified: `by_name_mut` never consults the delivered map. Fix: refuse non-delivered names with "only delivered runbooks participate in the ladder".
  - `[medium]` `[patch]` runbook deliveries skip the held-version monotonicity guard the settings kinds enforce — a replayed older `config.apply` supersedes the newer candidate in library, persistence, and applied-state — verified: the KIND_RUNBOOK branch returns before `classify_delivery`. Fix: apply the same changes-held-version check.
  - `[medium]` `[patch]` the opportunistic Validation gate is attributed by trigger only — any resolved host-health cycle validates every matching delivered candidate, though the plan was never shown to follow the runbook — verified: `record_runbook_gates` passes `Symptom("host-health")` unconditionally; plan→runbook attribution does not exist. Fix (minimal honest tightening): the opportunistic path requires the candidate to declare at least one `[[validation]]` criterion (the author's own attribution contract), records the outcome via `record_outcome` so attempts/success_rate live, and the approximation is documented on the runbook card hint and in Implementation Notes as spec-010 material (plan→runbook attribution).
  - `[low]` `[patch]` lost-update race: `decide`/`record_earned_gate` drop the library lock before `persist_delivered` re-reads and writes, so an interleaved superseding `deliver` can persist stale gates under new provenance — verified structurally. Fix: a manager-level ordering mutex held across mutate+persist.
  - `[low]` `[patch]` status spellings disagree: `runbooks_list` uses Rust Debug (`Candidate`) while the sentinel view and the web use serde snake_case (`candidate`), and `gates` in the same IPC entry are snake_case — verified in `handler.rs`. Fix: serialize status with the serde spelling.
  - `[low]` `[patch]` delivered candidates' `attempts`/`success_rate` never move yet both surfaces present them as live history — verified: `on_procedure_validated` never calls `record_outcome`. Fix: folded into the medium hook fix above.
  - `[low]` `[reject]` the Evaluation gate is trigger corroboration (a Resolved episode with remediation matching the trigger signature), not the spec's "steps checked against recorded episodes via the runbooks criterion" — true, but the spec's wording described machinery the memory layer does not hold: episodes carry free-text remediation (plan objectives), no per-step linkage to runbook steps exists, and the Criterion type evaluates readings, not episodes. The built check is the honest subset — deterministic, local, never fabricating; strengthening it means new memory-layer semantics (not a direct correction). Everyday harm is nil (the gate still refuses without local resolved-episode evidence). Rejected with the limitation documented; a real episode↔runbook linkage is spec-010 material alongside plan→runbook attribution.
  - `[low]` `[patch]` `argus runbook list`'s gate data and the loader's declared-gates replay make the review finding F1 testable end-to-end; folded into the high fix (reject declaring gates + a delivery test that declares all six and must land gateless).
- patches applied (re-engaged step-03 implementer; targeted tests green, full re-verification on the orchestrator side): `deliver` refuses deliveries whose parsed runbook declares ANY gates ("a delivered runbook cannot declare gates; it earns them here") + test declaring all six; `decide` refuses non-delivered (file-owned) names; the KIND_RUNBOOK branch moved below the `classify_delivery` held-version check (stale deliveries rejected, test v5-then-v4); the opportunistic hook requires declared `[[validation]]` criteria and records `record_outcome(true)` (attempts/success_rate live); a manager-level ordering mutex closes the mutate-vs-persist race (order → library → delivered everywhere); `runbooks_list` serializes status via serde matching every other surface.

## Design Notes

- Provenance is the audit story: every delivered candidate remembers the configuration version that delivered it; the runbook card shows it.
- The brain's procedure linkage is by trigger + name (`remember()` records procedure outcomes per acting cycle); Validation/Policy recording keys on the same linkage — if the brain cannot attribute a plan to a runbook, the gates wait (never guessed).
- The `runbook.decision` control message reuses the approval-decision discipline: single consumer, idempotent by name+decision, result reported back; the ladder's own errors are the validation.

## Verification

**Commands:**
- argus-ai: `export PATH="$HOME/.cargo/bin:$PATH"; cargo test --locked --workspace --all-targets` — expected: all pass (baseline 1097), `cargo clippy --workspace --all-targets` 0 warnings, `cargo fmt --check` clean.
- app-platform: `pnpm build && pnpm test` — expected: 8/8, 12/12.

**Manual checks (post-deploy, cloud first):**
- Deploy a `runbook` configuration from the dashboard/API → candidate appears on the instance page with Evaluation+Simulation (or logged reasons); `argus runbook list` shows gates; approve/promote round-trip lands `Promoted`.
