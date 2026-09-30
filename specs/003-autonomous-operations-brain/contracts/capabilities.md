# Contracts — Capabilities (spec 003)

New typed capabilities for this phase. Identifiers are typed, registry-enumerable
values (ADR-0027). Every capability declares risk class, blast radius,
reversibility, required privileges, and validation strategy (ADR-0028,
`domain-model.md` §10).

## Host / process

| Capability | Risk | Reversibility | Notes |
|---|---|---|---|
| `host.process.read` | READ | n/a | inventory + status |
| `host.process.signal` | CONTROLLED | conditional | SIGTERM/STOP/CONT via pidfd (M3) |

## Container

| Capability | Risk | Reversibility | Notes |
|---|---|---|---|
| `container.list` | READ | n/a | list containers |
| `container.inspect` | READ | n/a | inspect one container |
| `container.restart` | CONTROLLED | yes | via runtime API (M3) |

## Kubernetes (M4, ADR-0037)

| Capability | Risk | Reversibility | Notes |
|---|---|---|---|
| `k8s.cluster.read` / `k8s.node.read` / `k8s.pod.read` / `k8s.deployment.read` | READ | n/a | read-only intelligence |
| `k8s.pod.restart` | CONTROLLED | yes | delete pod, controller reschedules |
| `k8s.pod.delete` | HIGH_RISK | conditional | deny-by-default where not controller-managed |
| `k8s.deployment.restart` | CONTROLLED | yes | rollout restart |
| `k8s.deployment.rollback` | CONTROLLED | yes | to prior revision |
| `k8s.workload.scale` | CONTROLLED | yes | bounded by quotas |
| `k8s.node.cordon` | CONTROLLED | yes | `uncordon` reverses |
| `k8s.node.uncordon` | CONTROLLED | yes | — |
| `k8s.job.cleanup` | CONTROLLED | no | idempotent cleanup |
| `k8s.node.drain` | HIGH_RISK | conditional | **deferred** (deny-by-default) |
| `k8s.workload.reschedule` | HIGH_RISK | conditional | **deferred** (deny-by-default) |

## Resource autopilot (M3, CAP-15)

| Capability | Risk | Reversibility | Notes |
|---|---|---|---|
| `host.cgroup.adjust` | CONTROLLED | yes | CPU/memory/IO limits; rollback = restore |
| `host.process.priority` | CONTROLLED | yes | nice/priority; rollback = restore |
| `container.resource.adjust` | CONTROLLED | yes | runtime update |

## Read-only (existing + extended)

Existing read-only capabilities (`host.filesystem.inspect`, `host.network.inspect`,
`host.service.*` read paths) remain unchanged and are reused by the sensor stack.

## Invariants

- No capability exposes arbitrary shell or `kubectl` string execution.
- Every capability is denied by default until policy permits it.
- Guardrail predicates run at execution time against live state (ADR-0028).
