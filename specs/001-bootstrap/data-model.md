# Data Model: ARGUS Bootstrap Runtime

**Spec:** `specs/001-bootstrap/spec.md`
**Status:** Draft
**Date:** 2026-09-16

Bootstrap-scope domain entities. The full enterprise domain model lives in
`docs/architecture/domain-model.md`; this file defines only the entities the
bootstrap actually materializes, plus their validation rules, relationships, and
state transitions.

Convention: all identifiers are strongly-typed newtypes (never raw `String`/`u64`)
and must not leak infrastructure-specific types into `argus-domain` (Principle 12).

---

## 1. Identifiers

### `EnvironmentId`

- **Type:** newtype over a UUID (`uuid::Uuid`).
- **Validation:** non-nil UUID, generated once per installation and persisted in the
  `environment` table.
- **Purpose:** top-level operational boundary identity.

### `ResourceId`

- **Type:** newtype over a stable string, `{kind}:{identifier}` (e.g.
  `host:<machine_id>`).
- **Validation:** non-empty, lower-kebab `kind`, non-empty `identifier`, no control
  characters.
- **Purpose:** addresses any resource in the correlation graph uniformly.

### `CapabilityId`

- **Type:** newtype over a dotted capability path (e.g. `host.status.read`).
- **Validation:** matches `^[a-z][a-z0-9]*(\.[a-z][a-z0-9]*)+$`; registered in the
  capability registry before use.
- **Purpose:** typed, policy-checkable operation identifiers.

---

## 2. Value Objects

### `HealthStatus`

- **Fields:**
  - `state`: `Ready | Degraded | NotReady` (enum)
  - `reason`: optional human-readable degradation cause
  - `checked_at`: timestamp (RFC 3339 / `SystemTime`)
- **Validation:** `reason` is required iff `state == Degraded`.
- **Transitions:** `NotReady → Ready`, `Ready ⇄ Degraded`, `Degraded → NotReady`
  (on shutdown).

### `Observation`

- **Fields:**
  - `id`: `ResourceId`-compatible stable id
  - `source`: string (e.g. `argusd`, `procfs`)
  - `subject`: `ResourceId` of the observed entity
  - `attribute`: string (e.g. `memory.pressure`)
  - `value`: typed value (string / number / boolean)
  - `confidence`: `f32` in `[0.0, 1.0]`
  - `provenance`: source + method + timestamp of collection
  - `observed_at`: timestamp
- **Rules:** **immutable** once persisted; never overwritten by inferred state
  (Principle 4). `confidence` clamped to `[0.0, 1.0]`.

### `RequestContext`

- **Fields:**
  - `correlation_id`: UUID
  - `protocol_version`: semver string
  - `principal`: client identity (UID/GID verified via `SO_PEERCRED`)
  - `requested_at`: timestamp
- **Rules:** `correlation_id` is required and echoed through logs/traces/audit.

---

## 3. Entities

### `Host`

- **Fields:** `id: ResourceId`, `hostname`, `machine_id`, `boot_id`, `kernel_release`,
  `architecture`.
- **Validation:** `machine_id`/`boot_id` optional if the kernel does not expose them.
- **Relationships:** `Host → [Observation]` (has-many evidence).

### `PluginManifest`

- **Fields:** `name`, `version` (semver), `api_version`, `type`
  (`mcp | adapter | discovery | provider | policy | telemetry | runbook`),
  `capabilities: Vec<CapabilityId>`, `permissions: Vec<String>`,
  `dependencies: Vec<String>`, `health_check: Option<String>`.
- **Validation:** name matches `^[a-z][a-z0-9-]*$`; version is valid semver;
  `permissions` are declarative (not authorization grants — see ADR-016).
- **Transitions:** `Discovered → Loaded → Ready | Failed | Disabled`, `→ Removed`.

### `DomainEvent`

- **Fields:** `id`, `type`, `timestamp`, `source`, `subject`, `severity`,
  `correlation_id`, `causation_id`, `payload`.
- **Validation:** `type` matches the event taxonomy (see `contracts/events.md`); every
  event carries a `correlation_id` when triggered by a request.
- **Rules:** immutable; transport-independent schema (ADR-012).

---

## 4. Policy / Execution Contracts (boundary only)

These types establish the policy/executor boundary now; they are evaluated by the
bootstrap in-process evaluator and later by Cedar (see `research.md` § 8).

### `CapabilityRequest`

- **Fields:** `capability: CapabilityId`, `principal`, `resource: Option<ResourceId>`,
  `arguments: Map<String, Value>`, `context: RequestContext`.

### `AuthorizationRequest`

- **Fields:** `capability_request`, `risk_class`
  (`Read | LowRisk | Controlled | HighRisk | Destructive`), `blast_radius`.

### `PolicyDecision`

- **Fields:** `outcome: Allow | Deny | RequireApproval`, `reason`, `policy_id`,
  `decided_at`.
- **Transitions:** terminal per request (no re-open).

---

## 5. State Transition Summary

| Entity | States | Triggers |
|---|---|---|
| `HealthStatus` | `NotReady → Ready → Degraded → …` | daemon startup, health checks, shutdown |
| `PluginManifest` | `Discovered → Loaded → Ready/Failed/Disabled → Removed` | discovery, load result, health check, lifecycle |
| `PolicyDecision` | terminal (`Allow`/`Deny`/`RequireApproval`) | policy evaluation |
