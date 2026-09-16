# ADR-004: Plug-and-Play Extensible Architecture

- **Status:** Accepted
- **Date:** 2026-09-16

## Context

ARGUS AI is intended to operate across Linux/Unix environments that may contain very different technologies, services, databases, container runtimes, orchestration platforms, cloud providers, and operational tooling.

The core project must not become tightly coupled to a fixed set of infrastructure technologies. New capabilities should be installable and removable without requiring changes to the ARGUS core.

## Decision

ARGUS AI will adopt a **plug-and-play, extensible architecture**.

Capabilities will be exposed through explicit extension boundaries, including where appropriate:

- MCP servers and tools.
- Infrastructure adapters.
- Discovery providers.
- Agent implementations.
- Agent skills.
- Runbooks.
- Policy providers.
- Observability integrations.
- Cloud and platform integrations.
- Storage providers.
- AI model providers.

The ARGUS core will provide stable contracts for discovering, registering, configuring, authorizing, executing, monitoring, and removing extensions.

Extensions must declare their capabilities, dependencies, required permissions, configuration schema, health status, and compatibility information.

The initial distribution will contain a curated MCP Core, while the architecture must allow users and the community to install additional extensions independently.

## Consequences

### Positive

- ARGUS can support heterogeneous infrastructure without becoming platform-specific.
- Community contributors can add integrations independently.
- Users can install only the capabilities required by their environments.
- Core upgrades can remain decoupled from most integrations.
- Enables a future ARGUS extension marketplace/registry.

### Negative

- Stable extension APIs require careful versioning.
- Plugin compatibility and security become ongoing concerns.
- The project needs explicit lifecycle and dependency management.

## Architectural Principle

**The ARGUS core should know how to host and govern capabilities, not need to know how every infrastructure technology works.**

Platform-specific knowledge belongs in extensions/adapters whenever practical.