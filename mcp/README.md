# ARGUS MCP Core

The **MCP Core** is distributed with the ARGUS installer (ADR-003). MCP is an
integration protocol, **not** the ARGUS security boundary (Principle 8); the
policy engine and executor remain authoritative.

## Layout

```text
mcp/
├── core/        # Bundled MCP servers/tools shipped with the installer
└── community/   # Community MCPs packaged as plugins (via the plugin manifest)
```

## Packaging boundary

- `mcp/core/` contains the curated set of MCP integrations required for initial
  Linux/Unix operation (e.g. `mcp-sysinfo`, `docker-mcp` per ADR-003/ADR-007).
- `mcp/community/` contains additional, opt-in community MCPs installed through
  the plugin model (ADR-016).
- Every bundled MCP MUST declare a TOML manifest (`PluginManifest`, see
  `crates/argus-domain/src/plugin.rs`) and undergo dependency, licensing,
  provenance, permission, and execution-surface review (ADR-003).

## Runtime

Native MCP capabilities share the ARGUS lifecycle, security, configuration, and
observability. The integration surface is the `McpRuntime` trait
(`crates/argus-domain/src/mcp.rs`); the concrete `rmcp`-backed implementation is
added in a later phase (ADR-005).

The manifest is descriptive, not an authorization grant. Capabilities declared
by a plugin still pass through `PolicyEvaluator` → `Executor` before execution.
