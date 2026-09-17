# Tasks: ARGUS Bootstrap Runtime

**Spec:** `specs/001-bootstrap/spec.md`
**Plan:** `specs/001-bootstrap/plan.md`

## Phase 1 — Repository and Workspace

- [ ] T001 [P0] Create the Rust Cargo workspace and crate skeleton defined by the bootstrap plan.
- [ ] T002 [P0] Configure workspace-wide Rust edition, linting, formatting, Clippy and test conventions.
- [ ] T003 [P0] Add CI checks for `cargo fmt`, `cargo clippy`, `cargo test` and workspace builds.

## Phase 2 — Domain Foundations

- [ ] T004 [P0] Implement `ResourceId`, `EnvironmentId`, `CapabilityId` and shared domain identifiers.
- [ ] T005 [P0] Implement `Observation`, `HealthStatus` and `DomainEvent` primitives.
- [ ] T006 [P0] Implement versioned capability and request/response contracts.
- [ ] T007 [P0] Add domain-level invariant tests.

## Phase 3 — Daemon and IPC

- [ ] T008 [P0] Implement `argusd` process bootstrap with Tokio.
- [ ] T009 [P0] Define versioned Unix-domain-socket IPC protocol.
- [ ] T010 [P0] Implement `health.get` IPC endpoint.
- [ ] T011 [P0] Implement `status.get` IPC endpoint.
- [ ] T012 [P1] Implement `config.get` IPC endpoint.
- [ ] T013 [P1] Implement `plugins.list` IPC endpoint.
- [ ] T014 [P1] Implement `capabilities.list` IPC endpoint.
- [ ] T015 [P0] Reject malformed, unsupported-version and unauthorized IPC requests.

## Phase 4 — Policy and Executor Boundary

- [ ] T016 [P0] Define `CapabilityRequest`, `AuthorizationRequest` and `PolicyDecision` contracts.
- [ ] T017 [P0] Implement bootstrap policy engine with explicit allow/deny behavior.
- [ ] T018 [P0] Create `argus-executor` typed capability boundary.
- [ ] T019 [P0] Expose only safe read-only capabilities in bootstrap.
- [ ] T020 [P0] Add tests proving privileged actions cannot bypass policy/executor boundaries.
- [ ] T021 [P1] Prepare Cedar integration interface without coupling domain contracts to Cedar types.

## Phase 5 — State and Events

- [ ] T022 [P0] Implement repository abstraction for operational state.
- [ ] T023 [P0] Implement initial LanceDB repository adapter.
- [ ] T024 [P1] Implement local Tokio event bus.
- [ ] T025 [P1] Define NATS/JetStream transport adapter interface without requiring NATS.
- [ ] T026 [P1] Add persistence and event contract tests.

## Phase 6 — TUI / CLI

- [ ] T027 [P0] Implement `argus` CLI entry point.
- [ ] T028 [P0] Implement Ratatui application shell.
- [ ] T029 [P1] Implement runtime health view.
- [ ] T030 [P1] Implement environment status view.
- [ ] T031 [P1] Implement plugin/capability status view.
- [ ] T032 [P1] Implement configuration/diagnostics views.
- [ ] T033 [P1] Add CLI/TUI smoke tests where practical.

## Phase 7 — Observability

- [ ] T034 [P0] Configure `tracing` subscriber and structured logging.
- [ ] T035 [P1] Initialize OpenTelemetry instrumentation boundary.
- [ ] T036 [P1] Add correlation/request IDs to daemon operations.
- [ ] T037 [P1] Emit startup, readiness, degraded and shutdown events.

## Phase 8 — Linux Service Packaging

- [ ] T038 [P0] Add hardened `argusd.service` systemd unit.
- [ ] T039 [P0] Validate service startup, restart and shutdown behavior.
- [ ] T040 [P1] Document least-privilege service configuration.

## Phase 9 — Plugin / MCP Bootstrap

- [ ] T041 [P1] Define plugin manifest schema in TOML.
- [ ] T042 [P1] Implement plugin manifest parser and validation.
- [ ] T043 [P1] Implement capability registration abstraction.
- [ ] T044 [P1] Add MCP runtime abstraction for future `rmcp` integration.
- [ ] T045 [P1] Define MCP Core packaging boundary.

## Phase 10 — Installer and Release

- [ ] T046 [P1] Define GitHub Release artifact layout.
- [ ] T047 [P1] Define checksum/signature verification interface.
- [ ] T048 [P1] Add `argus upgrade` command skeleton.
- [ ] T049 [P2] Prepare installer documentation for `https://argus.0x-ai.com`.

## Phase 11 — Verification

- [ ] T050 [P0] Verify constitution compliance.
- [ ] T051 [P0] Verify applicable ADR compliance.
- [ ] T052 [P0] Verify Linux-first operation without Kubernetes.
- [ ] T053 [P0] Verify operation without NATS.
- [ ] T054 [P0] Verify operation without eBPF.
- [ ] T055 [P0] Verify optional telemetry degradation behavior.
- [ ] T056 [P0] Verify no secrets are emitted in logs.
- [ ] T057 [P0] Verify all privileged code paths cross policy and executor boundaries.
- [ ] T058 [P1] Update architecture documentation with implementation deltas.
