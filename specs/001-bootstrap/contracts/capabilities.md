# Capability Contract

**Spec:** `specs/001-bootstrap/spec.md`
**Status:** Draft
**Date:** 2026-09-16

Capabilities are the typed operations exposed to agents and to the IPC surface. Every
capability declares its required privileges and risk class; the Policy Engine remains
authoritative (the capability declaration is never an authorization grant).

## 1. Bootstrap capability set

| CapabilityId | Provider | Risk class | Required privileges |
|---|---|---|---|
| `host.status.read` | `argusd` native discovery | `Read` | none |
| `argus.health.read` | `argusd` | `Read` | none |
| `argus.config.read` | `argusd` | `Read` | none (secrets redacted) |
| `argus.plugins.list` | `argusd` plugin manager | `Read` | none |

## 2. Capability descriptor

```jsonc
{
  "id": "argus.health.read",
  "provider": "argusd",
  "operation": "health.get",
  "risk_class": "Read",
  "reversibility": "n/a",
  "input_schema": { "type": "object", "additionalProperties": false },
  "output_schema": { "type": "object", "properties": { "state": { "type": "string" } } }
}
```

## 3. Rules

- Every actionable operation MUST be a capability (no bare shell strings).
- `risk_class` is one of `Read | LowRisk | Controlled | HighRisk | Destructive`.
- Bootstrap registers **read-only** capabilities only (spec § 8, `plan.md` § 8).
- Unknown capabilities are **denied by default** (Principle 3).
- A privileged capability added later MUST declare its Linux capability/namespace/
  `seccomp`/`Landlock` requirements (ADR-016, Principle 14).

## 4. Extensibility

Plugins contribute capabilities via the TOML manifest `[capabilities]` table and are
registered through the capability registry; the registry is the single source of truth
for the `capabilities.list` operation.
