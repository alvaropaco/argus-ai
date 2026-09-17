# ADR-007: Host Discovery

- **Status:** Accepted
- **Date:** 2026-09-16

## Decision

ARGUS will use `mcp-sysinfo` as the primary MCP interface for host/system information in MCP Core.

ARGUS will additionally implement native Rust discovery for information that is critical to the runtime, security boundary, health model, or autonomous decision-making.

Native discovery may use mature Rust/Linux interfaces such as `sysinfo`, `/proc`, `/sys`, `nix`, netlink, and other appropriate OS APIs.

## Principle

MCP provides an extensible capability interface; native discovery provides trusted, low-level primitives for ARGUS itself.

## Candidate Community Component

- `allenbijo/mcp-sysinfo`: https://github.com/allenbijo/mcp-sysinfo

The component remains subject to supply-chain, license, maintenance, and security review before each bundled release.