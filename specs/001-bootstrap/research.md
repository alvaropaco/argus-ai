# Research: ARGUS Bootstrap Runtime

**Spec:** `specs/001-bootstrap/spec.md`
**Status:** Draft
**Date:** 2026-09-16

This document resolves every `NEEDS CLARIFICATION` item from the feature spec and
the open technical questions required to lock down the bootstrap design. Each entry
follows the format **Decision → Rationale → Alternatives considered**.

Decisions are bounded to the bootstrap scope. They do not commit ARGUS to a final
architecture for later features; anything that outlives the bootstrap is a candidate
for an ADR update (see `plan.md` § ADR Impact).

---

## 1. Unix socket authentication mechanism

**Decision:** Use Unix-domain-socket **filesystem permissions plus `SO_PEERCRED`
peer-credential checks** as the bootstrap authorization mechanism. Requests carry a
versioned envelope with an explicit `RequestContext` (client identity + correlation
ID), and the daemon validates the peer's UID/GID against the socket path's ownership
before dispatching.

**Rationale:**
- Matches the spec § 8 "Security and Policy" direction: *"The first implementation
  SHOULD use Unix socket filesystem permissions plus explicit request validation,
  with the architecture prepared for Cedar authorization."*
- `SO_PEERCRED` is a Linux-native, kernel-authoritative way to identify the peer
  process without trusting client-supplied identity strings (Principle 5 —
  kernel-native).
- No new secrets, no shared-key distribution, and no dependency on an external
  identity service for the single-host baseline.
- The IPC contract remains transport/identity-agnostic so a Cedar-backed model can
  replace peer-credential checks later without changing the envelope.

**Alternatives considered:**
- *Bearer token in a 0600 file*: adds key management and rotation for little
  bootstrap benefit; rejected for the initial implementation, kept as a future option
  for remote/fleet IPC.
- *TLS over the socket (mTLS)*: strong but overkill for a local, root-owned socket;
  deferred.
- *D-Bus/systemd activation*: couples the bootstrap to systemd; rejected because
  ARGUS must run without systemd (FR-006, Principle 11).

**References:** `man 7 unix` (`SO_PEERCRED`), spec § 8, ADR-009.

---

## 2. Initial Linux capability set for `argusd`

**Decision:** Bootstrap `argusd` runs with **no ambient capabilities**. The packaged
unit starts with `CapabilityBoundingSet=` (empty), `NoNewPrivileges=true`, and
read-only system protection, adding capabilities only as specific privileged
capabilities are introduced in later features.

**Rationale:**
- The bootstrap exposes only **read-only** capabilities (`host.status.read`,
  `argus.health.read`, `argus.config.read`, `argus.plugins.list`) that read `/proc`,
  `/sys` and local state without requiring any privilege (Principle 14 — least
  privilege; Principle 3 — policy before execution).
- Starting from an empty bounding set makes every future capability grant an
  explicit, reviewable decision.
- Aligns with the existing hardened unit (`deploy/debian/argusd.service`), which
  already sets `NoNewPrivileges`, `PrivateTmp`, `ProtectHome`, `ProtectSystem=strict`.

**Alternatives considered:**
- *Pre-grant `CAP_SYS_ADMIN`/`CAP_BPF`*: rejected; violates least-privilege and is
  not required for bootstrap.
- *Run as `root` and drop at runtime*: rejected; systemd hardening + bounding set is
  the cleaner, declarative control.

**Follow-up:** when native discovery or executor actions need privileges (later
specs), each capability MUST declare the exact capability/namespace/`seccomp`/`Landlock`
profile it requires, per ADR-016 and the constitution.

**References:** `deploy/debian/argusd.service`, ADR-009, ADR-016, Principle 14.

---

## 3. Secret backend for provider keys

**Decision:** Introduce an `argus-secrets` **trait boundary** with a single bootstrap
implementation: a **filesystem backend** that reads a root-owned, mode-`0600`
directory (`/etc/argus/secrets/`). The bootstrap does **not** persist provider keys
into LanceDB.

**Rationale:**
- The bootstrap has no live provider configuration yet (the AI/planning runtime is
  stubbed — spec § 4). The secret backend only needs to establish the *boundary* so
  keys never leak into config, logs, or the state store.
- A filesystem backend avoids introducing a keyring dependency and keeps the
  single-host baseline self-contained (Principle 11).
- `argus.config.read` must therefore redact secret values by construction: config and
  secrets are separate sources, and only non-secret config is readable over IPC.

**Alternatives considered:**
- *Linux kernel keyring (`keyutils`)*: good long-term target but adds a dependency and
  persistence complexity; deferred to a later feature with an ADR.
- *Environment variables*: rejected; too easy to leak into logs/process listings.
- *LanceDB-encrypted columns*: rejected; secrets are not domain state and must not
  share the operational store.

**References:** ADR-011 (state separation), c4 § 30 (deployment layout), Principle 3.

---

## 4. Exact LanceDB schema for bootstrap state

**Decision:** Define the bootstrap operational state as a small set of **logical
tables** behind the `DomainRepository` trait (never exposed to the domain model).
**SQLite is the interim default backend** (`SqliteRepository`); the LanceDB adapter
is optional behind the `lancedb` cargo feature.

| Table | Purpose | Key fields |
|---|---|---|
| `environment` | single environment identity | `environment_id`, `name`, `created_at` |
| `health` | daemon health/readiness | `status`, `degraded_reasons`, `updated_at` |
| `plugins` | plugin registry metadata | `plugin_id`, `manifest`, `health`, `state` |
| `observations` | immutable evidence | `observation_id`, `source`, `subject`, `attribute`, `value`, `provenance`, `timestamp` |
| `audit_events` | append-only audit trail | `event_id`, `correlation_id`, `action`, `decision`, `actor`, `timestamp` |

**Rationale:**
- Covers exactly what the bootstrap needs per spec § 5 of `plan.md`: environment
  identity, runtime status, plugin registry metadata, initial observations, and audit
  events.
- Keeps **evidence (`observations`) immutable and distinct** from **inferred/desired
  state** (Principle 4), and keeps **audit append-safe** (ADR-011).
- The domain model depends only on the `DomainRepository` trait; backend types never
  cross into `argus-domain` (Principle 12).
- SQLite is chosen as the interim backend because the LanceDB Rust SDK (0.38.0) is not
  yet production-ready (upstream compile bug, very heavy datafusion dependency tree).
  This validates ADR-011's own "validate LanceDB suitability" caveat and requires an
  ADR-011 amendment (see `plan.md` § 12).

**Alternatives considered:**
- *SQLite as permanent store*: rejected by spec § 3 ("SQLite is NOT used") — SQLite is
  interim only, pending LanceDB maturity.
- *One generic `state` table with a JSON blob*: rejected; loses typed fields and
  provenance, undermining auditability (Principle 10).

**Follow-up:** LanceDB's exact Rust SDK version and table DDL remain an implementation
task when the `lancedb` feature is enabled.

**References:** spec § 3, ADR-011, c4 § 25, Principle 4/12.

---

## 5. Installer artifact signing mechanism

**Decision:** Bootstrap defines a **checksum + signature verification interface**
(`argus-install`) and implements **SHA-256 checksums now**, with **Sigstore/cosign
keyless signing** as the target for release automation. The `argus upgrade` command
verifies checksums/signatures before installing.

**Rationale:**
- The spec only requires the *architecture* for verified installation (FR-001,
  FR-006), not a specific tool.
- SHA-256 checksums are a zero-dependency, deterministic first step; Sigstore/cosign
  provides the durable signing story (identity-based, no long-lived private keys to
  protect in CI) and aligns with ADR-017 ("cryptographic signatures and/or
  checksums").
- Keeping the verification behind an interface (`VerifiedArtifact` → `InstallOutcome`)
  lets the signature backend evolve without touching the CLI/daemon.

**Alternatives considered:**
- *minisign/ed25519 (long-lived key)*: simpler but requires protecting a private key
  in CI; rejected in favor of keyless signing as target, checksums as floor.
- *GPG detached signatures*: rejected; key management and ecosystem friction.

**References:** ADR-017, spec § FR-001, deploy/apt pipeline, Principle 10.

---

## 6. IPC framing and serialization

**Decision:** Newline-delimited JSON (`serde_json`) frames over a Unix stream socket.
Each frame is a versioned envelope carrying a `correlation_id` and `protocol_version`.

**Rationale:**
- Human-debuggable, trivially testable, and versionable without a codegen step —
  appropriate for a small bootstrap API surface (`health.get`, `status.get`,
  `config.get`, `plugins.list`, `capabilities.list`).
- JSON is sufficient because IPC payloads are low-throughput control messages, not a
  hot data path.

**Alternatives considered:**
- *MessagePack / CBOR*: more compact and typed, but adds a dependency and loses
  trivial wire-level inspection; deferred.
- *gRPC/protobuf*: heavier; introduces codegen and an extra runtime dependency;
  rejected for the local IPC boundary.

**References:** spec § FR-005, `plan.md` § IPC, ADR-009.

---

## 7. Event bus primitive for the local transport

**Decision:** Implement the local event bus on **Tokio `broadcast` channels** (fan-out
for typed domain events) with `mpsc` for command/control paths. A `EventTransport`
trait isolates consumers/producers so a NATS adapter is additive.

**Rationale:**
- `broadcast` gives one-publisher/many-subscriber semantics that match domain event
  fan-out without extra dependencies (ADR-006, ADR-012).
- The `EventTransport` trait keeps the schema transport-independent, so NATS is a
  later adapter, not a bootstrap dependency (Principle 11, spec § FR-006).

**Alternatives considered:**
- *`tokio::sync::watch`*: rejected for multi-event streams (last-value semantics).
- *NATS first*: rejected; NATS is optional by ADR-012.

**References:** ADR-012, ADR-006, c4 § 26.

---

## 8. Cedar integration approach for bootstrap

**Decision:** Ship the **policy boundary and request model** now (`CapabilityRequest`,
`AuthorizationRequest`, `PolicyDecision`) with a **deterministic in-process default
evaluator**: read-only capabilities allowed, unknown capabilities denied, privileged
classes require explicit authorization. Add Cedar behind a `PolicyEvaluator` trait in
a follow-up without touching domain contracts.

**Rationale:**
- The spec § 8 is explicit: *"The bootstrap establishes but does not yet implement the
  complete Cedar policy model."*
- A trait-based `PolicyEvaluator` means the domain never depends on Cedar types
  (Principle 9, Principle 12-style decoupling), so Cedar can be slotted in cleanly.

**Alternatives considered:**
- *Full Cedar now*: rejected; expands bootstrap scope and couples domain to a policy
  library prematurely.
- *No policy layer*: rejected; the daemon/executor boundary MUST exist before
  privileged capabilities (spec § 8, Principle 3).

**References:** spec § 8, ADR-010, tasks T016–T021.
