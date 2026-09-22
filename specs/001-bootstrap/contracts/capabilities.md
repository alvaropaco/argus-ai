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

## 5. Cloud-facing descriptor (0.2.0)

When an installation publishes its capability surface to Argus Cloud the local
descriptor is **translated**, never sent as-is: the cloud validates against its own
schema, and the two vocabularies genuinely differ. The rules below are fixed by
[ADR-0016](../../docs/adr/0016-plugin-architecture.md) and are covered by tests.

| Field | Local | Cloud | Rule |
|---|---|---|---|
| `id`, `provider`, `operation` | present | present | direct |
| `risk_class` | `snake_case` (`low_risk`) | PascalCase (`LowRisk`) | case normalised |
| `reversibility` | `none` \| `reversible` \| `partially_reversible` | `reversible` \| `irreversible` \| `n/a` | see below |
| `version` | present | absent | dropped; the cloud versions the schema at publication level |
| `input_schema`, `output_schema` | present | optional | direct |
| `requires_approval` | absent | optional | derived from the authorization model, never the descriptor |

Reversibility mapping:

| Local | Cloud |
|---|---|
| `reversible` | `reversible` |
| `partially_reversible` | `reversible` |
| `none` | `irreversible` |

`none` maps to `irreversible`, not `n/a`: `n/a` reads as "reversibility does not
apply", whereas `none` means the operation cannot be undone. The conservative
reading is chosen deliberately so an operator stays warned.

Health is translated on the same basis when the installation reports it:
`Ready` → `healthy`, `Degraded` → `degraded`, `NotReady` → `critical`. `unknown`
is reserved for an instance whose health has never been observed, and is never
fabricated from a state the installation actually saw.

An empty capability surface is still published explicitly, so the cloud can tell
"this installation can do nothing" from "this installation has not said".
