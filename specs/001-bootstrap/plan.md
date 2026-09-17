# Implementation Plan: ARGUS Bootstrap Runtime

**Spec:** `specs/001-bootstrap/spec.md`
**Status:** Draft
**Date:** 2026-09-16

## 1. Summary

Deliver a minimal but production-oriented bootstrap runtime: a Cargo workspace, the
`argus` CLI/TUI, the privileged `argusd` daemon, a versioned Unix-socket IPC contract,
the policy/executor boundary, an initial LanceDB-backed state repository, a local
event bus, `tracing`/OpenTelemetry instrumentation, and a hardened systemd unit —
all operable on a single Linux host with no Kubernetes, NATS, or eBPF.

The bootstrap intentionally does **not** implement autonomous remediation, eBPF
telemetry, a Kubernetes controller, or arbitrary shell execution.

## 2. Architecture Impact

Affected areas, mapped to the current (docs-only) baseline:

- **crates:** create the workspace and initial crates (see § 3).
- **domain contracts:** introduce strongly-typed identifiers, `Observation`,
  `HealthStatus`, `DomainEvent`, `PluginManifest`, `RequestContext`.
- **adapters/plugins:** plugin manifest schema + capability registry (no platform
  logic in core).
- **security boundaries:** `argusd`/executor boundary; IPC authorization via
  `SO_PEERCRED`; policy evaluator boundary.
- **state/persistence:** `DomainRepository` trait over LanceDB (never leaked into the
  domain).
- **event schemas:** transport-independent `DomainEvent` + local Tokio bus.
- **observability:** `tracing` subscriber + OpenTelemetry SDK boundary + correlation
  IDs.
- **installation/deployment:** `argusd.service`, installer/checksum interface,
  `argus upgrade` skeleton.

## 3. Proposed Design

### 3.1 Workspace

```text
crates/
├── argus-domain         # entities, value objects, ids
├── argus-core           # runtime lifecycle, coordination
├── argus-cli            # `argus` entry point
├── argus-tui            # Ratatui + Crossterm presentation
├── argus-daemon         # `argusd` process
├── argus-ipc            # versioned Unix-socket protocol
├── argus-policy         # PolicyEvaluator boundary + bootstrap evaluator
├── argus-executor       # typed capability boundary (read-only in bootstrap)
├── argus-events         # DomainEvent + EventTransport (local bus)
├── argus-state          # DomainRepository + LanceDB adapter
└── argus-observability  # tracing + OpenTelemetry init
```

### 3.2 Process boundaries

```text
argus CLI/TUI (unprivileged)
      │ Unix domain socket (SO_PEERCRED)
      ▼
argusd (privileged, minimized)
      ├── core → policy → executor
      ├── state (LanceDB via repository)
      └── events (local bus)
```

The AI/planning side is stubbed and receives **no** privileged access.

### 3.3 Key decisions

See `research.md` for full Decision/Rationale/Alternatives. Summary:

- IPC auth: filesystem perms + `SO_PEERCRED` (Cedar-ready later).
- `argusd` capabilities: none in bootstrap (empty `CapabilityBoundingSet`).
- Secrets: `argus-secrets` trait + `0600` filesystem backend (not in LanceDB).
- State schema: 5 logical tables (`environment`, `runtime_status`, `plugins`,
  `observations`, `audit_events`).
- Signing: SHA-256 checksums now; Sigstore/cosign keyless as release target.
- IPC framing: newline-delimited JSON envelope.
- Event bus: Tokio `broadcast` behind an `EventTransport` trait.
- Policy: in-process deterministic evaluator now; Cedar behind `PolicyEvaluator` later.

## 4. Interfaces and Contracts

- **IPC:** `contracts/ipc-protocol.md` — versioned envelope, operations
  `health.get`, `status.get`, `config.get`, `plugins.list`, `capabilities.list`,
  error codes, compatibility rules.
- **Capabilities:** `contracts/capabilities.md` — four read-only capabilities with
  risk class + required privileges.
- **Events:** `contracts/events.md` — schema + bootstrap event types.
- **Domain traits (target, not final API):** `DomainRepository`, `PolicyEvaluator`,
  `Executor`, `EventTransport`, `Capability` (see `docs/architecture/c4-solution-architecture.md` § 29).

## 5. Security Model

- **Trust boundaries:** AI/reasoning (untrusted data) → policy (deterministic) →
  privileged executor (`argusd` only) → OS.
- **IPC:** root-owned socket (`0600`), `SO_PEERCRED` identity, versioned envelope,
  no secrets in `config.get`.
- **Linux capabilities:** none ambient in bootstrap; hardened systemd unit
  (`NoNewPrivileges`, `ProtectSystem=strict`, `ProtectHome`, `PrivateTmp`).
- **Policy:** read-only allowed, unknown denied, privileged requires authorization
  (Cedar behind trait later).
- **Plugins:** declarative manifest permissions; never granted implicitly (ADR-016).
- **Secrets:** filesystem backend `0600`, redacted from config/logs/state.

## 6. Data / State Model

- See `data-model.md` for entities, validation, and transitions.
- Persistence through `DomainRepository`; backend types never cross into
  `argus-domain` (Principle 12).
- **Backend:** SQLite is the interim default (`SqliteRepository`); the LanceDB
  adapter (`LanceDbRepository`) is available behind the `lancedb` cargo feature
  (off by default). See the ADR-011 amendment note in § 12.
- Tables: `environment`, `health`, `observations` (immutable), `audit_events`
  (append-only).

## 7. Event Model

- Transport-independent `DomainEvent`; local bus via Tokio `broadcast`
  (ADR-012, no NATS required).
- Bootstrap events: `argus.started`, `argus.ready`, `argus.degraded`,
  `plugin.loaded`, `plugin.failed`.
- NATS/JetStream adapter is an additive transport behind `EventTransport`.

## 8. Observability

- `tracing` subscriber (structured JSON/text), OpenTelemetry SDK boundary, OTLP
  export optional.
- Correlation/request IDs propagated from IPC envelope → logs/traces/audit
  (FR-008, ADR-015).
- Startup/readiness/degraded/shutdown events emitted as structured telemetry.

## 9. Testing Strategy

- domain invariant unit tests;
- IPC contract tests (round-trip, malformed, version, auth, error codes);
- policy tests (allow/deny/require-approval, unknown denied, boundary non-bypass);
- repository tests (LanceDB adapter against temp dirs);
- event-bus tests (fan-out, ordering, no NATS);
- TUI/CLI smoke tests;
- daemon lifecycle integration test;
- systemd packaging validation;
- Linux-first: no Kubernetes/NATS/eBPF in test prerequisites.

## 10. Rollout / Migration

- Initial release: Linux binary + `argusd.service` via GitHub Releases
  (checksum-verified), installed via `curl -fsSL https://argus.0x-ai.com | sh`
  (ADR-017).
- `argus upgrade` verifies integrity before install and supports rollback where
  practical.
- No data migration needed (greenfield); repository abstraction allows backend
  swaps later.

## 11. Risks

| Risk | Impact | Mitigation |
|---|---|---|
| LanceDB Rust SDK maturity/churn | state layer instability | isolate behind `DomainRepository` trait; SQLite as interim default; LanceDB optional |
| LanceDB 0.38.0 upstream compile bug (`Error::Http`) | cannot build | workaround: enable `remote` feature; gate adapter behind `lancedb` feature (off by default) |
| IPC auth bypass | privilege escalation | `SO_PEERCRED` + socket perms + policy denial-by-default; tests |
| Bundled MCP supply chain | compromised dependency | ADR-003 security review; pinning; checksums (not in bootstrap scope) |
| Scope creep into autonomy/agents | violates product boundary | spec § 3 out-of-scope; ADR-013/017 guardrails |
| systemd hardening too strict | `argusd` can't start | validate unit in CI; keep hardening additive |

## 12. ADR Impact

Existing ADRs are **sufficient** for the bootstrap; no new ADR is required. The
following ADRs govern this feature: 000 (boundary), 001 (Rust), 002 (Ratatui),
003 (MCP Core), 004 (plugins), 005 (rmcp), 006 (Tokio), 007 (host discovery),
008 (executor), 009 (security boundary), 010 (Cedar policy), 011 (LanceDB),
012 (events), 013 (agent boundary), 014 (LLM abstraction), 015 (observability),
016 (plugin manifest), 017 (install/updates), 018 (domain model).

If a later feature changes an open question into a durable decision (e.g. secret
backend → keyring, or signing → Sigstore), that decision MUST be recorded as an ADR
before implementation.

**ADR-011 amendment needed:** this phase adopts **SQLite as the interim default
state backend** (behind `DomainRepository`), with LanceDB retained as an optional
`lancedb` feature. This temporarily supersedes the "SQLite is NOT used" note in the
spec and ADR-011's "LanceDB as primary store". The LanceDB Rust SDK (0.38.0) is
currently not production-ready: it has an upstream compile bug and pulls in a very
heavy dependency tree (datafusion). A follow-up ADR (or ADR-011 amendment) must
record the interim SQLite decision before it is considered durable.

## 13. Constitution Check

Gates evaluated against `.specify/memory/constitution.md`. All pass for the
bootstrap design:

| Principle | Status | Evidence |
|---|---|---|
| P1 Rust Core | PASS | workspace in Rust; executor/daemon Rust (ADR-001) |
| P2 Security Boundary | PASS | AI stubbed, no root shell; IPC → policy → executor (ADR-009) |
| P3 Policy Before Execution | PASS | deterministic evaluator; denial-by-default (§ 5, research § 8) |
| P4 Evidence Before Inference | PASS | `observations` immutable, distinct from inferred/desired (data-model § 2) |
| P5 Kernel-Native | PASS (forward-looking) | `SO_PEERCRED`; native discovery deferred but architected (ADR-007/018) |
| P6 Infra-Agnostic Core | PASS | no K8s/NATS/eBPF prerequisite (FR-006) |
| P7 Plug-and-Play | PASS | manifest + registry, no core edits (ADR-004/016) |
| P8 MCP Core + Ecosystem | PASS | MCP Core packaging boundary; MCP not the security boundary (ADR-003/005) |
| P9 Deterministic Control Plane | PASS | domain/policy/executor testable without LLM (ADR-010) |
| P10 Auditable Autonomy | PASS | `audit_events` append-only + correlation IDs |
| P11 Graceful Degradation | PASS | NATS/OTel/MCP optional; degraded states reported |
| P12 Storage Independence | PASS | `DomainRepository` trait over LanceDB |
| P13 Testability | PASS | no LLM/K8s/network in test prerequisites (§ 9) |
| P14 Least Privilege | PASS | empty capability set, hardened unit |
| P15 Stable Core Contracts | PASS | versioned IPC/events/capabilities |
| P16 SpecKit/ADR Separation | PASS | ADRs govern; no silent redefinition (§ 12) |
| P17 No Generic Agent Framework | PASS | agent runtime out of scope (ADR-013) |
| P18 Autonomous Control Loop | PASS (scaffold) | loop stages preserved as concepts, not yet implemented |

No violations requiring exceptions. Gates re-evaluated after Phase 1 design: no new
constitution conflicts introduced by the data model or contracts.
