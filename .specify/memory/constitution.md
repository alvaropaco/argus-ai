# ARGUS AI Constitution

**Version:** 1.0.0  
**Status:** Active  
**Date:** 2026-09-16

## 1. Purpose

ARGUS AI is an autonomous infrastructure operations runtime for Linux/Unix.

ARGUS continuously discovers infrastructure, builds an evidence-backed model of the environment, reasons about desired and observed state, plans changes, authorizes actions, executes permitted operations, validates outcomes, and maintains the environment autonomously.

ARGUS is not a Kubernetes-only platform, not a generic agent orchestration framework, and not merely a monitoring product.

## 2. Constitutional Principles

### Principle 1 — Rust Core

Rust is the primary implementation language for the ARGUS core, daemon, TUI, execution boundary, security-sensitive components, domain model, discovery adapters, and infrastructure control plane.

Other languages may be used for external integrations or AI-specific workflows when justified, but privileged execution remains controlled by Rust.

### Principle 2 — Security Boundary

AI reasoning and infrastructure execution MUST be separated.

LLM output is data, never authority. No model, prompt, agent, or external MCP may directly obtain unrestricted privileged shell access.

Privileged operations MUST cross the ARGUS policy and executor boundary.

### Principle 3 — Policy Before Execution

Every privileged operation MUST pass through explicit authorization before execution.

Authorization outcomes are:

- allow;
- deny;
- require human approval.

High-blast-radius and destructive capabilities MUST support explicit policy controls, approval requirements, timeout handling, and rollback where technically possible.

### Principle 4 — Evidence Before Inference

Observed infrastructure state MUST remain distinct from inferred state, desired state, and AI-generated hypotheses.

Evidence MUST preserve provenance, timestamp, source, subject, and correlation metadata.

ARGUS MUST NOT replace immutable observations with inferred conclusions.

### Principle 5 — Kernel-Native Infrastructure Intelligence

When authoritative Linux kernel interfaces exist, ARGUS SHOULD prefer them over parsing human-oriented command output.

Relevant interfaces include, where supported:

- `/proc`;
- `/sys`;
- cgroup v2;
- namespaces;
- Netlink/Generic Netlink;
- pidfd;
- PSI;
- eBPF/BTF;
- kernel event/notification facilities;
- Linux security primitives.

Userspace tools remain valid integration and fallback mechanisms when they provide capabilities unavailable or unsafe through direct kernel interfaces.

### Principle 6 — Infrastructure Agnostic Core

Kubernetes MUST NOT be a prerequisite for ARGUS.

Kubernetes is a domain adapter/control plane layered over the Linux substrate when present.

Infrastructure-specific knowledge belongs in adapters, plugins, providers, and capability implementations whenever practical.

### Principle 7 — Plug-and-Play Extensibility

ARGUS MUST support installable and removable capabilities without requiring changes to the ARGUS core.

Extensions MAY include:

- MCP servers;
- discovery providers;
- infrastructure adapters;
- agents and operational skills;
- model providers;
- policy providers;
- telemetry exporters;
- runbooks;
- cloud integrations.

Extensions MUST declare compatibility, capabilities, dependencies, and required privileges.

### Principle 8 — MCP Core + Community Ecosystem

The principal MCP capabilities required for initial Linux/Unix operation MUST ship with the ARGUS installation package.

Community MCPs MAY be integrated through the plugin model.

MCP is an integration protocol, not the ARGUS security boundary.

### Principle 9 — Deterministic Control Plane

The domain model, policy engine, executor, capability registry, persistence contracts, and security boundaries MUST remain deterministic and testable independently of any LLM.

ARGUS MUST NOT make correctness or safety depend on a model behaving consistently.

### Principle 10 — Auditable Autonomy

Every autonomous action MUST produce structured records for:

- initiating incident/intent;
- proposed plan;
- authorization decision;
- action;
- execution;
- resulting evidence;
- validation outcome;
- rollback/escalation when applicable.

### Principle 11 — Graceful Degradation

Optional components MUST degrade cleanly.

Basic single-host operation MUST NOT require:

- Kubernetes;
- NATS;
- eBPF;
- external observability systems;
- external databases.

ARGUS MUST retain useful discovery and operational functionality when optional capabilities are unavailable.

### Principle 12 — Storage Independence

LanceDB is the initial operational state implementation, but domain logic MUST NOT depend directly on LanceDB APIs.

Persistence MUST be accessed through repository/domain abstractions so that alternative backends can be introduced later.

### Principle 13 — Testability

Every major capability MUST be testable independently from:

- the LLM provider;
- production infrastructure;
- network availability;
- Kubernetes;
- external MCP servers.

Integration tests SHOULD use deterministic fixtures, mocks, test containers, or isolated test environments as appropriate.

### Principle 14 — Least Privilege

Every component MUST receive the smallest practical privilege set required for its responsibility.

Plugins and MCPs MUST NOT receive privileges implicitly.

ARGUS SHOULD use Linux capabilities, namespaces, seccomp, Landlock, and other available controls when they reduce risk.

### Principle 15 — Stable Core Contracts

Changes to domain entities, capability contracts, plugin interfaces, persisted state, action schemas, policy semantics, and security boundaries require explicit architectural review.

Breaking changes MUST be documented and versioned.

### Principle 16 — SpecKit / ADR Separation

SpecKit defines feature intent, requirements, implementation plans, and tasks.

ADRs define durable architectural decisions.

Architecture documents define the system structure and boundaries.

Implementation code realizes these contracts.

A feature MUST NOT silently redefine an architectural decision. When implementation requires a new architectural decision, the appropriate ADR MUST be updated or created.

### Principle 17 — No Generic Agent Framework by Accident

ARGUS is an autonomous infrastructure operations runtime, not a generic agent orchestration framework.

Agent abstractions MUST exist only where they directly support infrastructure operations, reasoning, capability management, incident response, or related ARGUS responsibilities.

### Principle 18 — Autonomous Control Loop

The canonical ARGUS loop is:

```text
Observe → Correlate → Understand → Plan → Authorize → Execute → Validate → Learn
```

Implementations SHOULD preserve these stages as explicit concepts even when optimized or combined internally.

## 3. Governance

All contributors and AI coding agents MUST consult this constitution and relevant ADRs before modifying core architecture.

An implementation that violates a constitutional principle requires an explicit documented exception and architectural review.
