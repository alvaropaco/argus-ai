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