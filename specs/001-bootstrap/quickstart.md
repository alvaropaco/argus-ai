# Quickstart / Validation Guide

**Spec:** `specs/001-bootstrap/spec.md`
**Status:** Draft
**Date:** 2026-09-16

Runnable end-to-end scenarios that prove the bootstrap feature works. This is a
validation/run guide; implementation details belong in `tasks.md` and the
implementation phase.

## Prerequisites

- Linux/Unix host (no Kubernetes, no NATS, no eBPF required).
- Rust stable toolchain (see `rust-toolchain.toml`; `rustfmt` + `clippy`).
- Optional: systemd for the packaged service; not required to build/run.

## Scenario 1 — Build and lint the workspace

```bash
cargo build --workspace
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

**Expected:** clean build, no formatting/clippy violations, all tests pass
(including domain invariant, IPC contract, policy, repository, and event-bus tests).

## Scenario 2 — CLI version

```bash
argus --version
```

**Expected:** prints `argus <semver>` (acceptance criterion 1).

## Scenario 3 — Daemon starts and becomes ready (no systemd)

```bash
argusd --config /tmp/argus/config.toml &
argus health            # or: argus status
```

**Expected:** `argusd` initializes state (LanceDB), starts the local event bus, emits
`argus.started` then `argus.ready`, and `argus health` returns `state: "Ready"` over
the Unix socket (acceptance criteria 4 & 5).

## Scenario 4 — Privileged requests are rejected

```bash
# as a peer not in the authorized UID set (or an unknown operation)
argus exec '{"operation":"host.process.signal", …}'
```

**Expected:** `DENIED` / `UNAUTHORIZED` / `UNKNOWN_OPERATION` — no privileged
operation crosses the policy/executor boundary (acceptance criterion 5, spec § 8).

## Scenario 5 — Graceful degradation (optional components absent)

Run with no NATS configured and no OpenTelemetry exporter:

```bash
argusd --config /tmp/argus/minimal.toml
```

**Expected:** daemon reports `Ready` (or `Degraded` only if LanceDB init fails),
local structured logs continue, and the local event bus still delivers
`argus.ready`/`plugin.*` events (acceptance criterion 6, FR-006).

## Scenario 6 — Plugin/MCP discovery boundary

```bash
argus plugins list
argus capabilities list
```

**Expected:** `capabilities.list` returns the four bootstrap capabilities
(`host.status.read`, `argus.health.read`, `argus.config.read`, `argus.plugins.list`);
a failing plugin surfaces as `plugin.failed` + `state: Failed` while the core stays
available (spec § 9, acceptance criterion 9).

## Scenario 7 — systemd packaging (Linux + systemd)

```bash
sudo systemctl enable --now argusd
systemctl status argusd
argus health
```

**Expected:** unit hardening (`NoNewPrivileges`, `ProtectSystem=strict`,
`ProtectHome`, `PrivateTmp`) is active; `argusd` starts, health is queryable, and an
unexpected exit is restarted by systemd (acceptance criteria 3 & 8, spec § 9).

## Scenario 8 — Observability

With `tracing` configured, exercise `argus health` and confirm structured logs carry
a `correlation_id` matching the request; with OTLP configured, confirm a trace/span
is exported (acceptance criterion 8, FR-008).

---

Full detail: `data-model.md`, `contracts/ipc-protocol.md`,
`contracts/capabilities.md`, `contracts/events.md`.
