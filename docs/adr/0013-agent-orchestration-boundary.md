# ADR-013: Agent Orchestration Boundary

- **Status:** Accepted
- **Date:** 2026-09-16

## Decision

ARGUS is **not an agent orchestration framework**.

ARGUS is an autonomous infrastructure operations runtime. Its agent model exists to provide infrastructure reasoning and operational capabilities, while the core product remains focused on discovery, state, policy, execution, validation, observability, and continuous operations.

ARGUS may integrate external agent runtimes or frameworks when they provide useful capabilities, but such integrations must remain behind explicit interfaces and must not define the ARGUS architecture.

## Principle

ARGUS orchestrates **infrastructure operations**, not generic AI agents.

This prevents the project from becoming a general-purpose agent framework and keeps the product boundary centered on autonomous infrastructure operations.