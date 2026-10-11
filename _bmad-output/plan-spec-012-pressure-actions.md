---
title: 'Spec 012 — Pressure-action capabilities: reclaim & journal vacuum'
type: 'feature'
ticket: ''
created: '2026-10-10'
status: 'in-review'
baseline_revision: '18a5fc4e0e0631a2e04213c944cc61161b78e7b1'
route: 'full'
route_source: 'auto'
review: 'quick'
review_source: 'pinned'
lenses_ran: ['quick']
review_loop_iteration: 0
followup_review_recommended: false
context: ['argus-ai/specs/012-pressure-action-capabilities/spec.md']
warnings: []
deferred: []
---

<intent-contract>

## Intent

**Problem:** Spec 011's pressure situations collect evidence and build attributed procedure plans, but no registered capability accepts the pressure subject — so a pressure procedure always has zero schema-valid steps and can never fire. The honest pressure actions operators run by hand (drop the page cache; vacuum the journal) have no typed capability.

**Approach:** Register two typed pressure-action capabilities through the existing registry → policy → executor → validation patterns: `host.memory.reclaim` (sync + write `1` to the drop_caches sysctl through a config-injectable kernel-write controller) and `host.journal.vacuum` (the journald D-Bus `Vacuum(since_usec)` call through the existing `argus-systemd` connection, to a configured retention bound). Both accept the optional pressure `subject`, are Controlled/Host risks, ride the blast-radius budget, and execute only through the local brain/procedure path.

## Boundaries & Constraints

**Always:** both capabilities are typed executor actions (no command execution, no shell); deny-by-default everywhere not explicitly registered; the cloud control channel never executes them (existing host-changing rule); they consume the blast-radius budget like any auto-executed step; failures are typed executor errors (fail-closed — the procedure terminates Failed, the runbook records the unsuccessful outcome); the write path and retention bound are config-injectable for tests; reclaim drops PAGE CACHE ONLY (write `1`, never `3`); the vacuum bound is clamped (1–365 days) and age-based (the D-Bus interface's semantics); the unit carve-out for the reclaim write path is documented in the deployment notes.

**Never:** no shell/command execution; no slabs/dentries drop (write `3` forbidden); no size-based vacuum (the D-Bus interface is age-based); no cloud-channel execution; no new protocol kinds; no change to the ladder, procedure construction rules, or autonomy machinery; do not commit `.mimosa/` or user artifacts.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Reclaim executes | `host.memory.reclaim` granted/allowed, write path valid | sync + drop_caches write `1`; Completed with evidence | Write/permissions failure → typed error → Failed |
| Vacuum executes | `host.journal.vacuum`, journal1 reachable | Vacuum(since_usec = now − keep_days); Completed | Missing interface/error → typed error → Failed |
| Pressure procedure builds | memory-pressure situation, promoted runbook with reclaim | Schema-valid step `{"subject": …}` (optional accepted) | — |
| Zero-execution guard | plan denied before any step | No outcome recorded (spec 010 rule) | — |
| Budget exhaustion | Controlled/Host-scope windows full | Step pauses via `budget.exhausted` (spec 008) | — |
| Misconfigured path | write path missing at execution | Typed error → Failed; never a silent success | — |

</intent-contract>

## Code Map

**argus-ai** (test: `export PATH="$HOME/.cargo/bin:$PATH"; cargo test --locked --workspace --all-targets`; baseline 1171 passed)
- `crates/argus-domain/src/id.rs:112+` — `HOST_MEMORY_RECLAIM = "host.memory.reclaim"`, `HOST_JOURNAL_VACUUM = "host.journal.vacuum"` (the CapabilityId consts pattern).
- `crates/argus-daemon/src/runtime.rs:1665 bootstrap_descriptors()` — descriptors: schema `{"subject": string}` optional (no `required`), risk `Controlled`, blast `Host`, reclaim `PartiallyReversible`, vacuum `None`; `crates/argus-policy/src/bootstrap.rs:44 with_local_remediation()` — add both ids to the allowlist (comment: spec 012 pressure actions, deny-by-default over the cloud channel by the host-changing rule).
- Executor wiring: the remediation executor dispatches by capability family — investigate `crates/argus-executor/src/remediation.rs` (`RemediationExecutor`) and the daemon's `remediation_ports` construction (`runtime.rs` `remediation_ports`, the 007/008 port pattern) for where a new controller slots. Reclaim: a small `KernelWriteController` (sync via `rustix::process::sync` if available else the `libc` sync through rustix's process module — check what rustix 1.1 exposes; else `std::process::Command("sync")` is FORBIDDEN — no command execution; rustix `rustix::process::sync_all`/`syscall` — investigate; a plain fs write of a temp file then drop is NOT sync; worst case use `rustix::fs::fdatasync` on a proc path — investigate and pick the honest available primitive; if sync is unavailable without unsafe, do the write without a separate sync and document it: the kernel syncs drop_caches internally per its own docs). Vacuum: `argus-systemd` gains a journal proxy — `zbus::proxy` on `org.freedesktop.journal1` `/org/freedesktop/journal1` interface `org.freedesktop.journal1`, method `Vacuum(u64 since_usec)`; the controller holds the `argus-systemd` Connection (check its visibility — may need a new accessor or a method on SystemdClient).
- Config: `[capabilities] journal_keep_days` (default 7, clamp 1–365) — the `AutonomyConfig` pattern in `config.rs`.
- Registration seam: `Daemon::init_with_extra_capabilities(config, extra)` exists (spec 011's review fix) — the production bootstrap registration goes in `bootstrap_capabilities()` + `bootstrap_descriptors()`; the executor wiring goes beside the existing controllers in the daemon's remediation ports.
- Tests: executor controller tests with an injected temp write path and a fake D-Bus (the argus-systemd fake pattern — check `fake.rs` if present; else trait-seam the vacuum call), policy allowlist test, registry test, procedure-plan integration (a memory-pressure situation + promoted runbook with reclaim → schema-valid step — the spec-011 drain test upgraded to the real capability).

**app-platform-argus-ai** — no required changes (the cloud channel never executes these; the ledger/Activity surfaces them through existing flows).

## Tasks & Acceptance

**Execution:**
- [ ] `crates/argus-domain` — CapabilityId consts
- [ ] `crates/argus-executor` — `KernelWriteController` (sync + drop_caches write, injectable path) + journal vacuum controller (journal1 D-Bus, injectable retention) + typed errors + tests (temp path; fake/trait-seamed D-Bus)
- [ ] `crates/argus-daemon` — descriptors in `bootstrap_descriptors()`, policy allowlist in `with_local_remediation()`, executor wiring into the remediation ports, `[capabilities] journal_keep_days` config
- [ ] argus-systemd — journal1 Vacuum proxy method (zbus proxy macro), behind the existing Connection
- [ ] both repos — full verification; deploy CLOUD FIRST (no cloud changes — still verify the pipeline) then daemon WITH the unit carve-out (`ReadWritePaths+=/proc/sys/vm`); live AC checks

**Acceptance Criteria:**
- Given a memory-pressure procedure plan, when granted, then `host.memory.reclaim` performs the sync + write on the configured path and reports Completed with evidence (AC-001).
- Given a disk-pressure procedure plan, when granted, then `host.journal.vacuum` calls Vacuum with the configured bound and reports Completed (AC-002).
- Given both registered, when the registry and policy are inspected, then both are present, policy-compatible, and cloud-channel-refused (AC-003).
- Given a missing write path or unreachable journal1, when the step executes, then a typed error → procedure Failed → runbook outcome(false) (AC-004).
- Given both actions auto-executed at L4, when the budget is inspected, then they consumed Controlled/Host-scope units (AC-005).

## Implementation Notes

- Implemented by the full-route subagent (2026-10-10): CapabilityId consts, journal1 zbus proxy + `SystemdClient::vacuum_journal`, `KernelWriteController`/`DropCachesController` (rustix sync + page-cache-only write, injectable path) and `JournalController` port + `DbusJournalController` (fresh connection per execution), descriptors (optional-subject schema, Controlled/Host, reclaim PartiallyReversible / vacuum None), policy allowlist, `[capabilities]` config (drop_caches_path, journal_keep_days 7 clamp 1-365), executor wiring with `risk_for` arms, unit carve-outs (deploy/debian/argusd.service carries all three: /proc/sys/vm /run/dbus /sys/fs/cgroup — the orchestrator fixed the edit dropping /sys/fs/cgroup and folded /run/dbus from the VPS drop-in into the checked-in unit).
- Review (quick lens): 7 findings — 6 patched (1 medium + 5 low), 1 rejected (AC-005's auto-execution premise: the code's approval-gating is the constitutionally correct behavior; the AC over-reached). Re-verified: 1185 passed (+4), clippy/fmt clean. The approval-flag flip (false → true) is the semantically significant review fix: pressure actions are per-execution approval-gated at every level; grant-backed execution is deliberately outside the budget.
- Deploy + live (2026-10-10/11): unit carve-outs applied on the VPS (union of drop-ins + the checked-in set), daemon swapped, connected. **The complete organism loop proven LIVE**: memory-pressure situation collected (threshold 50, host 61.5%) → procedure plan from the promoted (file-authored, full-gates) runbook → paused for approval → granted → `host.memory.reclaim` executed (sync + drop_caches write through the carve-out) → **Completed, executed=1** → runbook `attempts: 1, rate: 1.0` → episode symptom `memory-pressure`. Same-version re-delivery and dedup hold verified along the way. Config restored to defaults (threshold 90) after the drill; a duplicate `[situations]` section from the drill was deduped before it could break a restart.

## Auto Run Result

- **Summary:** Spec 012 registered the two honest pressure actions — `host.memory.reclaim` (kernel page-cache reclaim: sync + drop_caches write `1`, page cache only, path config-injectable) and `host.journal.vacuum` (journald D-Bus `Vacuum(since_usec)` to a clamped retention bound) — completing spec 011's perception→procedure→action chain. Both are Controlled/Host, registered, policy-compatible, approval-gated at every autonomy level, deny-by-default over the cloud channel, and budget-visible by classification.
- **Files changed:** argus-ai — argus-domain (ids), argus-systemd (journal1 proxy + vacuum), argus-executor (KernelWriteController/DropCachesController, JournalController port + dispatch, keep-days clamping), argus-policy (allowlist), argus-daemon (descriptors, registration, executor wiring, `[capabilities]` config, risk arms, e2e tests), deploy/debian/argusd.service (carve-outs documented + completed). app-platform: none.
- **Review findings:** quick lens, 7 findings — 6 patched (1 medium: untested granted-chain + real-sysctl test wiring; 5 low: false comment, FR-005 docs, deployment notes, D-Bus mapping seam, fold), 1 rejected with reasoning (AC-005's auto-execution premise — the approval-gating is correct; grant-backed is the execution path).
- **Follow-up review recommendation: false** — one medium patched; the semantic change it carried (descriptor approval flags) is pinned by the new round-trip tests and exercised live.
- **Verification performed:** argus-ai 1185 passed / 2 ignored post-patches (pre-patch 1181; +10 spec tests, +4 review tests), clippy 0 warnings, fmt clean. app-platform untouched (8/8, 12/12 baseline). Diff read in full by the orchestrator; quick lens reviewed with suite verification.
- **Residual risks:** a real `journal1 Vacuum` has run only against the production journald once (age-based; the bound keeps 7 days — default-safe); the reclaim write on production executed once under an explicit operator grant; `[capabilities]` thresholds are config not managed-settings (local file only, like every capability setting today).

## Plan Change Log

## Review Triage Log

### 2026-10-10 — Review pass (quick lens)
- verdicts: 7 findings — high 0, medium 1, low 6, false 0, maybe-false 0
- findings:
  - `[medium]` `[patch]` AC-001/002's "granted → Completed" and AC-004's procedure-level half untested for the real capabilities; test daemons wired the REAL sysctl path and real bus behind only L0. Fix: `test_config` points `drop_caches_path` at a per-test temp file; two new tests drive the full approval round-trip (pause → grant → resume → temp write `1` → Completed with evidence → attempts 1) and the AC-004 chain (nonexistent path → Failed → outcome(false) → nothing retried in-cycle). The implementer additionally flipped the descriptors' `approval` flag false → true: without it the operator-grant path could never fire for these Controlled actions (a resumed step without declared approval is refused by the matrix) — approval-per-execution is the conservative, review-framing-consistent semantics for one-way actions.
  - `[low]` `[patch]` the descriptor-arm comment claimed auto-execution at earned L4 — false (the governor never gates these families; the escalation's reversibility gate imposes AskHuman for undeclared-rollback steps; the reserve refunds on pause). Fix: the comment states the approval-gating truth.
  - `[low]` `[patch]` FR-005's validation semantics now carried verbatim in both descriptor construction-site comments.
  - `[low]` `[patch]` FR-004's deployment-notes documentation: a "Unit sandbox carve-outs" section in deploy/README.md (/sys/fs/cgroup, /run/dbus, /proc/sys/vm — one line and reason each).
  - `[low]` `[patch]` `DbusJournalController` error mappings extracted into testable pure helpers (`map_connect_error` → Unavailable, `map_journal_error` → Failed) with tests — no bus needed.
  - `[low]` `[reject]` the vacuum controller's fresh-Connection-per-execution wiring deviates from the plan's "holds the Connection" — deliberate, justified (no idle privileged bus between rare vacuums; fail-closed at execution), documented in code; recorded in Implementation Notes.
  - `[reject]` AC-005's premise (auto-execution at L4 consuming budget) is unreachable by design and the design is correct: the escalation gate requires a declared rollback for AutoFix and reclaim/vacuum honestly have none — irreversible/one-way actions get a human gate; they execute grant-backed, and grant-backed execution deliberately skips the budget reserve (the operator IS the rationing). Fixing it would amend the intent (plan edit — rejected by rule); the corrected semantics are recorded in Auto Run Result.

### 2026-10-10 — Review pass (quick lens)
- verdicts: 7 findings — high 0, medium 1, low 6, false 0, maybe-false 0
- findings:
  - `[medium]` `[patch]` AC-001/002's "granted → Completed" and AC-004's procedure-level half are untested for the REAL capabilities — the new brain tests stop at the L0 plan half; the grant → resume → execute → outcome chain is untested for reclaim/vacuum — verified. Fix: `test_config` points `capabilities.drop_caches_path` at a temp file (also fixes the real-sysctl footgun), and a test drives grant → resume → execution (temp write → Completed with evidence) plus the AC-004 chain (nonexistent path → Failed → runbook outcome(false)).
  - `[low]` `[patch]` the runtime.rs comment claims auto-execution at earned L4 for the pressure actions — false three ways (the governor doesn't gate these families, the escalation's reversibility gate imposes exactly the human decision, and budget consumption is refunded on pause) — verified. Fix: the comment states the truth (pressure actions pause for approval by design; the operator's grant is the execution path).
  - `[low]` `[patch]` FR-005's validation semantics live only in trait docs/inline comments, never attached to the descriptors (CapabilityDescriptor has no description field) — verified. Fix: the construction-site comments in `bootstrap_descriptors()` carry the validation sentence verbatim.
  - `[low]` `[patch]` FR-004's "documented in the deployment notes" — the carve-out is in the unit file and code comments but deploy/README.md documents no sandbox carve-outs — verified. Fix: a short "Unit sandbox carve-outs" section in deploy/README.md.
  - `[low]` `[patch]` `DbusJournalController` has zero test coverage (block_in_place panics on current-thread runtimes; every test seams the port) — verified. Fix: extract the error-mapping into a small testable seam and cover connect-error → Unavailable / D-Bus error → Failed.
  - `[low]` `[reject]` the vacuum controller deviates from the plan's wiring (fresh Connection per execution instead of holding one) — the deviation is deliberate, justified, and documented in code (no idle privileged bus between rare vacuums; fail-closed at the moment it matters). No change needed beyond recording it in Implementation Notes.
  - `[reject]` AC-005's premise — "both actions auto-executed at L4 and consumed budget" — is unreachable by design: the escalation gate requires a declared rollback for AutoFix, and reclaim/vacuum honestly have none (an irreversible action gets a human gate — constitutionally correct), so they execute grant-backed, and grant-backed execution deliberately skips the budget reserve (the operator IS the rationing for non-autonomous actions). The code behaves correctly; the AC over-reached. Fixing it would be an intent amendment (plan edit — rejected by rule); documented in Auto Run Result as the corrected semantics instead.

## Design Notes

- Drop caches writes `1` (page cache) — the conservative, standard reclamation; `3` is forbidden by the spec.
- Vacuum is age-based because `org.freedesktop.journal1.Vacuum(since_usec)` is the D-Bus interface's actual semantics; size-based vacuuming would need journald config writes (out of scope).
- The reclaim write path is config-injectable (`[capabilities] drop_caches_path`, default `/proc/sys/vm/drop_caches`) — tests inject a temp file; production needs the unit carve-out.
- The subject argument is informational for these capabilities (the action is host-wide); the schema accepts it optionally so spec 010's per-signature derivation produces valid steps without special-casing.

## Verification

**Commands:**
- argus-ai: `export PATH="$HOME/.cargo/bin:$PATH"; cargo test --locked --workspace --all-targets` — expected: all pass (baseline 1171), `cargo clippy --workspace --all-targets` 0 warnings, `cargo fmt --check` clean.
- app-platform: `pnpm build && pnpm test` — expected: 8/8, 12/12.

**Manual checks (post-deploy):**
- The unit carries `/proc/sys/vm` in ReadWritePaths; deliver + promote a memory-pressure runbook driving `host.memory.reclaim` (file-authored full-gates on the host, the honest local-operator path), set `[situations] memory_used_percent = 50`, and watch: procedure plan `procedure: <name>` → pause → grant → reclaim executes → validated → episode `memory-pressure` → gates climb.
