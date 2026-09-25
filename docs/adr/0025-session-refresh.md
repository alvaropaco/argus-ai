# ADR-0025: Session Refresh — Striking the Balance Between Expiry and Availability

- **Status:** Accepted
- **Date:** 2026-09-25
- **Supersedes:** nothing. **Superseded by:** nothing.

## Context

An installation proves its identity with the session credential issued at
enrollment. `redeemPairingCode` issued it with a hardcoded one-hour lifetime
(`60 * 60 * 1000`), ignoring `SESSION_TOKEN_TTL_SECONDS`, and the client had no
way to renew it. A daemon that stayed connected therefore never noticed the
expiry — the socket was already authenticated — but any restart after an hour
found an expired credential and was refused with `UNAUTHENTICATED`, requiring
operator re-enrollment.

This was observed in production: a daemon that had been connected for two days
was refused on restart, because its stored token had expired 53 hours earlier.

## Decision

### 1. The lifetime is configurable, not hardcoded

`sessionTokenTtlSeconds()` resolves `SESSION_TOKEN_TTL_SECONDS` at call time
(falling back to one hour). The value is read at call time so a runtime secret
provider that populates the environment after import is still honoured.

### 2. The token is refreshed on every connection

`handshake.ready` is followed by a `session.rotate` carrying a freshly signed
session token. The daemon already implemented `session.rotate` (storing the
rotated credential atomically), so this required no agent-side change. The
effect is a sliding session: while an installation reconnects at least once per
lifetime, its credential never expires.

### 3. The lifetime bounds offline time, and is set generously in production

Because a token that has already expired cannot be refreshed — the refresh
happens inside an authenticated connection — the lifetime must exceed plausible
downtime. Production sets `SESSION_TOKEN_TTL_SECONDS` to 30 days. This is
deliberate: the credential is stored at mode `0600` on the installation and the
installation additionally holds an Ed25519 private key (ADR-0024), so a long,
continuously-refreshed token is an availability choice rather than a weakening
of the trust boundary.

## Consequences

### Positive

- A long-running daemon no longer needs re-enrollment after a restart.
- The lifetime is answerable from configuration; no magic number remains.
- The refresh reuses the existing `session.rotate` message and daemon handling.

### Negative

- A 30-day credential is a longer-lived bearer token than a short one. Possession
  of the token file is still sufficient to authenticate until the instance is
  revoked or the credential is rotated; the Ed25519 key narrows but does not
  remove that window.

## Follow-up

The asymmetric identity introduced by ADR-0024 makes the session credential
arguably redundant for authentication: the installation already proves
possession of a key on every connection. A future ADR SHOULD evaluate making the
key the sole authenticator and retiring the expiring bearer token entirely,
which would remove this availability/expiry trade-off rather than tune it.

## Reference

- Superseded context: ADR-0024 (asymmetric identity)
- `app-platform-argus-ai/packages/core/src/crypto.ts` (`sessionTokenTtlSeconds`)
- `app-platform-argus-ai/packages/core/src/pairing.ts`
- `app-platform-argus-ai/apps/gateway/src/connection/handshake.ts`
- `argus-ai/crates/argus-daemon/src/cloud.rs` (`rotate_session_credential`)
