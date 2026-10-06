# Changelog

All notable changes to ARGUS are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/) and the project versions
the whole workspace together.

## [0.2.0] — 2026-10-06 — "The Brain"

The first complete ARGUS brain: a Linux host runtime that observes
itself, reasons with a configured AI decision engine, plans, and acts
inside a typed security boundary — validated live on a production Linux
host.

### Added

- **The running brain** (spec 005): the daemon now thinks. A periodic
  loop gathers typed evidence from live systemd, poses JEV structured
  decisions (`choice`/`score`/`noul`) to the configured provider, gates on
  confidence, and executes the resulting plan through the full safety
  boundary at the operator's autonomy level — **L0 observe-only by
  default**; execution is opt-in. `argus diagnose` triggers a cycle on
  demand; the reasoning history lands in `plan.list`, episodic memory,
  and aggregated procedure outcomes. (IPC 0.7.0: `brain.diagnose`,
  `runbooks.list`.)
- **DeepSeek provider adapter**: OpenAI-compatible chat-completions
  behind the `DecisionProvider` trait, JSON mode, fail-closed schema
  validation (no invented answers, no smuggled options), fallback models.
  The `[model]` section of `argus.toml` is now consumed at startup
  together with the `0600` secret store; `laya` sidecar remains an
  alternative.
- **Adaptive layer** (spec 003, milestones 3–6): typed host remediation
  (`host.process.signal`, `container.restart`, cgroup v2 freeze/thaw)
  under autopilot governance; the Kubernetes brain (typed projections,
  deterministic evidence bundles, typed self-healing capabilities,
  deny-by-default drain/reschedule); labeled predictions with explicit
  uncertainty; change intelligence ("what changed before the incident"
  as a cited hypothesis); five deterministic memory layers with
  incident similarity over typed fields; declarative runbooks with the
  six-gate promotion ladder; impact simulation; seven deterministic
  report kinds; the L0–L5 autonomy model with the escalation decision
  (OBSERVE/EXPLAIN/RECOMMEND/ASK-HUMAN/AUTO-FIX, policy final); sentinel
  mode with safe-mode degradation (IPC 0.5.0–0.6.0: `sentinel.get`,
  `report.generate`, `plan.list`, `audit.list`); self-observability
  counters; a TUI Sentinel tab.
- **Live validation harness** (`argus-daemon` live-effects example):
  drives the real remediation loop against live cgroup v2, the Docker
  socket, `/proc` signals, and systemd D-Bus, for validating deployments.

### Fixed

- **Chunked Docker responses**: the Docker daemon answers with
  `Transfer-Encoding: chunked`; both hand-rolled HTTP parsers fed the
  chunk framing to serde, so every live container inspect failed. Both
  clients now share a chunk-decoding parser (found live on the VPS;
  fixture tests could never catch it).
- Hermetic config test (a deployed `argus.toml` on the host no longer
  breaks the test suite).

### Operational notes

- The `argusd.service` unit now needs `ReadWritePaths=/etc/argus
  /sys/fs/cgroup` for the cgroup freezer (a kernel-native file write,
  scoped by the typed executor and the governor).
- SQLite state from 0.1.x upgrades in place (additive tables).
- Full security audit of this release: see the Mimosa scan record for
  0.2.0 (findings tracked in the repository).

## [0.1.10] — 2026-09 — Bootstrap + AI runtime

The secure skeleton (spec 001): `argusd` daemon, authenticated IPC, the
policy/executor boundary, read-only capabilities, SQLite state, the
event bus, observability. The AI runtime (spec 002): the JEV reasoning
gateway, hypothesis/plan/execution persistence, provider health,
plan/audit IPC, the setup TUI, and cloud enrollment with the
credential-ref secret store.
