# ADR-0026: The Installation Key Is the Sole Authenticator

- **Status:** Accepted
- **Date:** 2026-09-26
- **Supersedes:** ADR-0025 (in part: §2, the sliding refresh, and §3, the lifetime
  trade-off). ADR-0025 §1 (a configurable lifetime) still applies to the
  credential issued at enrollment.
- **Superseded by:** nothing.

## Context

ADR-0024 gave the installation a real Ed25519 key pair and made the cloud verify
a per-connection challenge signature against the key stored at enrollment.
Authentication nevertheless still required the cloud-issued session credential:
`authenticateInstance` rejected a connection whose `session_proof` was missing,
invalid, or expired, *before* it looked at the key.

That made a durable, cryptographic identity hostage to a bearer token with a
finite lifetime. ADR-0025 mitigated the symptom — a configurable lifetime plus a
`session.rotate` on every connection — but the trade-off remained: an
installation offline longer than the lifetime still could not reconnect, and the
mitigation only worked while the token stayed valid.

## Decision

### 1. The challenge signature is the authority

`authenticateInstance` authenticates on `instance_id` plus the Ed25519
`challenge_signature`, verified against the public key stored in
`instance_credentials`. The instance and its credential must still be active:

- a revoked or suspended instance is refused (`REVOKED`);
- an instance with no active credential, or an empty public key, is refused
  (`UNAUTHENTICATED`);
- a signature that does not verify is refused (`UNAUTHENTICATED`).

The session credential no longer participates. `session_proof` becomes optional
in `handshake.authenticate` and is accepted but ignored, so installations and
clouds built before this ADR keep interoperating.

### 2. The credential epoch check is removed

`payload.epoch + 1000 < credential.issuedAt` existed to reject bearer tokens
issued before a credential rotation. With no token in the decision there is no
epoch to compare; revocation and credential status carry those semantics.

### 3. The sliding refresh is retired

`session.rotate` is no longer emitted after a successful authentication. It
existed only to keep a load-bearing token fresh, and the token is no longer
load-bearing. The daemon still handles an inbound `session.rotate`, so a
cloud that has not yet adopted this ADR can still rotate.

### 4. The enrollment credential is retained for compatibility

`pairing.granted` still carries a `session_token`, and the installation still
stores it. It is no longer required to reconnect; removing it from the protocol
would be a breaking change with no security benefit.

## Consequences

### Positive

- Availability and expiry are decoupled: an installation that was offline for any
  length of time reconnects with its key, and re-enrollment is needed only when
  the private seed is lost or the instance is revoked.
- Fewer moving parts: one proof of identity instead of two, and no refresh
  machinery to reason about.
- The trust boundary is exactly the key boundary documented by ADR-0024.

### Negative

- Possession of the private seed is now solely sufficient to impersonate the
  installation. This was already true of the key; what changes is that a leaked
  session credential is no longer a second, independently revocable path. The
  seed's `0600` storage and the installation's compromise posture carry the full
  weight, which is the intended model.
- An installation whose credential row is deleted can no longer connect by any
  means; that is a deliberate operator action (revocation), not an accident.

## Reference

- Superseded in part: `docs/adr/0025-session-refresh.md`
- Key identity: `docs/adr/0024-installation-identity-asymmetric-ed25519.md`
- `app-platform-argus-ai/apps/gateway/src/connection/auth.ts`
- `app-platform-argus-ai/packages/contracts/src/agent/messages.ts`
- `crates/argus-cloud/src/client/handshake.rs`,
  `crates/argus-daemon/src/cloud.rs`
