# ADR-0039: Event Streaming Backbone — Phased NATS, Existing WebSocket First

- **Status:** Accepted
- **Date:** 2026-10-09

## Context

Spec 007 (nervous system) turns agent activity — actions, brain traces, token
usage — into a continuous event stream that must reach the cloud and the
dashboard in near real time, survive offline periods, and later feed more
consumers (spec 008's graduated autonomy, alerting, fleet aggregation). The
question is the transport backbone for that stream.

Today the daemon↔cloud link is a persistent WebSocket carrying a versioned
`Envelope` protocol (`argus-cloud` transport), with a bounded offline buffer.
The local event bus (`argus-events`) is in-process broadcast, explicitly
"NATS-ready". The deployment reality is one VPS running the cloud control
plane; the constitution prizes the lean single-host build. A candidate
technology, gun.db, was proposed for "know in real time everything being
executed".

## Decision

### 1. Phase 1 (spec 007): ride the existing WebSocket envelope

Action/trace/usage events are new message kinds on the existing daemon↔cloud
WebSocket. No new infrastructure, no new dependency, offline buffering already
solved. The event *schema* is transport-independent (typed payloads, not
envelope-coupled), so nothing here forecloses a later transport change.

### 2. Phase 2 (when a second cloud-side consumer exists): NATS JetStream

When the dashboard live tail is joined by another consumer (API replays,
alerting, spec 008 services), the cloud gateway publishes the same payloads to
NATS JetStream and consumers subscribe. JetStream provides persistence,
at-least-once redelivery, replay by subject/time, and per-host/kind subject
filtering. Subjects: `argus.<instance>.actions|traces|usage`. A browser-facing
bridge (SSE) stays in the gateway; browsers never touch NATS directly.

### 3. Phase 3 (optional, much later): daemon-side NATS

Replacing the in-process `LocalEventBus` with embedded NATS on hosts is not
planned. The local bus serves one process; the WebSocket uplink serves the
fleet. Revisit only if multi-process hosts or local subscribers appear.

### 4. Alternatives considered and rejected

- **gun.db** — rejected. A JavaScript P2P graph store with no production-grade
  Rust client and irregular maintenance; embedding JS contradicts
  Constitution Principle 1 for the core. Its real-time sync appeal is fully
  covered by JetStream (Phase 2), and its decentralized, serverless topology
  conflicts with the existing hub-and-spoke cloud control plane, which is
  where auditability must live.
- **MQTT (EMQX/Mosquitto)** — viable for edge QoS tiers, but the Rust client
  ecosystem is thinner, and retained-message/replay semantics are weaker than
  JetStream's for audit replay. No capability that NATS lacks.
- **Kafka / Redpanda** — the durability/replay model is right but the
  operational weight (JVM or dedicated binary, partition zoo) is unjustified
  on a single-VPS control plane at current scale.
- **Redis Streams** — workable and lean, but delivery semantics and
  observability trail JetStream; adopting it would diverge from the
  "NATS-ready" direction already declared in the architecture without buying
  simplicity.
- **Cloud vendor buses (SNS/EventBridge/...)** — couples the control plane to
  a specific cloud and adds egress cost/latency; the VPS is deliberately
  provider-neutral.

## Consequences

- Spec 007 ships with zero new infrastructure (Phase 1).
- The gateway gains a publish-to-NATS step and consumers in Phase 2; event
  payloads do not change.
- gun.db's underlying requirement — real-time fleet-wide awareness — is met
  by Phases 1–2 with Rust-native, production-grade components.
- Revisit trigger for Phase 3: a concrete second local subscriber on hosts.
