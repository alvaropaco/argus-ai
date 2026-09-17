# ARGUS AI — AI Development Guide

This repository is designed to be developed with AI coding agents as first-class contributors.

## Required reading before changes

AI agents MUST read, in this order:

1. `.specify/memory/constitution.md`
2. Relevant ADRs under `docs/adr/`
3. Relevant architecture documents under `docs/architecture/`
4. The feature specification under `specs/`, when one exists
5. Existing code and tests affected by the change

## Architectural invariants

Do not violate these invariants unless an explicit ADR is created or updated:

- Rust is the core implementation language.
- Ratatui is the TUI framework.
- ARGUS must work without Kubernetes.
- AI output is data, never authority.
- Privileged operations MUST pass through policy and the Rust executor boundary.
- The ARGUS core MUST NOT contain unnecessary infrastructure-specific implementation details.
- Infrastructure-specific capabilities SHOULD live in adapters/plugins.
- The MCP Core is distributed with ARGUS.
- External MCPs are extensions, not the security boundary.
- LanceDB is behind a repository abstraction.
- NATS is optional; the local event bus must work without it.
- OpenTelemetry instrumentation is part of the architecture.
- Evidence, inferred state, desired state, and historical state remain distinct concepts.

## Implementation rules

### Security

Never grant an AI agent unrestricted root shell access.

Never bypass `argusd`, the policy engine, or the typed executor for privileged operations.

Do not add broad permissions to plugins or MCPs without documenting why they are required.

Prefer typed capabilities such as:

- `host.service.restart`
- `host.process.signal`
- `host.filesystem.inspect`
- `container.restart`
- `k8s.deployment.rollback`

over arbitrary command strings.

### Linux-first design

For authoritative Linux information, prefer kernel interfaces and structured APIs over parsing human-oriented CLI output.

Consider, where applicable:

- `/proc`
- `/sys`
- cgroup v2
- namespaces
- Netlink
- pidfd
- PSI
- eBPF/BTF
- D-Bus/systemd

CLI tools can be wrappers/fallbacks when a native API is unavailable or when they provide a capability not worth reimplementing.

### Extensibility

New infrastructure integrations SHOULD be implemented as adapters or plugins.

Plugins MUST declare:

- name
- version
- API compatibility
- capabilities
- dependencies
- required permissions
- health check

Use the TOML plugin manifest defined by the architecture.

### Persistence

Do not import LanceDB types into the core domain model. Use repository abstractions.

### Events

Use domain events with transport-independent schemas. The default local bus must not require NATS.

### AI / LLM

Use the ARGUS model-provider abstraction. Do not make core domain logic depend directly on a specific model vendor.

LLM responses MUST be validated as structured domain data before entering planning or execution.

### Tests

Every new capability MUST include tests for:

- happy path;
- failure path;
- authorization/policy behavior;
- degraded dependencies where relevant;
- deterministic behavior independent of an LLM where possible.

## SpecKit workflow

For new work:

```text
Specify → Plan → Tasks → Implement → Verify
```

Use an existing specification under `specs/` where applicable.

If a feature introduces an architectural decision not covered by current ADRs, stop and create/update the appropriate ADR rather than silently changing the architecture.

## Commit / PR expectations

Keep changes focused and explain architectural impact.

Prefer small, reviewable commits.

Generated code is still subject to the same security and architecture requirements as handwritten code.
