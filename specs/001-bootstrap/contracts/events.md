# Event Contract

**Spec:** `specs/001-bootstrap/spec.md`
**Status:** Draft
**Date:** 2026-09-16

Domain events are transport-independent and versioned. The local bus is the default
transport; NATS/JetStream is an optional adapter (ADR-012).

## 1. Event schema

```jsonc
{
  "id": "…-uuid",
  "type": "argus.ready",                 // dotted, namespaced
  "timestamp": "2026-09-16T00:00:00Z",
  "source": "argusd",
  "subject": "host:<machine_id>",        // ResourceId or component id
  "severity": "info | warning | error",
  "correlation_id": "…",                // set when triggered by a request
  "causation_id": "…",                  // optional parent event id
  "payload": {}
}
```

## 2. Bootstrap event types

| Type | Emitted when | Payload |
|---|---|---|
| `argus.started` | daemon process starts | `{ "version", "environment_id" }` |
| `argus.ready` | readiness passes | `{ "health": … }` |
| `argus.degraded` | optional component degraded | `{ "component", "reason" }` |
| `plugin.loaded` | plugin loaded successfully | `{ "name", "version", "capabilities" }` |
| `plugin.failed` | plugin failed to load | `{ "name", "reason" }` |

## 3. Rules

- Events are **immutable** and append-only.
- Schema changes are additive and versioned; consumers must ignore unknown fields.
- The `EventTransport` trait isolates producers/consumers so NATS can be added without
  touching event producers (ADR-012, `plan.md` § 6).
- Local transport: Tokio `broadcast` for fan-out; no NATS required (FR-006).
