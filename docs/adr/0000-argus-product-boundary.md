# ADR-000: ARGUS Product Boundary

- **Status:** Accepted
- **Date:** 2026-09-16

## Decision

**ARGUS is an autonomous infrastructure operations runtime for Linux/Unix.**

ARGUS is not intended to be a generic agent framework, a Kubernetes-only management platform, or merely a monitoring system.

Its primary responsibility is to discover, understand, monitor, secure, operate, remediate, optimize, and continuously improve heterogeneous Linux/Unix infrastructure through autonomous AI-assisted operations.

Kubernetes, Docker, systemd, databases, cloud providers, networking systems, and other technologies are infrastructure targets exposed through adapters, tools, or plugins rather than assumptions embedded in the ARGUS core.

## Architectural Implications

- Linux/Unix is the initial supported operating environment.
- The ARGUS core must remain infrastructure-technology agnostic where practical.
- AI reasoning is a capability of ARGUS, not the product boundary itself.
- Infrastructure execution and security policy remain under ARGUS-controlled interfaces.
- External agent frameworks may be integrated when useful, but ARGUS is not itself positioned as a general-purpose agent orchestration framework.
- Extensions must be able to add infrastructure capabilities without modifying the core.

## Guiding Principle

> ARGUS should be able to arrive on an unfamiliar Linux/Unix machine, understand its environment, establish an operational model, build an appropriate operations team, implement the required capabilities, and continuously operate the environment within defined security and autonomy policies.