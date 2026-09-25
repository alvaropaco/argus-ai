# ADR-0023: Installation Identity — Credential-Based v1 and Its Production Limitation

- **Status:** Superseded
- **Date:** 2026-09-21
- **Supersedes:** nothing. **Superseded by:** [ADR-0024](0024-installation-identity-asymmetric-ed25519.md).

> **Superseded.** The credential-based v1 identity and its placeholder key
> described below were replaced by the Ed25519 asymmetric identity in ADR-0024.
> The placeholder MUST NOT be reinstated; §2's "keep it non-decodable" rule is
> historical and no longer applies.

## Context

The Argus Cloud agent protocol reserves an asymmetric identity: the installation
supplies a `public_key` at enrollment and a `challenge_signature` at every
connection, and the cloud verifies the signature against the stored key.

The installation runtime has no asymmetric key material today, and introducing key
generation, storage, and rotation into a bootstrap runtime was judged out of scope
for this feature. The operator decision (Q3 in
`specs/001-argus-cloud-sync/spec.md`) was therefore to prove identity with the
cloud-issued session credential and supply a non-secret placeholder for the key
field.

That decision has a hard consequence discovered in the delivered cloud code, and it
must be recorded rather than discovered in production.

## Decision

### 1. v1 identity is credential-based

The installation proves its identity by presenting the session credential issued at
enrollment (`session_proof` in `handshake.authenticate`, `session_token` from
`pairing.granted`). It transmits only non-secret identifying material alongside it:
installation id, hostname, and agent version.

### 2. The placeholder key MUST remain non-decodable

The protocol requires `public_key` (32–2000 chars) and `challenge_signature`
(16–2000 chars) to be present. v1 supplies a deliberately **non-decodable**
placeholder for both.

This is load-bearing, not cosmetic. The cloud's verifier
(`app-platform-argus-ai/apps/gateway/src/connection/signature.ts`) behaves as
follows:

- if the key cannot be decoded as DER/SPKI base64url, it takes the tolerant branch
  and returns `process.env.NODE_ENV !== "production"`;
- if the key *is* decodable but the signature does not match, it returns `false`
  regardless of environment.

A "better" well-formed key would therefore make the installation fail in **every**
environment, while appearing more correct. The placeholder is documented here so a
future contributor does not improve it into a defect.

### 3. This satisfies a non-production cloud only

Verified against the delivered cloud code:

- `app-platform-argus-ai/packages/core/src/pairing.ts:182` — `redeemPairingCode`
  awaits the challenge verification and returns `{ ok: false, code: "INVALID" }`
  when it fails, so a placeholder key is refused at enrollment.
- `app-platform-argus-ai/apps/gateway/src/connection/auth.ts` — reconnection
  returns `UNAUTHENTICATED` when the challenge signature does not verify.

Consequently **v1 enrolls and authenticates successfully against a non-production
Argus Cloud and is refused by a production one.** This is the recorded scope of the
feature and is why SC-020 scopes conformance to a non-production build and
explicitly excludes production enrollment from this feature's success criteria.

The limitation MUST be stated in the product documentation, not left to be
discovered in production.

### 4. Asymmetric identity is a prerequisite for production, not optional polish

Closing this requires a follow-up feature that: generates an installation key pair;
stores the private key in the secret store at mode `0600`; signs the cloud's
challenge on every connection; and aligns the signature algorithm with the cloud's
verifier, whose current implementation does not correctly handle Ed25519 via
`createVerify("sha256", …)` and must be reconciled on the cloud side.

Until that lands, any claim that this feature is production-ready for cloud
enrollment is false.

### 5. Rotation and revocation are still fully honoured

Credential-based identity does not weaken the trust lifecycle: `session.rotate`
replaces the credential, revocation terminates the connection permanently, and
suspension parks it. Those semantics are unchanged and are specified in ADR-0020 and
`contracts/privileged-execution.md`.

## Consequences

### Positive

- The installation needs no key management, so the bootstrap runtime stays small.
- The protocol's key fields are satisfied without inventing a key lifecycle.
- The limitation is explicit and testable at the boundary (SC-020).

### Negative

- **The installation cannot authenticate to a production cloud.** This is the
  dominant limitation of the feature and must be communicated to operators.
- The identity is trust-on-first-use bound to a non-secret placeholder, so an
  attacker who obtains the session credential can impersonate the installation until
  the credential is rotated or the instance revoked.
- A second feature and a cloud-side change are required before production use.

## Implementation Principles

1. Never present the placeholder as a verified cryptographic identity.
2. Never "fix" the placeholder into a well-formed key.
3. Surface the limitation in local diagnostics and in product documentation.
4. Keep the placeholder non-secret; the session credential is the only secret.
5. Do not claim production readiness for cloud enrollment until ADR-0023 is
   superseded.

## Reference

- `specs/001-argus-cloud-sync/spec.md` FR-005, FR-009; SC-020
- `specs/001-argus-cloud-sync/contracts/agent-protocol-conformance.md` §6
- `specs/001-argus-cloud-sync/research.md` R16
- `app-platform-argus-ai/apps/gateway/src/connection/signature.ts`
- `app-platform-argus-ai/packages/core/src/pairing.ts:182`
- `app-platform-argus-ai/apps/gateway/src/connection/auth.ts`
- Constitution Principles 2, 15, 16
