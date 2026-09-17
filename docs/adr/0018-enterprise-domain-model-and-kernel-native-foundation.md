# ADR-018: Enterprise Domain Model and Kernel-Native Foundation

- **Status:** Accepted
- **Date:** 2026-09-16

## Context

ARGUS is an autonomous infrastructure operations runtime for Linux/Unix. Its architecture must support heterogeneous infrastructure without making Kubernetes or any single userspace platform the system's foundation.

Linux exposes authoritative operating-system state and control mechanisms through kernel/userspace interfaces including `/proc`, `/sys`, cgroup v2, namespaces, Netlink, pidfds, PSI, eBPF/BTF, capabilities, seccomp and LSM facilities. Kubernetes adds a higher-level declarative control plane with resources, watches, controllers, container runtime integration, networking and storage abstractions.

A shallow monitoring model would lose the causal relationships between application, workload, container, process and kernel state.

## Decision

ARGUS will use a **kernel-native, layered domain model**.

The canonical operational model will contain:

- Environment
- Host
- Kernel
- Process
- Cgroup
- Namespace
- Device
- Filesystem
- Network
- Service
- Container
- Workload
- Endpoint
- Dependency
- Observation
- Desired State
- Observed State
- Intent
- Plan
- Capability
- Policy
- Action
- Execution
- Evidence
- Incident
- Agent
- Plugin
- Event

Linux is the primary infrastructure substrate. Kubernetes is modeled as an optional control-plane provider layered above Linux.

ARGUS will explicitly correlate Kubernetes resources with host-level objects where possible:

```text
Kubernetes Pod
  -> Container ID
  -> Container Runtime
  -> Cgroup
  -> PID(s)
  -> Linux Namespaces
  -> Network Interfaces / Sockets
  -> Filesystems / Mounts
  -> Kernel Telemetry
```

The Linux kernel interfaces are preferred over parsing human-oriented CLI output when an authoritative programmatic interface exists.

The LLM is not the system of record, authorization engine, or privileged executor. Autonomous actions must be explainable through evidence, typed capabilities, policy decisions, execution results and validation.

## Consequences

### Positive

- ARGUS can reason across the complete infrastructure stack.
- Kubernetes is supported without making Kubernetes a dependency.
- Kernel-level evidence can improve diagnosis and causal analysis.
- Typed capabilities create a strong security and audit boundary.
- The same domain model can support future fleet and cloud integrations.
- The architecture creates a foundation for an ARGUS telemetry ecosystem.

### Negative

- The core domain model is significantly more sophisticated than a conventional monitoring agent.
- Linux-specific capabilities require careful kernel-version compatibility handling.
- Cross-layer correlation can be computationally and operationally complex.
- Some advanced kernel capabilities require elevated privileges and careful sandboxing.

## Implementation Principles

1. Prefer direct kernel APIs and documented interfaces over shell parsing.
2. Treat `/proc`, `/sys`, cgroup v2, PSI and Netlink as first-class discovery/telemetry sources.
3. Use pidfds for robust process lifecycle operations where available.
4. Use eBPF/BTF as an optional advanced telemetry layer, not as a baseline installation requirement.
5. Use Linux capabilities, seccomp and Landlock to minimize privileged execution surface.
6. Preserve raw observations and provenance separately from inferred conclusions.
7. Treat Kubernetes desired state and Linux observed state as separate layers that must be correlated.
8. Keep provider-specific concepts behind adapters while retaining a provider-neutral canonical domain model.

## Reference

The detailed model is documented in `docs/architecture/domain-model.md`.