# ADR-0024: Installation Identity — Asymmetric Ed25519

- **Status:** Accepted
- **Date:** 2026-09-23
- **Supersedes:** ADR-0023 (Installation Identity — Credential-Based v1 and Its Production Limitation)
- **Superseded by:** nothing.

## Context

ADR-0023 established a credential-based v1 identity that supplies a deliberately
non-decodable placeholder for the protocol's `public_key` / `challenge_signature`
fields. Its §3 and §4 recorded the hard consequence and the follow-up: the
installation cannot enroll against a **production** cloud, and closing the gap
requires a real key pair, a private key stored at mode `0600`, a signature over
the cloud's challenge on every connection, and a cloud verifier reconciled for
the signature algorithm.

That follow-up is now implemented. This ADR records the concrete identity v2 so
the wire contract is fixed in one place for both repositories.

## Decision

### 1. Algorithm is Ed25519 (RFC 8032), PureEdDSA

No prehash, no digest parameter. The cloud MUST NOT use
`createVerify("sha256")`, which is the RSA/ECDSA shape and does not verify
Ed25519.

### 2. The signed message is the challenge string's UTF-8 bytes

The cloud sends a per-connection `challenge`
(`randomBytes(24).toString("base64url")`) in `handshake.hello`. The installation
signs the **UTF-8 bytes of that exact string**, and the cloud verifies the same
bytes. The challenge is per connection, so a signature is computed on every
`pairing.redeem` and every `handshake.authenticate`.

### 3. Key material and encodings (normative)

| Item | Format | Size | Wire encoding |
|------|--------|------|---------------|
| Private seed | raw 32 bytes | 32 B | never transmitted; secret store only |
| Public key | DER `SubjectPublicKeyInfo` (RFC 8410) | 44 B | base64url **no padding** (59 chars) |
| Challenge signature | raw Ed25519 signature | 64 B | base64url **no padding** (86 chars) |

The DER SPKI for Ed25519 is the fixed 12-byte prefix
`30 2a 30 05 06 03 2b 65 70 03 21 00` followed by the raw 32-byte public key.
Both encodings sit inside the protocol bounds (public key 32–2000, signature
16–2000).

base64url no-pad (RFC 4648 §5) is used on both sides.

### 4. Cloud verification is strict

The cloud replaces the tolerant verifier with:

```ts
const key = createPublicKey({
  key: Buffer.from(publicKey, "base64url"),
  format: "der",
  type: "spki",
});
if (key.asymmetricKeyType !== "ed25519") return false;
return verify(null, Buffer.from(challenge, "utf8"), key,
              Buffer.from(signature, "base64url"));
```

The `NODE_ENV !== "production"` tolerant branch is **removed**. A key that does
not decode, a non-Ed25519 key, and a signature that does not verify all return
`false` in every environment. The placeholder is gone, so nothing depends on the
tolerant branch.

### 5. Key storage and lifecycle

- The 32-byte seed is generated with an OS CSPRNG when `argus cloud enroll`
  runs, before `pairing.redeem` is sent.
- It is persisted through the existing daemon secret store
  (`CloudSecretStore`, atomic write, mode `0600`) as a new file
  `cloud-identity-key`, base64url no-pad.
- The same key is reused for every reconnect and across restarts. Reconnect
  presents the same public key that enrollment stored, byte-for-byte; otherwise
  the cloud refuses with `KEY_CHANGED`.
- `argus cloud forget` removes the seed together with the session credential.
- The private seed never leaves the daemon: not over IPC, not in logs (the
  existing redacting `Secret` type is used), not in `argus.toml`.
- Key rotation and regeneration are **out of scope**. Regenerating identity
  requires re-enrollment (a deliberate, visible operator act).

### 6. `EnrolledIdentity` carries the public key

`placeholder_public_key` becomes `public_key` (the base64url DER string),
populated from the signing key at enrollment and re-derived from the stored seed
on reconnect. The private seed is never carried by `EnrolledIdentity`, which
stays secret-free and safe to render in diagnostics.

## Consequences

### Positive

- Production enrollment and reconnection work with real cryptographic proof.
- The trust model no longer relies on a non-secret placeholder, so possession of
  the session credential alone is insufficient: the peer must also hold the
  private seed.
- The verifier is strict in every environment; there is no production-only
  behavioural divergence to reason about.

### Negative

- The daemon now owns key material, so the secret store must be available and
  writable at `0600`; a missing or corrupt seed blocks cloud connectivity until
  re-enrollment.
- A lost seed cannot be recovered; the operator must `forget` and re-enroll
  (which the `KEY_CHANGED` denial makes explicit).
- One more dependency (`ed25519-dalek`) enters the Rust workspace.

## Implementation Principles

1. Never transmit, log, or IPC the private seed.
2. Verify in constant structure: decode, check key type, verify; any failure is
   `false`.
3. Do not reintroduce a tolerant branch keyed on `NODE_ENV`.
4. Keep the encodings exactly as specified in §3; a mismatch fails in every
   environment and is worse than a visible placeholder.
5. Regenerating a key without re-enrollment is forbidden; surface `KEY_CHANGED`
   as an actionable operator message.

## Cross-Language Test Vector

Both repositories assert this vector, which pins the encoding and the signature
contract across languages:

```
seed_hex        = 0707070707070707070707070707070707070707070707070707070707070707
challenge       = test-challenge-nonce
public_key_b64u = MCowBQYDK2VwAyEA6kpsY-KcUgq-9VB7Ey7F-ZVHdq6-vnuSQh7qaRRG0iw
signature_b64u  = ub_IfDKsfEELPjLDfWLy9xzCJIrbJzD3jZN2KZEbXT8kwS079k76yWLqrJWFPd2vQSEdgf3-tbskRdB6lvFoBQ
```

Verification of `signature_b64u` over `challenge` with `public_key_b64u` MUST
succeed, and verification over any other message MUST fail.

## Reference

- Superseded: `docs/adr/0023-installation-identity-credential-v1.md`
- `crates/argus-cloud/src/client/pairing.rs`, `.../client/handshake.rs`,
  `.../state.rs`
- `crates/argus-daemon/src/cloud.rs` (secret store, enrollment, supervision)
- `app-platform-argus-ai/apps/gateway/src/connection/signature.ts`,
  `.../connection/auth.ts`, `packages/core/src/pairing.ts`
- Constitution Principles 2, 15, 16
