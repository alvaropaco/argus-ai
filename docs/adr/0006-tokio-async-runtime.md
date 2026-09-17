# ADR-006: Tokio as the Async Runtime

- **Status:** Accepted
- **Date:** 2026-09-16

## Decision

ARGUS will use **Tokio** as its asynchronous runtime.

Tokio will provide the execution foundation for concurrent discovery, monitoring, MCP communication, IPC, networking, timers, subprocess management, events, and long-running background tasks.

## Consequences

ARGUS components should prefer Tokio-compatible libraries and avoid introducing competing async runtimes into the core unless technically justified.