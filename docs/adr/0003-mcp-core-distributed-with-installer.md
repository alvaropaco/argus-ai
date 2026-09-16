# ADR-003: MCP Core Distributed with the Installer

- **Status:** Accepted
- **Date:** 2026-09-16

## Context

ARGUS AI must be useful immediately after installation and should be able to discover and operate common Linux/Unix infrastructure without requiring users to manually install and configure a collection of MCP servers.

At the same time, MCP provides an extensible integration boundary for infrastructure capabilities and community-maintained tools.

## Decision

ARGUS AI will ship a **MCP Core** as part of the standard installation package.

The MCP Core will contain the principal integrations required for the initial Linux/Unix operating environment. These integrations should be selected from mature community projects whenever practical rather than reimplemented by ARGUS.

Initial candidates include:

- `allenbijo/mcp-sysinfo` — system information and host discovery.
- `QuantGeekDev/docker-mcp` — Docker integration.

Community MCPs may be vendored, packaged, wrapped, adapted, or otherwise integrated depending on licensing, security, maintenance, runtime, and compatibility requirements.

The installer must provide a consistent ARGUS configuration and lifecycle for the bundled MCP Core components.

## Consequences

### Positive

- ARGUS works out of the box after installation.
- Reduces duplicated community effort.
- Accelerates support for infrastructure platforms.
- Provides a standard MCP capability set for ARGUS agents.

### Negative

- Bundled third-party projects introduce dependency and supply-chain risk.
- Licensing and compatibility must be reviewed before distribution.
- ARGUS must monitor upstream projects and manage version pinning.

## Security Requirements

Every bundled MCP must undergo dependency, licensing, provenance, permissions, and execution-surface review before becoming part of the MCP Core.

MCPs must not automatically receive privileges beyond those required for their declared capabilities.