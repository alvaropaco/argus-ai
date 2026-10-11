# Feature Specification: ARGUS MCP Integrations — External Tools as Capabilities

**Status:** Draft
**Spec ID:** 014
**Created:** 2026-10-11

> Opens the MCP track: external MCP servers become typed capability sources —
> the first expansion track named in spec 013's conclusion. The
> `McpRuntime` trait (domain, ADR-005) named `rmcp` as the future
> implementation; this is that phase. Decision record: ADR-0044 (an MCP tool
> is a capability of unknown provenance — HighRisk by default, always across
> the boundary, never a special path).

## 1. Problem Statement

The capability registry is closed to everything except the compiled-in
bootstrap set. The ecosystem's tools — Prometheus queries, database
inspections, the long tail of MCP servers — can only reach Argus by
becoming compiled Rust, which is exactly the wall the MCP protocol exists
to remove. Meanwhile the organism is perception-limited to what the kernel
exposes: an external data source (metrics, logs, databases) cannot become
evidence or an action without a release.

## 2. Objective

The daemon connects to configured MCP servers (stdio subprocess or HTTP),
discovers their tools, and registers each as a typed capability —
namespaced, HighRisk by default, crossing the ordinary policy → escalation
→ approval → budget → ledger boundary like every capability. When an
external tool is called, the result returns as execution evidence.
Everything about the integration is deny-by-default and config-driven: no
server, no tool, no call exists unless the operator declared it.

## 3. Functional Requirements

**FR-001 (Configuration)** A `[mcp]` config section MUST declare servers:
name, transport (`stdio` with command+args, or `http` with url), `enabled`
(default false), an optional per-tool risk override
(`[mcp.servers.<name>.tools.<tool>] risk = "controlled"`), and an optional
tool allowlist (`tools = [...]` — absent means all). Names must be unique
and schema-valid (`[a-z0-9.-]`). Unlisted or disabled servers contribute
nothing.

**FR-002 (Discovery & registration)** At startup (and on server
(re)connect), the daemon MUST discover each enabled server's tools via the
MCP protocol and register each as a capability with the deterministic id
`mcp.<server>.<tool>`, the tool's JSON Schema as the input schema, risk
class per the override or **HighRisk** by default, blast radius `Host`,
reversibility `None`, and approval required. Capabilities publish to the
cloud like any registered capability (the existing host-changing rule
denies them over the cloud channel).

**FR-003 (Execution)** Calling an `mcp.*` capability MUST route through the
ordinary boundary (policy → escalation → approval → budget → ledger) to the
MCP runtime, which invokes the tool with the plan's arguments and returns
the tool's output as execution evidence (bounded). The call is fail-closed:
a crashed, timed-out, or disconnected server is a typed error — the
procedure terminates Failed per the spec-010 rules, and the server's
capabilities stop being offered until it returns (reconnect with backoff;
a re-registered tool keeps its ladder/audit state by capability id).

**FR-004 (Isolation & lifecycle)** Each server connects and reconnects
independently (backoff, never blocking the loop or other servers); a
server's tools are removed from the registry when its connection closes for
a configured grace (default: keep registered but failing — the runbook
story prefers visible failure); all lifecycle transitions are logged and
traced.

**FR-005 (The boundary is the boundary)** An MCP tool is never authority:
its output is execution evidence, its identity is a capability id, and
nothing about the MCP protocol bypasses policy, budget, approval, or the
ledger. The provider never sees tool outputs except as evidence through the
ordinary paths.

## 4. Non-Functional Requirements

- The MCP client lives in a new `argus-mcp` crate behind the existing
  `McpRuntime` trait seam; the domain and daemon depend only on traits.
- A crashed or malicious server cannot crash the daemon, exhaust memory
  (bounded evidence), or widen authority (HighRisk default; risk overrides
  are config the operator wrote).
- Startup with zero `[mcp]` servers behaves byte-identically to today (the
  lean build's default).
- Timeouts bound every tool call (default 30s, config per server).

## 5. Acceptance Criteria

- [ ] AC-001 — A configured stdio MCP server's tools register as
      `mcp.<server>.<tool>` capabilities with the tool's schema, HighRisk
      default, visible in `argus` capabilities and the cloud's published
      surface.
- [ ] AC-002 — A procedure plan (or CLI-approved invocation) calling the
      tool crosses the ordinary boundary and the tool's result lands as
      execution evidence; a per-tool risk override changes only the
      autonomy matrix's treatment.
- [ ] AC-003 — A crashed server's tools fail typed (Failed), the daemon
      stays healthy, and the tools return after the server restarts.
- [ ] AC-004 — A disabled or removed server's capabilities are unregistered
      and its calls refused as unsupported.
- [ ] AC-005 — A tool call consumes the blast-radius budget per its risk
      class and is audited in the ledger like any capability.
- [ ] AC-006 — Zero `[mcp]` servers: startup, capabilities, and the cloud
      surface are byte-identical to today.

## 6. Out of Scope

- Argus exposing ITSELF as an MCP server (the outbound direction — the
  natural second phase).
- MCP resources/prompts as evidence sources (tools first).
- HTTP/SSE transports beyond the streamable HTTP one.
- Per-tool budget overrides.
- Trusting a server's declared annotations (trust is the operator's config,
  never the server's self-report).
