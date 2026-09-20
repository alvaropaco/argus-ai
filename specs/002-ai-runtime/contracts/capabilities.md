# Capability Contract (Spec 002 extension)

**Spec:** `specs/002-ai-runtime/spec.md`
**Status:** Draft
**Date:** 2026-09-19

Extends the Spec 001 capability set with the first executable, typed action
capabilities. The Policy Engine remains authoritative (a capability declaration is
never an authorization grant).

## 1. Added capabilities

| CapabilityId | Provider | Risk class | Reversibility | Required privileges |
|---|---|---|---|---|
| `host.service.restart` | `argusd` service control | `LowRisk` | reversible | systemd/D-Bus manage access |
| `host.service.stop` | `argusd` service control | `LowRisk` | reversible | systemd/D-Bus manage access |
| `host.service.start` | `argusd` service control | `LowRisk` | reversible | systemd/D-Bus manage access |

The Spec 001 read-only capabilities (`host.status.read`, `argus.health.read`,
`argus.config.read`, `argus.plugins.list`) remain unchanged.

## 2. Descriptor

```jsonc
{
  "id": "host.service.restart",
  "provider": "argusd",
  "operation": "service.restart",
  "risk_class": "LowRisk",
  "reversibility": "reversible",
  "input_schema": { "type": "object", "required": ["unit"], "properties": { "unit": { "type": "string" } } },
  "output_schema": { "type": "object", "properties": { "result": { "type": "string" } } }
}
```

## 3. Rules

- Service actions target systemd units via D-Bus (structured), never shell.
- `risk_class` is `LowRisk`; they are reversible but still policy-checked and
  autonomy-gated (FR-004, FR-007).
- Unknown units or actions are denied by default (Principle 3).
- The executor records evidence before and after each action (Principle 10).
