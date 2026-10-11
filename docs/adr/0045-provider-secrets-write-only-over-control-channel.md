# ADR-0045: Provider Secrets Are Write-Only over the Control Channel

- **Status:** Accepted
- **Date:** 2026-10-11

## Context

The AI provider's API token lives in a local 0600 secret store; setting it
required shell access to the host. The managed-configuration channel
already delivers `provider` settings with a credential extraction path
(`split_provider_content` → `store_provider_credential`, 0600), and the
dashboard's instance page deploys configuration versions (the autonomy
control). The operator story (spec 015) asks for the token to be settable
from the dashboard — which means a secret crosses the control channel.

## Decision

1. **Write-only, once.** The API token may be delivered inside a `provider`
   configuration deployment; the daemon stores it in the local secret store
   (0600) at apply time and forgets it from every response surface. It is
   never echoed, never logged, never stored cloud-side beyond the audit's
   boolean "token updated", never returned by config-state, IPC, or any
   API read.
2. **Presence, not value, is state.** Every read surface (config-state,
   dashboard, audit) reports token presence (set/unset) — the operator
   verifies the token landed without the system ever having to hand it
   back.
3. **MCP topology is configuration, not execution.** The `[mcp]` servers
   document is deliverable as a managed configuration (the runbook
   precedent: structured content validated locally, applied live). Declaring
   a server is not executing it: the tools still enter the registry as
   HighRisk-by-default capabilities behind the full ladder (ADR-0044).
4. **Precedence stays managed-over-file.** A delivered `mcp` document
   supersedes file-declared servers on name conflict; file-only servers
   keep working. Restart re-applies both, cloud first.

## Alternatives considered

- **Local-only tokens (status quo)** — rejected: the operator's most common
  credential operation stays an SSH round-trip, and the dashboard story is
  the product.
- **Credential refs pointing at operator-seeded local files** — kept for
  the air-gapped case, but it does not serve the dashboard story; the
  write-only delivery is the cloud-era complement, not a replacement.
- **Vault/external secret manager integration** — deferred: one more
  dependency and a deployment prerequisite; revisit when a fleet operator
  asks for it.

## Consequences

- The control channel now carries a secret exactly once per update; its TLS
  and authentication are the boundary — the same trust that already
  delivers autonomy ceilings and runbook promotion decisions.
- Every read surface must be audited for the token's absence (spec 015
  FR-003/FR-004); a regression here is a security defect, not a bug.
- Config-state grows a presence indicator; nothing else in the protocol
  changes.
