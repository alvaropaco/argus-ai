# IPC Protocol Contract

**Spec:** `specs/001-bootstrap/spec.md`
**Status:** Draft
**Date:** 2026-09-16

Versioned request/response protocol between `argus` (CLI/TUI) and `argusd` over a
Unix-domain stream socket. This is a control plane, not a shell: every operation is
typed.

## 1. Transport & framing

- Socket path: `/run/argus/argusd.sock` (root-owned directory, socket mode `0600`).
- Framing: **newline-delimited JSON** — one JSON object per line.
- Connection identity: `SO_PEERCRED` (kernel-authoritative UID/GID) — the daemon
  never trusts client-supplied identity.
- Serialization: `serde_json`.

## 2. Envelope

Every request and response is wrapped in a versioned envelope.

```jsonc
// Request
{
  "protocol_version": "0.2.0",
  "correlation_id": "4f0a…-uuid",
  "operation": "health.get",        // typed operation id
  "principal": "uid=1000 gid=1000", // informational; authority is SO_PEERCRED
  "payload": {}                     // operation-specific, optional
}
```

```jsonc
// Response (success)
{
  "protocol_version": "0.2.0",
  "correlation_id": "4f0a…-uuid",
  "ok": true,
  "result": {}
}
```

```jsonc
// Response (error)
{
  "protocol_version": "0.2.0",
  "correlation_id": "4f0a…-uuid",
  "ok": false,
  "error": {
    "code": "UNAUTHORIZED",       // machine-readable
    "message": "..."              // human-readable, no secrets
  }
}
```

## 3. Error codes

| Code | Meaning |
|---|---|
| `MALFORMED` | invalid JSON or envelope |
| `UNSUPPORTED_VERSION` | protocol_version not supported |
| `UNKNOWN_OPERATION` | operation id not registered |
| `UNAUTHORIZED` | peer credentials not permitted for the operation |
| `DENIED` | policy evaluation returned `Deny`/`RequireApproval` |
| `NOT_READY` | daemon not yet ready |
| `INTERNAL` | unexpected internal failure |

## 4. Operations (bootstrap)

| Operation | Authorization | Payload | Result |
|---|---|---|---|
| `health.get` | any authorized peer | `{}` | `{ "state": "Ready\|Degraded\|NotReady", "reason": null\|"…", "checked_at": "…" }` |
| `status.get` | any authorized peer | `{}` | `{ "health": …, "environment_id": "…", "plugins": …, "uptime_seconds": … }` |
| `config.get` | any authorized peer | `{ "section": "…" }` | non-secret config subsection (secrets always redacted) |
| `plugins.list` | any authorized peer | `{}` | `[ { "name", "version", "state", "capabilities" } ]` |
| `capabilities.list` | any authorized peer | `{}` | `[ "host.status.read", "argus.health.read", … ]` |

## 5. Cloud operations (0.2.0)

Added for Argus Cloud connectivity. Each is a MINOR addition, so `protocol_version`
moves `0.1.0` → `0.2.0` and older clients keep working.

| Operation | Authorization | Payload | Result |
|---|---|---|---|
| `cloud.enroll` | allowlisted UID or root | `{ "code": "ARGUS-…" }` | `{ "ok": true, "tenant_id", "installation_id", "instance_name", "state" }`, or `{ "ok": false, "code", "message" }` |
| `cloud.status` | any authorized peer | `{}` | connectivity `state`, `enabled`, `endpoint_configured`, tenant and installation identity, `instance_name`, `protocol_version`, `last_exchange_at`, `last_error`, `buffered_reports`, `buffer_capacity`, `dropped_reports`, `privileged_execution_enabled`, `remediation` |
| `cloud.forget` | allowlisted UID or root | `{ "confirm": true }` | `{ "state": "not_configured" }`; idempotent |

Rules:

- `cloud.status` reports *that* a credential exists and when it was last used, never
  its value. The result must be safe to paste into a ticket.
- `cloud.enroll` and `cloud.forget` change identity material, so they are restricted to
  allowlisted peers or root. `cloud.status` is diagnostic and open to any authorized peer.
- `cloud.forget` is idempotent and deliberately does not contact the cloud: an operator
  reaches for it exactly when the cloud cannot be reached.
- `cloud.enroll` refuses when an enrollment already exists (`ALREADY_ENROLLED`) rather
  than silently rebinding the installation to another organization.
- A fourth operation, `cloud.set-privileged-execution`, arrives with the privileged
  execution feature rather than being stubbed here.

## 6. Compatibility rules

- `protocol_version` is a semver `MAJOR.MINOR` contract. A client MUST be rejected
  (`UNSUPPORTED_VERSION`) when `MAJOR` differs; `MINOR` differences are tolerated
  (forward-compatible additive fields only).
- The daemon must tolerate unknown additive fields (ignore, don't error).
- Adding an operation is a `MINOR` bump; removing/renaming is `MAJOR`.

## 7. Test matrix

- envelope round-trip (serde) unit tests;
- malformed frame → `MALFORMED`;
- wrong `MAJOR` version → `UNSUPPORTED_VERSION`;
- unknown operation → `UNKNOWN_OPERATION`;
- peer not in allowed UID set → `UNAUTHORIZED`;
- daemon booting → `NOT_READY` until ready.
