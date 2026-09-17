# ADR-005: MCP Runtime

- **Status:** Accepted
- **Date:** 2026-09-16

## Decision

ARGUS will use a combination of:

1. A native Rust MCP implementation based on the MCP Rust ecosystem (`rmcp`) for ARGUS-owned MCP capabilities.
2. External MCP servers as first-class integrations.

ARGUS is therefore an MCP host/runtime and ecosystem, not limited to MCP servers implemented internally.

## Consequences

- Native capabilities can share ARGUS lifecycle, security, configuration, and observability.
- Community MCPs can be reused without reimplementation.
- External MCPs must pass capability, provenance, licensing, permission, and security checks before inclusion in MCP Core.
- MCP processes must remain subject to ARGUS lifecycle and policy controls where practical.