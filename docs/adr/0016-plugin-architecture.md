# ADR-016: Plug-and-Play Plugin Architecture

- **Status:** Accepted
- **Date:** 2026-09-16

## Decision

ARGUS will use a plug-and-play extension architecture. Capabilities must be installable, removable, discovered, configured, versioned, authorized, health-checked, and upgraded independently of the ARGUS core where practical.

Supported extension categories may include:

- MCP servers/tools
- infrastructure adapters
- discovery providers
- agents and skills
- runbooks
- policy providers
- observability integrations
- AI model providers
- storage providers
- cloud/platform integrations

Extensions will declare metadata through a TOML manifest. The manifest will include, as applicable, name, version, type, capabilities, dependencies, required permissions, configuration schema, compatibility, health information, and lifecycle metadata.

Example:

```toml
[plugin]
name = "docker"
version = "1.0.0"
type = "mcp"

[capabilities]
tools = [
  "docker.list_containers",
  "docker.inspect_container",
  "docker.restart_container"
]

[permissions]
docker_socket = true
network = false
filesystem = false
```

## Principle

The ARGUS core provides extension contracts, lifecycle, security, and governance. Platform-specific knowledge belongs in plugins/adapters whenever practical.

## Future Direction

The architecture should support a future ARGUS extension registry or marketplace without requiring a redesign of the core plugin model.

## Amendment (2026-09-21): Capability Declaration and Cloud-Facing Vocabulary Mapping

**Related:** `specs/001-argus-cloud-sync` (feature `001-argus-cloud-sync`), ADR-0020, ADR-0021.

The capability declaration is the surface this ADR already owns, so the following
extensions and translation rules are recorded here.

### 1. `required_os_privileges` joins the capability declaration

A capability that needs elevated privilege MUST declare the exact Linux
capabilities, namespaces, `seccomp` profile, and `Landlock` rules it requires,
alongside its risk class and reversibility. A capability that needs privilege but
declares none is rejected at registration. See ADR-0021 for the execution path and
the sandbox rule.

### 2. The cloud-facing descriptor is a translation, not the local descriptor

The local `CapabilityDescriptor` (`argus-domain::capability`) and the cloud's
descriptor are different shapes, and neither can change without breaking a shipped
consumer: the local one is persisted and exposed over IPC, the cloud one is
validated by the cloud's own schema. The mapping is therefore permanent and
explicit, never coerced silently.

| Field | Local | Cloud | Rule |
|---|---|---|---|
| `id`, `provider`, `operation` | present | present | direct |
| `risk_class` | `snake_case` (`low_risk`) | PascalCase (`LowRisk`) | **case normalisation required** |
| `reversibility` | `none` \| `reversible` \| `partially_reversible` | `reversible` \| `irreversible` \| `n/a` | see §3 |
| `version` | present | absent | dropped on publish; the cloud versions the schema at publication level |
| `input_schema`, `output_schema` | present | optional | direct |
| `description` | absent | optional | not invented locally |
| `requires_approval` | absent | optional | derived from the authorization model, never from the descriptor |

### 3. `none` maps to `irreversible`, not `n/a`

| Local | Wire |
|---|---|
| `reversible` | `reversible` |
| `partially_reversible` | `reversible` |
| `none` | `irreversible` |

The cloud's `n/a` reads as "reversibility is not applicable". The local `none`
means "this operation cannot be undone" — the stronger, failure-safe reading.
Mapping `none` to `irreversible` keeps the operator warned even when the local
vocabulary is coarser; mapping it to `n/a` would hide a real risk. The conservative
direction is chosen deliberately and is asserted by test.

### 4. Health vocabulary mapping

| Local (`argus-domain::health::HealthState`) | Wire |
|---|---|
| `Ready` | `healthy` |
| `Degraded` | `degraded` (local `reason` carries the summary) |
| `NotReady` | `critical` |
| — | `unknown`, reserved for a never-observed instance; never fabricated |

`NotReady` maps to `critical` rather than `unknown`: the runtime cannot serve, and
`unknown` would understate a genuine failure. The cloud has no `not_ready` value.

### 5. Test obligation

Every rule in this amendment is covered by a mapping test asserting the exact
normalisation and the `none` → `irreversible` direction. A silent coercion at the
boundary is the failure mode this amendment exists to prevent.