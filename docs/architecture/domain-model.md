# ARGUS AI — Enterprise Domain Model & Control Plane

- **Status:** Draft architecture
- **Date:** 2026-09-16
- **Scope:** Linux/Unix first; Kubernetes as an optional infrastructure domain

## 1. Purpose

ARGUS is an autonomous infrastructure operations runtime for Linux/Unix.

ARGUS is not primarily a monitoring product, Kubernetes manager, or generic agent framework. Its core responsibility is to continuously build an evidence-backed model of an environment, reason about its desired and observed state, plan changes, execute authorized actions, validate outcomes, and maintain the environment autonomously.

The architecture therefore treats the operating system kernel as a first-class substrate and Kubernetes as an additional control-plane/domain layer when present.

## 2. Architectural Principle

ARGUS should understand infrastructure from the bottom up:

```text
Hardware / VM
    ↓
Linux Kernel
    ↓
Kernel interfaces and events
    ↓
Processes / cgroups / namespaces / network / filesystems
    ↓
System services / container runtime
    ↓
Containers / workloads
    ↓
Kubernetes control plane (optional)
    ↓
Applications / dependencies
    ↓
Business-facing services
```

The reverse direction is equally important for diagnosis:

```text
Incident
  ↓
Service
  ↓
Workload
  ↓
Container
  ↓
PID / cgroup / namespace
  ↓
Process / socket / file / resource pressure
  ↓
Kernel evidence
```

This allows ARGUS to answer questions such as:

> Why is this Kubernetes pod slow?

not merely from Pod status, but by correlating the Pod to its container, Linux cgroup, PID, network namespace, sockets, CPU/memory/IO pressure, filesystem state, and kernel-level events.

## 3. Canonical Domain Model

The core domain model contains the following concepts.

### 3.1 Environment

The top-level operational boundary.

Examples:

- one VPS
- one physical server
- one VM
- a fleet of Linux hosts
- a Kubernetes cluster
- a hybrid environment

### 3.2 Host

A Linux/Unix execution node.

Attributes include:

- hostname
- boot ID
- machine ID
- kernel release
- architecture
- CPU topology
- NUMA topology
- memory
- swap
- storage
- network interfaces
- mounted filesystems
- security configuration
- time synchronization
- running services

### 3.3 Kernel

The running kernel and its exposed capabilities.

Model:

```text
Kernel
 ├── release
 ├── version
 ├── architecture
 ├── config
 ├── modules
 ├── BTF
 ├── BPF capabilities
 ├── LSMs
 ├── namespaces
 ├── cgroup controllers
 ├── sysctls
 └── boot parameters
```

### 3.4 Process

A process is a first-class ARGUS resource.

Important identity fields:

- pid
- pid namespace
- start time
- executable
- command line
- parent PID
- process group
- session
- UID/GID
- Linux capabilities
- cgroup
- namespaces
- open file descriptors
- sockets
- memory mappings
- CPU usage
- IO usage
- security context

ARGUS should use pidfds for process lifecycle operations when supported, rather than relying exclusively on PID numbers. pidfds provide a stable file-descriptor reference to a task and can be monitored for task termination and used for signal delivery. [Linux pidfd documentation](https://man7.org/linux/man-pages/man2/pidfd_open.2.html).

### 3.5 Cgroup

cgroups are a core resource-control and identity primitive.

The ARGUS model should represent:

```text
Cgroup
 ├── hierarchy
 ├── controllers
 ├── cpu
 ├── memory
 ├── io
 ├── pids
 ├── cpuset
 ├── pressure
 ├── limits
 ├── current usage
 ├── events
 └── descendant processes
```

Linux cgroup v2 exposes resource controllers and per-cgroup state; ARGUS should treat cgroups as the bridge between host processes and container/workload resource accounting. [Linux cgroup v2 documentation](https://docs.kernel.org/6.0/admin-guide/cgroup-v2.html).

### 3.6 Namespace

Namespaces represent isolation boundaries.

ARGUS should model at least:

- mount
- PID
- network
- IPC
- UTS
- user
- cgroup
- time

A namespace object should expose its inode/identity and membership relationships so ARGUS can correlate isolated processes with their host representation. Linux namespaces are a fundamental mechanism used to create container-style isolation. [Linux namespaces](https://man7.org/linux/man-pages/man7/namespaces.7.html).

### 3.7 Network

Network is modeled as a graph, not only as metrics.

```text
NetworkNamespace
  ├── Interface
  ├── Address
  ├── Route
  ├── Neighbor
  ├── Link
  ├── Bridge
  ├── VLAN
  ├── Tunnel
  ├── Socket
  ├── Listener
  └── Connection
```

ARGUS should use Linux Netlink/Generic Netlink and rtnetlink-capable interfaces where appropriate instead of parsing CLI output as its primary source. Netlink is a kernel/userspace interface used for structured communication and dynamic network configuration/state. [Linux Netlink Handbook](https://docs.kernel.org/userspace-api/netlink/index.html).

### 3.8 Filesystem

A filesystem is modeled independently from the process using it.

```text
Filesystem
 ├── device
 ├── mount
 ├── filesystem type
 ├── capacity
 ├── inodes
 ├── options
 ├── mount namespace
 ├── read/write state
 ├── IO pressure
 └── consumers
```

ARGUS should detect overlay filesystems and preserve lower/upper/workdir relationships for containerized environments. [Linux OverlayFS documentation](https://docs.kernel.org/filesystems/overlayfs.html).

### 3.9 Device

Model hardware and virtual devices exposed through sysfs, including:

- block devices
- network devices
- PCI devices
- USB devices
- GPUs
- NVMe
- accelerators
- virtual devices

sysfs exports kernel objects, attributes, and relationships to userspace and provides a stable kernel/userspace ABI for documented interfaces. [Linux sysfs documentation](https://docs.kernel.org/filesystems/sysfs.html).

### 3.10 Service

A service abstracts a long-running operational unit.

A service may be implemented by:

- systemd
- SysV/init scripts
- Docker
- containerd/CRI-O
- Kubernetes
- custom supervisors
- a raw process

The domain model must not equate Service with systemd Unit.

### 3.11 Container

A container is represented as a composite object connecting:

```text
Container
 ├── runtime
 ├── image
 ├── container ID
 ├── process tree
 ├── cgroup
 ├── namespaces
 ├── filesystem
 ├── mounts
 ├── network namespace
 ├── interfaces
 ├── ports
 └── security profile
```

### 3.12 Workload

A workload represents what the system is trying to run.

Examples:

- systemd service
- Docker Compose service
- Kubernetes Deployment
- StatefulSet
- Job
- CronJob
- raw daemon

### 3.13 Endpoint

A logical network endpoint:

```text
Endpoint
 ├── protocol
 ├── address
 ├── port
 ├── listener
 ├── owning process
 ├── container
 ├── service
 └── network policy
```

### 3.14 Dependency

The graph must explicitly model relationships such as:

```text
depends_on
connects_to
exposes
runs_on
managed_by
owned_by
mounted_by
uses
stores_data_in
routes_to
authenticated_by
```

### 3.15 Observation

An observation is evidence collected from the environment.

```text
Observation
 ├── source
 ├── timestamp
 ├── subject
 ├── attribute
 ├── value
 ├── confidence
 └── provenance
```

Observations are immutable evidence; inferred state should never overwrite the evidence.

### 3.16 State

ARGUS distinguishes:

```text
Observed State
Desired State
Inferred State
Historical State
```

This is critical for autonomous operations. The system must be able to say what it observed separately from what it believes should be true.

### 3.17 Intent

An intent describes a desired condition.

Examples:

```text
service nginx should be running
replicas >= 3
port 443 should be reachable
backup freshness < 24h
memory pressure should remain below threshold
```

### 3.18 Plan

A plan is a proposed sequence of actions designed to move observed state toward desired state.

```text
Plan
 ├── objective
 ├── assumptions
 ├── preconditions
 ├── actions
 ├── dependencies
 ├── expected outcomes
 ├── rollback
 ├── blast radius
 └── confidence
```

### 3.19 Action

An action is a typed operation.

Examples:

```text
RestartService
StopProcess
StartProcess
KillProcess
ScaleWorkload
RollbackDeployment
ChangeCgroupLimit
RotateCertificate
CreateBackup
ModifyRoute
BlockEndpoint
DeployArtifact
```

Actions should be typed rather than arbitrary shell strings wherever possible.

### 3.20 Execution

An execution is the actual attempt to perform an action.

```text
Execution
 ├── action
 ├── actor
 ├── policy decision
 ├── start time
 ├── end time
 ├── exit result
 ├── stdout/stderr
 ├── side effects
 └── evidence
```

### 3.21 Evidence

Evidence is produced both before and after actions.

The ARGUS control loop is:

```text
Observe → Hypothesize → Plan → Authorize → Execute → Observe → Validate
```

### 3.22 Incident

An incident aggregates observations, symptoms, hypotheses, plans, actions and outcomes.

```text
Incident
 ├── trigger
 ├── impact
 ├── affected resources
 ├── timeline
 ├── hypotheses
 ├── evidence
 ├── plan
 ├── actions
 ├── remediation
 └── resolution
```

### 3.23 Policy

Policy governs what ARGUS may do.

It is evaluated independently from LLM output.

```text
principal
action
resource
context
→ allow / deny / require approval
```

### 3.24 Agent

An Agent is a reasoning capability, not the infrastructure itself.

Examples:

- SRE
- SecOps
- DevOps
- Capacity
- Network
- Database
- Backup
- Engineering

Agents consume observations and capabilities; they do not own privileged execution.

### 3.25 Plugin

A Plugin adds capabilities without modifying the ARGUS core.

Plugin types:

```text
MCP
Adapter
Discovery
Agent skill
Provider
Policy provider
Telemetry exporter
Runbook
```

### 3.26 Capability

A capability is a typed operation exposed to agents.

Examples:

```text
host.process.read
host.process.signal
host.service.restart
host.filesystem.inspect
host.network.inspect
container.list
container.restart
k8s.pod.read
k8s.deployment.rollback
```

Every capability should declare required privileges.

## 4. Linux Kernel Substrate

ARGUS should use the kernel interfaces directly wherever they provide authoritative data or safer control semantics.

### 4.1 `/proc`

Use `/proc` for process and kernel state discovery.

Primary domains:

```text
/proc/<pid>/status
/proc/<pid>/stat
/proc/<pid>/cmdline
/proc/<pid>/exe
/proc/<pid>/fd
/proc/<pid>/mountinfo
/proc/<pid>/ns/*
/proc/<pid>/cgroup
/proc/<pid>/net/*
/proc/meminfo
/proc/vmstat
/proc/loadavg
/proc/pressure/*
/proc/sys/*
```

The kernel documents `/proc` as an interface to internal kernel data structures and process state, including runtime sysctl controls. [Linux `/proc`](https://docs.kernel.org/filesystems/proc.html).

ARGUS should not scrape `ps`, `top`, `free`, `df`, `ip`, or similar utilities when equivalent kernel state is directly available.

### 4.2 `/sys`

Use sysfs to discover kernel devices, buses, power, CPU topology, block devices, network devices and subsystem attributes.

### 4.3 cgroups v2

This is a foundational ARGUS primitive.

Capabilities include:

```text
CPU quota/weight
Memory limits
Memory pressure
IO control
PID limits
CPU affinity/cpuset
Hierarchical accounting
Resource events
Process membership
```

ARGUS should treat cgroup identity as part of workload identity.

### 4.4 PSI

Linux Pressure Stall Information exposes CPU, memory and IO contention and supports threshold-triggered notifications. ARGUS should use PSI as a first-class signal instead of relying only on utilization percentages. [Linux PSI](https://cdn.kernel.org/doc/html/latest/accounting/psi.html).

This enables reasoning such as:

```text
CPU = 60%
BUT
CPU PSI = rising sharply
```

which indicates contention that raw utilization may not explain.

### 4.5 Netlink

Use Netlink/Generic Netlink for kernel networking and other subsystem state where supported.

Potential domains:

```text
interfaces
addresses
routes
neighbors
bridges
VLANs
network namespaces
firewall state
traffic control
wireless
```

### 4.6 pidfd

Use pidfds for process identity, lifecycle monitoring and signals when supported. This avoids PID-reuse races inherent to integer-PID-only designs. [pidfd_open](https://man7.org/linux/man-pages/man2/pidfd_open.2.html), [pidfd_send_signal](https://man7.org/linux/man-pages/man2/pidfd_send_signal.2.html).

### 4.7 eBPF/BTF

ARGUS should have an optional **Kernel Telemetry Engine** based on eBPF.

Potential capabilities:

```text
process exec/exit tracing
syscall observation
file activity
network connections
TCP lifecycle
DNS observations where feasible
run queue latency
block IO
memory events
security events
container-aware tracing
```

BPF programs can attach to kernel hooks such as tracepoints, kprobes, cgroup hooks and network paths. BTF/CO-RE allows BPF applications to adapt to the running kernel's type information, and the kernel recommends libbpf/libbpf-rs for this class of applications. [Linux BPF documentation](https://www.kernel.org/doc/html/latest/bpf/), [libbpf overview](https://origin.kernel.org/doc/html/latest/bpf/libbpf/libbpf_overview.html).

Important design rule: eBPF is an enhancement layer, not a hard dependency for basic ARGUS operation.

### 4.8 Security capabilities

ARGUS should avoid running the entire application as unrestricted root. Linux capabilities divide traditional superuser privileges into independently controllable units such as `CAP_BPF`, `CAP_NET_ADMIN`, `CAP_SYS_PTRACE`, and others. [Linux capabilities](https://man7.org/linux/man-pages/man7/capabilities.7.html).

The privileged daemon should use the smallest capability set required for each operation and drop capabilities when possible.

### 4.9 seccomp

seccomp should be used to reduce the syscall surface of sandboxed components. It is not a complete sandbox by itself; filesystem, namespace, LSM and policy controls must complement it. [seccomp](https://docs.kernel.org/userspace-api/seccomp_filter.html).

### 4.10 Landlock

Landlock is particularly interesting for ARGUS plugin isolation because it can restrict filesystem and network rights from userspace without requiring privileged access. [Landlock](https://docs.kernel.org/userspace-api/landlock.html).

Potential use:

```text
Untrusted plugin
      ↓
Landlock restrictions
      ↓
Limited filesystem/network view
```

### 4.11 Kernel event sources

ARGUS should prefer event-driven mechanisms where available instead of aggressive polling.

Candidate mechanisms:

```text
pidfd
PSI triggers
eBPF ring buffers
netlink multicast
inotify
fanotify
watch_queue
systemd D-Bus signals
```

The kernel watch queue is a general notification mechanism capable of delivering kernel-generated notifications to userspace. [watch_queue](https://docs.kernel.org/core-api/watch_queue.html).

## 5. Security Model for Kernel Access

ARGUS execution must be tiered.

```text
AI Process
   │
   ▼
Capability Request
   │
   ▼
Policy Engine
   │
   ├── deny
   ├── approve
   └── require human approval
   │
   ▼
Privileged Executor
   │
   ▼
Kernel API / userspace subsystem
```

The AI process must not receive unrestricted root shell access.

## 6. Kubernetes Domain

Kubernetes is represented as an additional control plane layered over the Linux substrate.

### 6.1 Kubernetes Cluster

```text
Cluster
 ├── API Server
 ├── Controller Manager
 ├── Scheduler
 ├── Nodes
 ├── API Groups
 ├── CRDs
 ├── Admission
 └── Extensions
```

The Kubernetes API is resource-based and supports CRUD, subresources, discovery, watches and cached/list synchronization semantics. [Kubernetes API Concepts](https://kubernetes.io/docs/reference/using-api/api-concepts/).

### 6.2 API Object

ARGUS should model Kubernetes objects generically:

```text
K8sObject
 ├── apiVersion
 ├── kind
 ├── metadata
 ├── spec
 ├── status
 ├── ownerReferences
 ├── labels
 ├── annotations
 ├── finalizers
 └── resourceVersion
```

The generic model should allow ARGUS to observe unknown/custom resource kinds without needing a new release for every CRD.

### 6.3 Kubernetes Desired vs Actual State

Kubernetes itself provides a powerful desired-state model: objects are persistent records of intent and controllers continuously work to make actual state match desired state. [Kubernetes Objects](https://kubernetes.io/docs/concepts/overview/working-with-objects/).

ARGUS should therefore correlate three levels:

```text
ARGUS Desired Intent
        ↓
Kubernetes Desired State
        ↓
Kubernetes Actual State
        ↓
Linux Actual State
```

### 6.4 Workload Model

ARGUS should understand:

```text
Namespace
Pod
ReplicaSet
Deployment
StatefulSet
DaemonSet
Job
CronJob
```

But the canonical business model should remain `Workload`; Kubernetes resource kinds are implementations/providers of that abstraction.

### 6.5 Node Model

A Kubernetes Node should be linked to a physical/virtual `Host` entity.

```text
K8s Node
   │
   └── Host
        ├── Kernel
        ├── CPU
        ├── Memory
        ├── Disk
        ├── Network
        └── Processes
```

### 6.6 Pod-to-Kernel Correlation

This is a major ARGUS capability.

```text
Pod
 │
 ├── Pod UID
 ├── Container ID
 │       │
 │       ▼
 │   Container runtime
 │       │
 │       ▼
 │   Linux cgroup
 │       │
 │       ▼
 │   PID(s)
 │       │
 │       ├── namespaces
 │       ├── sockets
 │       ├── files
 │       └── syscalls/eBPF events
```

Kubernetes uses the CRI as the stable protocol between kubelet and the container runtime. [CRI](https://kubernetes.io/docs/concepts/containers/cri/).

ARGUS should correlate Kubernetes metadata with CRI/container metadata and then with kernel state.

### 6.7 Kubernetes Networking

Model:

```text
Pod
 └── Network Namespace
      ├── Interface
      ├── IP
      ├── Route
      └── Socket
           │
           ▼
      Host Network
           │
           ▼
      CNI / routing / firewall
```

Kubernetes defines the network model, while node-level runtime components and CNI plugins implement substantial parts of it. [Kubernetes networking](https://kubernetes.io/docs/concepts/services-networking/).

### 6.8 Kubernetes Storage

ARGUS should correlate:

```text
PVC
 ↓
PV
 ↓
StorageClass / CSI
 ↓
Node mount
 ↓
Linux filesystem
 ↓
Block device
```

CSI provides the standard extension interface for external storage implementations. [Kubernetes volumes/CSI](https://kubernetes.io/docs/concepts/storage/volumes/).

### 6.9 Kubernetes Metrics

ARGUS should consume the Kubernetes Metrics API when available, but should not depend on it for node truth.

Kubernetes v1.37 promoted `metrics.k8s.io/v1` to stable for node and Pod CPU/memory metrics. The kubelet, metrics-server and Metrics API form the resource metrics pipeline. [Kubernetes resource metrics](https://kubernetes.io/docs/tasks/debug/debug-cluster/resource-metrics-pipeline/).

ARGUS can compare:

```text
Kubernetes metrics
        vs
Linux cgroup metrics
        vs
Linux PSI
        vs
ARGUS eBPF telemetry
```

This comparison is valuable for detecting accounting gaps and identifying whether resource problems originate in workloads or on the host.

## 7. Correlation Graph

The real enterprise value of ARGUS is the unified graph.

Example:

```text
Internet
  │
Cloudflare
  │
Ingress
  │
Service
  │
Deployment
  │
Pod
  │
Container
  │
Cgroup
  │
PID
  │
Network Namespace
  │
Socket
  │
Remote Endpoint
```

Simultaneously:

```text
Pod
 │
 └── Volume
      │
      └── Mount Namespace
           │
           └── OverlayFS / block device
```

And:

```text
PID
 │
 ├── CPU
 ├── memory
 ├── IO
 ├── PSI
 ├── syscalls
 └── file/network events
```

## 8. Enterprise Control Loops

ARGUS should implement multiple control loops rather than one monolithic agent loop.

```text
Discovery Loop
Telemetry Loop
Health Loop
Security Loop
Capacity Loop
Backup Loop
Remediation Loop
Configuration Drift Loop
Patch Loop
Certificate Loop
Dependency Loop
Cost Loop
```

Each loop consumes the same domain model and emits typed events/actions.

## 9. Core Event Model

Events should be normalized regardless of source.

```text
Event
 ├── id
 ├── type
 ├── timestamp
 ├── source
 ├── subject
 ├── severity
 ├── evidence
 ├── correlation_id
 └── causation_id
```

Examples:

```text
host.memory.pressure
process.started
process.exited
service.failed
container.restarted
cgroup.memory.threshold
network.link.changed
network.connection.failed
filesystem.space.low
k8s.pod.pending
k8s.deployment.degraded
k8s.node.notready
security.capability.changed
certificate.expiring
backup.stale
```

## 10. Capability Model

Every actionable operation must become a capability.

```text
Capability
 ├── id
 ├── provider
 ├── operation
 ├── input schema
 ├── output schema
 ├── required permissions
 ├── risk level
 ├── reversibility
 └── validation strategy
```

Risk classes:

```text
READ
LOW_RISK
CONTROLLED
HIGH_RISK
DESTRUCTIVE
```

## 11. Plugin Boundary

Every extension should expose a TOML manifest.

```toml
[plugin]
name = "docker"
version = "1.0.0"
type = "mcp"

[capabilities]
tools = [
  "container.list",
  "container.inspect",
  "container.restart"
]

[permissions]
docker_socket = true
network = false
filesystem = false

[compatibility]
argus_min = "0.1.0"
```

The manifest is descriptive, not an authorization grant. The Policy Engine remains authoritative.

## 12. Persistence Model

LanceDB is the selected ARGUS persistence substrate.

The domain model should use deterministic identifiers and preserve provenance so that observations, incidents, actions and historical evidence can be associated with stable entities.

The data model should distinguish:

```text
Entity records
Observations
Events
Evidence
Embeddings
Plans
Actions
Executions
Incidents
Policies
Agent memory
```

Vector similarity must never be the sole basis for an authorization or destructive operational decision; deterministic IDs, typed fields and explicit policy evaluation remain authoritative.

## 13. Telemetry Architecture

ARGUS will use `tracing` and OpenTelemetry as the initial observability foundation.

The long-term architecture should allow ARGUS to become a telemetry platform itself:

```text
Linux Kernel
  │
  ├── procfs/sysfs
  ├── PSI
  ├── Netlink
  ├── eBPF
  └── kernel events
       │
       ▼
ARGUS Telemetry Engine
       │
       ├── metrics
       ├── logs
       ├── traces
       ├── events
       └── profiles
       │
       ▼
OpenTelemetry
       │
       └── future ARGUS telemetry ecosystem
```

## 14. What ARGUS Should Build vs Reuse

### Reuse

- Ratatui
- Crossterm
- Tokio
- rmcp
- sysinfo where useful
- mcp-sysinfo as bundled MCP
- Bollard / docker-mcp where appropriate
- kube-rs for Kubernetes API access
- zbus for systemd/D-Bus integration
- OpenTelemetry
- tracing
- Cedar policy engine
- libbpf-rs/libbpf ecosystem for BPF telemetry

### Build in Rust

- ARGUS domain model
- ARGUS discovery coordinator
- Infrastructure graph/correlation engine
- `argus-executor`
- privileged `argusd`
- capability registry
- plugin registry and lifecycle
- policy integration layer
- action validation/rollback framework
- autonomous control-loop engine
- environment assessment
- plan representation
- incident model
- cross-layer Linux/Kubernetes correlation
- ARGUS TUI application logic

## 15. Future High-Value Capabilities

The domain model intentionally leaves room for:

- eBPF-based continuous kernel telemetry
- syscall anomaly detection
- automatic service dependency mapping
- network topology discovery
- process-to-container-to-Pod tracing
- storage dependency tracing
- resource pressure forecasting
- kernel configuration drift detection
- capability/privilege drift detection
- filesystem mutation analysis
- automated incident reconstruction
- deterministic change impact analysis
- autonomous rollback
- fault-injection/resilience testing
- fleet-level operations

## 16. Core Invariant

The most important ARGUS invariant is:

> **Every autonomous action must be explainable in terms of observed evidence, a declared capability, an independent policy decision, an execution result, and post-action validation.**

The LLM can propose hypotheses and plans. It does not become the source of truth, authorization system, or privileged execution engine.
