# ADR-015: Observability and OpenTelemetry

- **Status:** Accepted
- **Date:** 2026-09-16

## Decision

ARGUS will use the Rust `tracing` ecosystem and **OpenTelemetry** as its observability foundation.

ARGUS must emit structured logs, metrics, and traces through standard interfaces and should support OTLP-based export to external observability systems.

Observability is both an internal requirement and a strategic product boundary. The architecture must allow a future ARGUS telemetry ecosystem to provide commercial-grade infrastructure telemetry, analytics, dashboards, anomaly detection, retention, fleet visibility, and related services around the ARGUS runtime.

## Principle

ARGUS must be observable by default and must avoid locking telemetry into a single backend.

## Consequences

- Native ARGUS behavior can be diagnosed independently of the AI layer.
- OpenTelemetry provides interoperability with existing telemetry ecosystems.
- Future ARGUS telemetry products can consume the same canonical signals.
- Telemetry collection must be designed with low overhead and explicit privacy/security controls.