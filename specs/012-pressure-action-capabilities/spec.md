# Feature Specification: ARGUS Pressure-Action Capabilities — Reclaim & Journal Vacuum

**Status:** Draft
**Spec ID:** 012
**Created:** 2026-10-10

> Completes spec 011's pressure story end-to-end: the situation vocabulary
> has per-signature procedure plans, but no registered capability accepts
> the pressure subject — so a pressure procedure can never fire. This spec
> registers the two honest pressure actions: kernel page-cache reclaim
> (memory pressure) and a journald vacuum (disk pressure). No ADR: this is
> capability addition inside the existing registry/policy/executor patterns
> (the spec-004 k8s precedent).

## 1. Problem Statement

A `memory-pressure` or `disk-pressure` situation collects evidence, matches
a promoted runbook, and builds an attributed procedure plan — which then has
zero schema-valid steps, because no registered capability accepts the
pressure subject. The perception→procedure→action chain breaks at its last
link, and the honest pressure actions that operators run by hand (drop the
page cache; vacuum the journal) are exactly the kind of bounded, reversible
remediations the autonomy machinery exists to drive.

## 2. Objective

Register two typed pressure-action capabilities, wired through the existing
registry → policy → executor → validation boundary:

1. **`host.memory.reclaim`** — kernel page-cache reclaim: `sync`, then
   write `1` to `/proc/sys/vm/drop_caches` (page cache only — never
   slabs/dentries). Controlled risk, host blast radius, partially reversible
   (the cache repopulates naturally).
2. **`host.journal.vacuum`** — vacuum systemd-journald to a configured age
   bound via the journald D-Bus API (`org.freedesktop.journal1`), a typed
   D-Bus call through the existing `argus-systemd` connection — no command
   execution. Controlled risk, host blast radius, irreversible (vacuumed
   logs are gone), bounded by the configured retention.

Both accept the pressure subject (`{"subject": …}`) as an (optional,
informational) argument, so spec 010's per-signature procedure derivation
produces schema-valid steps.

## 3. Functional Requirements

**FR-001 (`host.memory.reclaim`)** The executor MUST gain a typed kernel
controller that performs the reclaim: `sync` (via the journald-independent
kernel path — `sync()` libc call through `rustix::process`), then write `1`
to the drop_caches sysctl. The write path is configurable (default
`/proc/sys/vm/drop_caches`) so tests inject a temp file. A failure (path
missing, write refused) is a typed executor error — fail-closed. The
capability descriptor: schema `{"subject": string, optional}`, risk
`Controlled`, blast radius `Host`, reversibility `PartiallyReversible`.

**FR-002 (`host.journal.vacuum`)** The executor MUST gain a typed journal
controller that vacuates journald via the `org.freedesktop.journal1`
D-Bus `Vacuum(since_usec)` call through the existing `argus-systemd`
connection: the retention bound is a config value
(`[capabilities] journal_keep_days`, default 7, clamped 1–365); the call
vacuums everything older than `now − keep_days`. The descriptor: schema
`{"subject": string, optional}`, risk `Controlled`, blast radius `Host`,
reversibility `None`. A missing journal1 interface is a typed error.

**FR-003 (Registry & policy)** Both capabilities MUST be registered in the
bootstrap registry with the descriptors above, and both MUST be policy-
compatible (added to the local-remediation allowlist). The cloud control
channel remains host-changing-never: these capabilities execute only through
the local brain/procedure path, behind policy, escalation, approval, and
budget as always.

**FR-004 (Procedure integration)** Spec 010's procedure construction MUST
accept the pressure subject for both: the pressure signature derives
`{"subject": <situation subject>}`, `input_matches` validates (the schemas
treat it as optional-informational), and the steps build — completing the
perception→procedure→action chain. The unit carve-out for the reclaim write
path (`ReadWritePaths+=/proc/sys/vm`) MUST be documented in the deployment
notes and applied with the daemon deploy.

**FR-005 (Validation contract)** Each capability's descriptor carries
validation semantics in its docs: reclaim validates when the write is
accepted by the kernel; vacuum validates when the D-Bus call returns. The
procedure validation machinery (spec 005/010) consumes these outcomes
unchanged.

## 4. Non-Functional Requirements

- No command execution: reclaim is a sysctl write through a typed
  controller; vacuum is a D-Bus call through the existing connection.
- Deny-by-default everywhere the capability is not explicitly registered;
  the cloud channel never carries them (existing host-changing rule).
- Both actions ride the blast-radius budget (`Controlled: 5/day`,
  `Host-scope: 3/day` defaults) — they are rationed like any auto-executed
  step.
- The write path and retention bound are config-injectable for tests.

## 5. Acceptance Criteria

- [ ] AC-001 — A procedure plan for a `memory-pressure` situation builds
      schema-valid `host.memory.reclaim` steps and, granted, the controller
      performs sync + drop_caches write (temp-path injected) and reports
      Completed with the evidence.
- [ ] AC-002 — A procedure plan for a `disk-pressure` situation builds
      `host.journal.vacuum` steps and, granted, the controller calls the
      journal1 Vacuum with the configured bound (fake D-Bus in tests) and
      reports Completed.
- [ ] AC-003 — Both capabilities are registered with their descriptors,
      policy-compatible, and denied over the cloud control channel.
- [ ] AC-004 — A failed write (missing path) or a failed Vacuum is a typed
      executor error: the procedure terminates Failed, the runbook records
      the unsuccessful outcome, and nothing is retried within the same
      cycle.
- [ ] AC-005 — Both actions consume the blast-radius budget as Controlled/
      Host-scope executions.

## 6. Out of Scope

- Further pressure capabilities (eviction, swap tuning, service-aware
  reclamation).
- Size-based vacuum (the D-Bus interface is age-based; a size target needs
  journald configuration writes).
- Cloud-channel execution of either capability (never permitted, existing
  rule).
