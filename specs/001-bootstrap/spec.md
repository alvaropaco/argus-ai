# Feature Specification: ARGUS Bootstrap Runtime

**Status:** Baseline
**Spec ID:** 001
**Created:** 2026-09-16

## 1. Problem Statement

ARGUS needs a minimal but production-oriented bootstrap runtime that can be installed on a Linux/Unix host, start safely, expose its TUI/CLI, initialize configuration and state, and establish the architectural boundaries required for future autonomous infrastructure operations.

The bootstrap must not attempt to implement the complete autonomous SRE system. Its purpose is to establish the secure runtime skeleton on which discovery, policy, execution, MCP, plugins, telemetry, and autonomous operations can be built incrementally.

## 2. Objective

Deliver a minimal ARGUS installation that can:

1. install as a native Linux service;
2. launch the Rust CLI/TUI;
3. initialize configuration and state;
4. start `argusd` with the least practical privileges;
5. establish secure IPC between the CLI/AI-side runtime and privileged daemon;
6. expose a health/status model;
7. initialize the event bus and telemetry interfaces;
8. register the MCP Core architecture without requiring Kubernetes, NATS, or eBPF;
9. provide a stable foundation for future feature specifications.

## 3. Scope

### In scope

- Rust workspace bootstrap.
- `argus` CLI.
- `argusd` privileged daemon.
- Ratatui TUI shell.
- Unix-domain-socket IPC contract.
- Basic domain model and IDs.
- SQLite is NOT used; operational state uses the ADR-approved LanceDB abstraction.
- Local event bus abstraction.
- NATS adapter interface without requiring NATS.
- `tracing` and OpenTelemetry interfaces.
- Configuration and provider placeholders.
- Systemd unit for `argusd`.
- Installer/update scaffolding.
- Constitution/ADR compliance checks.

### Out of scope

- Full autonomous remediation.
- Production-grade eBPF telemetry.
- Full Kubernetes controller.
- Full cloud provider integrations.
- General-purpose agent orchestration.
- Arbitrary privileged shell execution.

## 4. Actors

- **Operator:** installs and initializes ARGUS.
- **ARGUS CLI/TUI:** presents configuration and status.
- **argusd:** privileged control boundary.
- **AI/Planning runtime:** future reasoning boundary; initially stubbed.
- **Systemd:** service manager.

## 5. Functional Requirements

### FR-001

The installer MUST install the ARGUS binaries and required MCP Core assets using signed/checksummed releases.

### FR-002

`argus init` MUST provide a TUI setup flow for initial configuration.

### FR-003

`argus` MUST expose status information without requiring root privileges.

### FR-004

`argusd` MUST run as a separate daemon and MUST NOT accept arbitrary shell commands as its primary API.

### FR-005

The CLI/TUI MUST communicate with `argusd` through an authenticated local IPC mechanism.

### FR-006

The initial runtime MUST work without Kubernetes, NATS, eBPF, external observability backends, or cloud credentials.

### FR-007

The runtime MUST expose typed health and readiness information.

### FR-008

Core components MUST emit structured logs and OpenTelemetry-compatible telemetry.

### FR-009

The bootstrap MUST expose extension points for MCPs and plugins without hard-coding infrastructure-specific logic into the core domain.

## 6. Non-Functional Requirements

### Security

- Least privilege.
- Explicit IPC authorization.
- No unrestricted agent/root shell.
- No secrets in plaintext configuration by default.
- Security-sensitive capabilities declared explicitly.

### Reliability

- Service auto-restart through systemd.
- Deterministic startup/shutdown.
- Graceful degradation of optional components.

### Performance

- Low idle CPU and memory footprint.
- Non-blocking asynchronous runtime through Tokio.

### Observability

- `tracing` structured logs.
- OpenTelemetry instrumentation boundary.
- Correlation IDs for requests and future executions.

## 7. Domain Impact

Initial entities:

- `EnvironmentId`
- `Host`
- `ResourceId`
- `CapabilityId`
- `Observation`
- `HealthStatus`
- `DomainEvent`
- `PluginManifest`

Initial capabilities:

- `host.status.read`
- `argus.health.read`
- `argus.config.read`
- `argus.plugins.list`

Initial events:

- `argus.started`
- `argus.ready`
- `argus.degraded`
- `plugin.loaded`
- `plugin.failed`

## 8. Security and Policy

The bootstrap establishes but does not yet implement the complete Cedar policy model.

The daemon/executor boundary MUST exist before privileged capabilities are added.

The first implementation SHOULD use Unix socket filesystem permissions plus explicit request validation, with the architecture prepared for Cedar authorization.

## 9. Failure and Recovery

- If LanceDB initialization fails, ARGUS MUST report degraded state rather than silently starting an incomplete autonomous runtime.
- If optional NATS configuration fails, local eventing MUST continue.
- If optional telemetry export fails, local structured logs MUST continue.
- If an MCP plugin fails to load, the core MUST remain available and expose the plugin as degraded.
- If `argusd` exits unexpectedly, systemd SHOULD restart it according to the packaged unit.

## 10. Acceptance Criteria

- [ ] `argus --version` works.
- [ ] `argus init` launches the Ratatui setup flow.
- [ ] `argusd` installs and starts through systemd.
- [ ] CLI/TUI can query daemon health through local IPC.
- [ ] Unauthorized privileged requests are rejected.
- [ ] Core can start with no Kubernetes and no NATS.
- [ ] Structured logs are emitted.
- [ ] OpenTelemetry instrumentation is initialized when configured.
- [ ] Plugin/MCP discovery architecture exists.
- [ ] Relevant architecture invariants are covered by tests.

## 11. Architectural Constraints

This feature is governed by:

- ADR-000 — ARGUS product boundary.
- ADR-001 — Rust.
- ADR-002 — Ratatui.
- ADR-003 — MCP Core bundled with installer.
- ADR-004 — Plug-and-play architecture.
- ADR-005 — Native `rmcp` plus external MCP support.
- ADR-006 — Tokio.
- ADR-008 — `argus-executor` in Rust.
- ADR-009 — Rust security boundary.
- ADR-010 — Cedar + ARGUS policy layer.
- ADR-011 — LanceDB.
- ADR-012 — Local event bus + optional NATS.
- ADR-014 — LLM provider abstraction.
- ADR-015 — tracing + OpenTelemetry.
- ADR-016 — Plugin architecture + TOML manifest.
- ADR-017 — GitHub Releases + signature/checksum + `argus upgrade`.
- ADR-018 — Kernel-native enterprise domain model.

## 12. Open Questions

- Exact Unix socket authentication mechanism.
- Initial Linux capability set for `argusd`.
- Secret backend for provider keys.
- Exact LanceDB schema for bootstrap state.
- Installer artifact signing mechanism.
