# ADR-012: Internal Event Bus with Native NATS Support

- **Status:** Accepted
- **Date:** 2026-09-16

## Decision

ARGUS will implement an internal event bus as the default event transport and will provide a native NATS/NATS JetStream integration as an optional transport.

The default installation must not require an external NATS deployment.

The event abstraction will support operational events such as:

- discovery completed
- health state changed
- incident detected
- diagnosis produced
- action requested
- action approved/denied
- action executed
- remediation validated
- agent lifecycle events

NATS support will allow ARGUS installations to integrate with larger distributed environments and external event-driven systems.

## Architecture

`ARGUS Components -> Event Bus Abstraction -> Internal Transport | NATS Adapter`

## Consequences

- Single-host ARGUS remains self-contained.
- NATS can be enabled without changing agent/business logic.
- Event schemas become versioned ARGUS contracts.
- The system can evolve from local operation to distributed/fleet operation.