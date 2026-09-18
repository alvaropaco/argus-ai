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

## Scenario 3 — Interactive setup (`argus init`)

```bash
argus init
```

**Expected:** a wizard opens with the bootstrap paths (socket, state,
environment) and the AI provider fields (provider, model, fallback models, base
URL, API token — masked on screen). Saving (Enter/Ctrl-S) writes two files to
the current directory:

- `argus.toml` — non-secret config, including a `[model]` section;
- `argus.secrets.toml` — the API token, mode `0600`.

The token never appears in `argus.toml` or in `argus config` output.

## Scenario 4 — Daemon starts and becomes ready (no systemd)

```bash
argusd --socket /tmp/argus/argusd.sock --state /tmp/argus/argus.db &
argus --socket /tmp/argus/argusd.sock health   # or: status
```

**Expected:** `argusd` initializes state (SQLite interim backend), starts the
local event bus, emits `argus.started` then `argus.ready`, and
`argus health` returns `state: "Ready"` over the Unix socket (acceptance
criteria 4 & 5).

## Scenario 5 — Privileged requests are rejected

The bootstrap exposes only read-only capabilities; there is no privileged
execution path yet:

```bash
argus capabilities
```

**Expected:** only the four read-only capabilities are listed
(`host.status.read`, `argus.health.read`, `argus.config.read`,
`argus.plugins.list`); no privileged operation crosses the policy/executor
boundary (acceptance criterion 5, spec § 8).

## Scenario 6 — Graceful degradation (optional components absent)

Run with no NATS configured and no OpenTelemetry exporter:

```bash
argusd --socket /tmp/argus/minimal.sock --state /tmp/argus/minimal.db
```

**Expected:** daemon reports `Ready` (or `Degraded` only if state init fails),
local structured logs continue, and the local event bus still delivers
`argus.ready`/`plugin.*` events (acceptance criterion 6, FR-006).

## Scenario 7 — Plugin/MCP discovery boundary

```bash
argus plugins
argus capabilities
```

**Expected:** `argus capabilities` returns the four bootstrap capabilities
(`host.status.read`, `argus.health.read`, `argus.config.read`, `argus.plugins.list`);
a failing plugin surfaces as `plugin.failed` + `state: Failed` while the core stays
available (spec § 9, acceptance criterion 9).

## Scenario 8 — systemd packaging (Linux + systemd)

```bash
sudo systemctl enable --now argusd
systemctl status argusd
argus health
```

**Expected:** unit hardening (`NoNewPrivileges`, `ProtectSystem=strict`,
`ProtectHome`, `PrivateTmp`) is active; `argusd` starts, health is queryable, and an
unexpected exit is restarted by systemd (acceptance criteria 3 & 8, spec § 9).

The unit runs `/usr/bin/argusd`; to use a binary installed elsewhere, set
`ARGUSD_BINARY` in `/etc/default/argusd` (see `deploy/debian/argusd.default`).

## Scenario 9 — Observability

With `tracing` configured, exercise `argus health` and confirm structured logs carry
a `correlation_id` matching the request; with OTLP configured, confirm a trace/span
is exported (acceptance criterion 8, FR-008).

---

Full detail: `data-model.md`, `contracts/ipc-protocol.md`,
`contracts/capabilities.md`, `contracts/events.md`.
