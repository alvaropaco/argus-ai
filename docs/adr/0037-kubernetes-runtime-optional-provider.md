# ADR-0037: Kubernetes as a Runtime-Optional Provider

- **Status:** Accepted
- **Date:** 2026-09-30

## Context

CAP-12/13/14 require Kubernetes intelligence (cluster/node/pod/deployment
observation, autonomous troubleshooting, and typed self-healing capabilities),
while the constitution (Principle 6) mandates that Kubernetes must not be a
prerequisite and that infrastructure-specific knowledge lives in adapters. The
open question is whether `argus-kubernetes` is a bundled crate or an installable
plugin.

## Decision

### 1. `argus-kubernetes` is a bundled workspace crate

It compiles into the workspace but is a **runtime-optional provider** behind the
provider/plugin trait boundary. It uses `kube-rs` for the Kubernetes API. It is
not a build-time prerequisite for the core, and the core links to it only
through the provider trait.

### 2. Runtime-optional, degrade gracefully

When no cluster is configured, or the API server is unreachable, the provider
reports `degraded`/unconfigured and contributes nothing. The host/Linux path
(observation, correlation, investigation, remediation) continues unaffected.

### 3. Correlate K8s metadata with Linux evidence

The provider links a Pod to its container → cgroup → PID → namespace → socket
using the CRI/container-runtime metadata and `/proc`/cgroup reads already
available to `argus-observe`. K8s is a control-plane layer over the Linux
substrate, not a separate telemetry silo.

### 4. Self-healing capabilities are typed, never kubectl

`k8s.pod.restart/delete`, `k8s.deployment.restart/rollback`, `k8s.workload.scale`,
`k8s.node.cordon/uncordon`, `k8s.job.cleanup` are typed capabilities declared in
the registry and executed through policy + the typed executor. The provider
never shells out to `kubectl`, and no LLM output can reach the Kubernetes API
directly. Higher-risk actions (`k8s.node.drain`, `k8s.workload.reschedule`) are
deny-by-default and out of scope initially.

### 5. Troubleshooting is deterministic before AI

For CrashLoopBackOff, OOMKilled, ImagePullBackOff, Pending, failed probes, and
node pressure, the provider gathers a deterministic evidence bundle (restart
count, exit reason, image, requests/limits, events, logs) before any AI
reasoning is invoked.

## Consequences

### Positive

- Kubernetes is fully optional; the core never requires it.
- One correlation path (pod → container → cgroup → PID) drives both Linux and
  K8s diagnosis.
- Self-healing stays inside the security boundary.

### Negative

- A bundled crate still adds `kube-rs` to the dependency graph; it must be
  gated so the single-host build remains lean.
- Cross-layer correlation requires CRI/runtime metadata plumbing.

## Implementation Principles

1. Kubernetes is optional; degrade gracefully when absent.
2. Correlate K8s with Linux evidence; no separate silo.
3. Typed capabilities only; never `kubectl`; never direct model → API.
4. Deterministic evidence gathering precedes any AI reasoning.

## Reference

- `.specify/memory/constitution.md` — Principle 6 (infra-agnostic core).
- `docs/architecture/domain-model.md` — §6 Kubernetes Domain.
- `_bmad-output/specs/spec-autonomous-operations-brain/SPEC.md` — CAP-12..14.
- `_bmad-output/specs/spec-autonomous-operations-brain/roadmap.md` — Milestone 4.
- `docs/adr/0016-plugin-architecture.md`, `0027-capability-identifier-representation.md`.
