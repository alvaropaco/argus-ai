# Implementation Plan: ARGUS Bootstrap Runtime

**Spec:** `specs/001-bootstrap/spec.md`
**Status:** Baseline

## 1. Workspace

Create a Cargo workspace with an initial minimal set of crates:

```text
crates/
├── argus-domain
├── argus-core
├── argus-cli
├── argus-tui
├── argus-daemon
├── argus-ipc
├── argus-policy
├── argus-executor
├── argus-events
├── argus-state
└── argus-observability
```

The workspace should make future adapters/plugins additive rather than forcing early implementation of every subsystem.

## 2. Process Boundaries

```text
argus CLI/TUI
      │
      │ Unix domain socket
      ▼
argusd
      │
      ├── core
      ├── policy
      ├── executor
      ├── state
      └── events
```

The AI/planning side is not granted privileged access in this bootstrap stage.

## 3. IPC

Define a versioned request/response protocol over a Unix domain socket.

Requests should use typed operations rather than shell commands.

Initial API:

```text
health.get
status.get
config.get
plugins.list
capabilities.list
```

Every request must contain a correlation ID and protocol version.

## 4. Domain Contracts

Implement minimal strongly typed domain primitives:

```text
ResourceId
EnvironmentId
CapabilityId
Observation
HealthStatus
DomainEvent
PluginManifest
RequestContext
```

Avoid infrastructure-specific types in `argus-domain`.

## 5. State

Implement the LanceDB repository behind a trait-based persistence boundary.

The bootstrap only needs state sufficient for:

- environment identity;
- runtime status;
- plugin registry metadata;
- initial observations;
- audit events.

Do not leak LanceDB APIs into domain entities.

## 6. Events

Implement a local event bus using Tokio primitives.

Define a transport abstraction so the NATS adapter can be added without changing event producers/consumers.

## 7. Policy

Create the policy boundary and request model.

Initial behavior:

- read-only operations allowed;
- unknown capabilities denied;
- privileged operation classes require explicit authorization;
- policy implementation prepared for Cedar.

## 8. Executor

Create `argus-executor` with typed command/capability structures.

For bootstrap, expose only safe read-only operations.

Do not implement a general arbitrary shell tool as part of the primary API.

## 9. TUI

Use Ratatui with Crossterm.

Initial screens:

```text
Welcome
Provider setup placeholder
Runtime status
Environment status
Plugin status
Configuration
Diagnostics
```

Keep presentation state separate from domain state.

## 10. Observability

Initialize:

- `tracing`;
- structured JSON/text logging options;
- OpenTelemetry SDK boundary;
- correlation/request IDs.

Telemetry export must be optional.

## 11. Systemd

Provide a hardened `argusd.service` with:

- dedicated service account where practical;
- restrictive filesystem access;
- controlled environment;
- restart policy;
- explicit capability configuration when needed;
- protected system directories where compatible;
- private temporary directory where compatible.

Do not prematurely request broad capabilities. Start with the minimum needed for bootstrap.

## 12. Installer

Prepare the repository structure for GitHub Release artifacts and the future installer URL:

```text
https://argus.0x-ai.com
```

The installer should eventually verify artifact checksums/signatures before installation.

## 13. Testing

Implement:

- domain unit tests;
- IPC contract tests;
- policy tests;
- repository tests;
- event-bus tests;
- TUI smoke tests where practical;
- daemon lifecycle integration test;
- systemd packaging validation.

## 14. Completion Criteria

The bootstrap is complete when:

1. Cargo workspace builds.
2. `argus --version` works.
3. `argus init` starts.
4. `argusd` starts under systemd.
5. CLI/TUI can query `argusd` health.
6. The daemon can persist/read bootstrap state through the repository interface.
7. Domain events work without NATS.
8. Telemetry initializes without requiring an external backend.
9. No privileged operation bypasses the policy/executor boundary.
10. Architecture documentation remains consistent with the ADRs and constitution.
