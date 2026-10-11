# ADR-0044: External MCP Tools Are Capabilities — Unknown Provenance, HighRisk by Default

- **Status:** Accepted
- **Date:** 2026-10-11

## Context

The capability registry is closed to everything except compiled-in Rust
capabilities — the wall the MCP protocol exists to remove. The domain's
`McpRuntime` trait (ADR-005) reserved the seam and named `rmcp` as the
future implementation. The MCP track's first phase is inbound: configured
external MCP servers' tools become capabilities. The constitutional
question is sharp here, because an external tool is the first capability
whose provenance Argus does not control.

## Decision

1. **An MCP tool is a capability of unknown provenance.** It enters the
   registry like any capability — typed descriptor, input schema (the
   tool's own JSON Schema, so spec 010's construction-time validation
   applies unchanged) — with risk **HighRisk by default**: unknown
   provenance never auto-executes at any autonomy level. The operator may
   lower a specific tool's risk in config (`risk = "controlled"`), which is
   the operator writing law, not the server declaring trust.
2. **Deterministic namespacing, no negotiation.** Capability id =
   `mcp.<server>.<tool>`. Nothing about the server's self-description
   (annotations, metadata, advertised risk) changes authority — trust is
   the operator's config, never the server's self-report.
3. **The boundary is the boundary.** Tool calls route through policy →
   escalation → approval → budget → ledger exactly like native
   capabilities; results return as bounded execution evidence. No MCP
   message bypasses the boundary, and the provider never sees tool output
   except through the ordinary evidence paths.
4. **Fail-closed isolation.** Each server connects/reconnects
   independently with backoff; a dead server's tools fail as typed errors
   and stop being offered; a crashed server cannot crash the daemon or
   widen authority. The runtime lives in a new `argus-mcp` crate behind
   the `McpRuntime` trait (the domain stays implementation-free).
5. **Config-driven existence.** No `[mcp]` servers → no runtime, no
   capabilities, byte-identical behavior. The operator's config is the
   only way anything exists.

## Alternatives considered

- **Controlled as the default risk** — rejected: "the server says it's a
  read tool" is the server's claim; unknown provenance earns HighRisk, and
  the operator lowers it per tool after reading what the tool does.
- **Trusting MCP tool annotations** (readOnlyHint, destructiveHint) —
  rejected for authority: hints inform, config decides. A future release
  may surface hints in the operator UI.
- **Argus-as-MCP-server first** — deferred: outbound exposure is valuable
  but has no trust gradient to solve; inbound tools exercise the whole
  boundary first.
- **A generic key-value plugin bus instead of MCP** — rejected: MCP is the
  ecosystem's protocol, the domain trait already named it, and a bespoke
  bus would be a second protocol to maintain.

## Consequences

- New crate `argus-mcp` (rmcp-backed); the daemon gains an `[mcp]` config
  section and startup wiring behind the existing trait seam.
- External tools appear in `argus` capabilities, the cloud's published
  surface, and the ledger like native ones — one audit story.
- The second phase (Argus as MCP server) reuses the same registry and
  policy machinery in reverse.
- A runbook authored for an MCP tool's signature is indistinguishable from
  one authored for a native capability — the procedure-plan machinery
  (spec 010) applies unchanged.
