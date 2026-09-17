# ARGUS AI — C4 Solution Architecture

- **Status:** Proposed / Target Architecture
- **Date:** 2026-09-16
- **Architecture scope:** Linux/Unix first; Kubernetes optional
- **Primary language:** Rust
- **TUI:** Ratatui
- **MCP:** Native `rmcp` + external MCP servers
- **State:** LanceDB
- **Eventing:** Internal event bus + optional native NATS/JetStream adapter
- **Policy:** Cedar + ARGUS policy layer
- **Observability:** `tracing` + OpenTelemetry

## 1. Architectural Intent

ARGUS AI is an **autonomous infrastructure operations runtime for Linux/Unix**.

It continuously discovers an environment, builds an evidence-backed operational model, detects deviations and incidents, reasons about causes, creates plans, evaluates policy, executes authorized actions, validates outcomes, and learns from the resulting evidence.

ARGUS is deliberately **not**:

- a Kubernetes-only platform;
- a generic agent orchestration framework;
- a traditional monitoring product;
- a replacement for the Linux kernel, systemd, Docker, Kubernetes or cloud APIs.

ARGUS is the **autonomous control and reasoning layer above the operating system and infrastructure control planes**.

The primary architectural principle is:

```text
Observe → Correlate → Understand → Plan → Authorize → Execute → Validate → Learn
```

The architecture is designed to operate from kernel-level facts upward:

```text
Hardware / VM
      ↓
Linux Kernel
      ↓
/proc / /sys / cgroups / namespaces / netlink / pidfd / eBPF
      ↓
Processes / resources / network / filesystems
      ↓
systemd / container runtime / host services
      ↓
Containers / workloads
      ↓
Kubernetes control plane (optional)
      ↓
Applications / dependencies
      ↓
Business-facing services
```

## 2. C4 Model Overview

This document uses the four C4 abstraction levels:

1. **System Context** — ARGUS and its users/external systems.
2. **Containers** — deployable/runtime boundaries inside ARGUS.
3. **Components** — major logical components inside the core runtime.
4. **Code-level guidance** — Rust crate/module boundaries and interfaces sufficient to drive implementation.

An additional deployment view is included because ARGUS is a privileged infrastructure runtime and trust boundaries are part of the architecture.

---

# Level 1 — System Context

## 3. Context Diagram

```plantuml
@startuml ARGUS-System-Context
!include https://raw.githubusercontent.com/plantuml-stdlib/C4-PlantUML/master/C4_Context.puml
LAYOUT_WITH_LEGEND()

Person(operator, "Infrastructure Operator", "Human who installs, configures, reviews and governs ARGUS")
Person(developer, "Developer / Platform Engineer", "Developer responsible for applications, infrastructure and automation")

System(argus, "ARGUS AI", "Autonomous infrastructure operations runtime for Linux/Unix")

System_Ext(llm, "AI Model Providers", "OpenAI, Anthropic, Gemini, OpenRouter, Ollama and compatible providers")
System_Ext(github, "GitHub / Git Providers", "Source code, commits, CI/CD, release and deployment metadata")
System_Ext(cloud, "Cloud / Infrastructure APIs", "AWS, GCP, OCI, Cloudflare and other provider APIs")
System_Ext(nats, "NATS / JetStream", "Optional distributed event transport")
System_Ext(telemetry, "Telemetry Ecosystem", "OpenTelemetry collectors, Grafana, Prometheus, Loki, future ARGUS telemetry products")
System_Ext(k8s, "Kubernetes Control Plane", "Optional workload orchestration control plane")
System_Ext(mcp, "Community / External MCP Servers", "Optional pluggable MCP capabilities")

Rel(operator, argus, "Installs, configures, observes, approves and governs")
Rel(developer, argus, "Investigates incidents, reviews plans and operational changes")
Rel(argus, llm, "Sends reasoning requests / receives structured decisions")
Rel(argus, github, "Reads source/deployment context and optionally manages engineering workflows")
Rel(argus, cloud, "Discovers and manages external infrastructure")
Rel(argus, nats, "Publishes/subscribes to events when enabled")
Rel(argus, telemetry, "Emits logs, metrics and traces; consumes selected telemetry")
Rel(argus, k8s, "Discovers and operates Kubernetes resources when present")
Rel(argus, mcp, "Discovers and uses pluggable tools/capabilities")

@enduml
```

## 4. Context Responsibilities

### Human actors

**Infrastructure Operator** controls installation, AI provider configuration, autonomy mode, policies, approvals, access to secrets, and operational boundaries.

**Developer / Platform Engineer** interacts with incidents, plans, remediation history, application deployments, Git repositories and engineering workflows.

### External systems

ARGUS must remain functional on a single Linux host even when all optional external systems are absent. The minimum viable environment is the Linux host itself plus the configured AI provider(s).

External services are adapters, not architectural dependencies of the core domain.

---

# Level 2 — Container Architecture

## 5. Container Diagram

The term **container** here follows C4 semantics: a deployable/runtime boundary, not a Docker container.

```plantuml
@startuml ARGUS-Containers
!include https://raw.githubusercontent.com/plantuml-stdlib/C4-PlantUML/master/C4_Container.puml
LAYOUT_WITH_LEGEND()

Person(operator, "Infrastructure Operator")

System_Boundary(argus, "ARGUS AI") {
    Container(cli, "ARGUS CLI / TUI", "Rust + Ratatui", "Installation, configuration, dashboards, incidents, plans, approvals and administration")
    Container(core, "ARGUS Core", "Rust + Tokio", "Domain model, lifecycle, operational control loops and control plane")
    Container(discovery, "Discovery Engine", "Rust", "Builds inventory and topology from Linux, Unix, containers, Kubernetes and external systems")
    Container(observation, "Observation & Correlation Engine", "Rust", "Normalizes signals and correlates entities, metrics, events, topology and evidence")
    Container(planner, "Planning & Reasoning Gateway", "Rust", "Builds context and invokes configured AI model providers; converts reasoning into typed intents/plans")
    Container(policy, "Policy & Authorization Engine", "Rust + Cedar", "Evaluates capabilities, risk, permissions, blast radius and approval requirements")
    Container(executor, "Privileged Executor", "Rust", "Typed and policy-authorized execution against Linux, Unix and infrastructure APIs")
    Container(agent, "Operational Capability Runtime", "Rust", "Hosts operational roles/skills without becoming a generic agent orchestration framework")
    Container(mcp, "MCP Runtime", "Rust + rmcp", "Native MCP client/server capability and lifecycle management")
    Container(plugin, "Plugin Manager", "Rust", "Discovers, validates, loads and manages extensible plugins and capability manifests")
    Container(state, "Operational State Store", "LanceDB", "Evidence, environment model, topology, incidents, plans, executions, historical state and semantic retrieval")
    Container(events, "Event Bus", "Rust async channels + NATS adapter", "Internal event distribution with optional durable/distributed NATS JetStream transport")
    Container(telemetry, "ARGUS Telemetry", "Rust + OpenTelemetry", "Logs, traces, metrics and future telemetry products")
    Container(secrets, "Secret & Credential Manager", "Rust adapters", "Protects provider keys, credentials and integration secrets")
}

System_Ext(llm, "AI Model Providers")
System_Ext(github, "GitHub / Git")
System_Ext(cloud, "Cloud APIs")
System_Ext(k8s, "Kubernetes API")
System_Ext(extmcp, "External MCP Servers")
System_Ext(nats, "NATS / JetStream")
System_Ext(otel, "OpenTelemetry Ecosystem")
System_Ext(host, "Linux/Unix Host + Kernel")

Rel(operator, cli, "Uses")
Rel(cli, core, "Controls")
Rel(core, discovery, "Coordinates")
Rel(core, observation, "Consumes operational state")
Rel(core, planner, "Requests diagnosis / plans")
Rel(core, policy, "Requests authorization")
Rel(core, executor, "Requests authorized actions")
Rel(core, agent, "Loads operational capabilities")
Rel(core, events, "Publishes/subscribes")
Rel(core, state, "Reads/writes domain state")
Rel(core, telemetry, "Emits telemetry")
Rel(discovery, host, "Discovers host state")
Rel(discovery, k8s, "Discovers Kubernetes resources")
Rel(discovery, cloud, "Discovers provider resources")
Rel(observation, host, "Reads low-level evidence")
Rel(observation, state, "Persists observations/evidence")
Rel(planner, llm, "Structured model inference")
Rel(planner, state, "Retrieves context / history")
Rel(policy, state, "Reads policies / environment context")
Rel(executor, host, "Performs authorized host operations")
Rel(executor, k8s, "Operates Kubernetes when authorized")
Rel(executor, cloud, "Operates external infrastructure when authorized")
Rel(mcp, extmcp, "Invokes / hosts MCP capabilities")
Rel(plugin, mcp, "Installs/manages MCP plugins")
Rel(plugin, agent, "Installs/manages capability plugins")
Rel(events, nats, "Optional durable/distributed transport")
Rel(telemetry, otel, "OTLP / telemetry export")
Rel(secrets, llm, "Supplies credentials securely")
Rel(secrets, cloud, "Supplies credentials securely")
Rel(secrets, github, "Supplies credentials securely")

@enduml
```

## 6. Container Responsibilities

### ARGUS CLI / TUI

The user-facing interface. Built in Rust with Ratatui.

Responsibilities:

- first-run setup;
- AI provider configuration;
- discovery progress;
- infrastructure overview;
- active capability status;
- incidents;
- plans;
- policy and autonomy settings;
- approval workflows;
- audit and execution history;
- diagnostics;
- upgrades.

The TUI contains presentation logic only. It must not directly perform privileged operations.

### ARGUS Core

The central control-plane runtime.

Responsibilities:

- domain lifecycle;
- control loop coordination;
- resource identity;
- state transitions;
- scheduled and event-driven workflows;
- capability registration;
- incident lifecycle;
- plan lifecycle;
- autonomy modes;
- coordination between discovery, observation, planner, policy and executor.

ARGUS Core does not contain arbitrary infrastructure-specific logic that belongs in adapters/plugins.

### Discovery Engine

Responsible for discovering the environment from bottom up and top down.

Sources:

```text
Linux kernel / procfs / sysfs
cgroups v2
namespaces
netlink
pidfd
systemd / D-Bus
Docker / containerd / CRI
Kubernetes API
cloud APIs
filesystem
configuration files
MCP Core
community plugins
```

### Observation & Correlation Engine

Converts raw observations into canonical evidence and relationships.

It maintains the distinction between:

```text
Observed state
Inferred state
Desired state
Historical state
```

This is where causal correlation begins.

### Planning & Reasoning Gateway

ARGUS deliberately separates **AI reasoning** from **agent orchestration**.

This container:

1. builds a bounded context;
2. retrieves relevant evidence/history;
3. calls the selected model provider;
4. requires structured output;
5. validates model output against the domain schema;
6. returns an Intent / Hypothesis / Plan proposal.

It cannot directly invoke privileged infrastructure operations.

### Policy & Authorization Engine

The hard security boundary between reasoning and execution.

Implemented using Cedar plus ARGUS-specific policy/context logic.

Evaluation model:

```text
principal
+ action
+ resource
+ context
+ risk
+ blast radius
→ Allow / Deny / Require Approval
```

### Privileged Executor

The only ARGUS component allowed to cross the privileged execution boundary.

Operations are typed, for example:

```text
RestartService
TerminateProcess
ScaleWorkload
ModifyCgroup
CreateRoute
BlockEndpoint
RollbackDeployment
RotateCertificate
CreateBackup
DeployArtifact
```

Arbitrary shell is an escape hatch, not the primary action model.

### Operational Capability Runtime

Hosts operational capabilities such as:

```text
SRE
DevOps
SecOps
Network
Database
Capacity
Backup
Engineering
```

These are capability domains rather than independent infrastructure control planes.

### MCP Runtime

Uses `rmcp` for native MCP support and supports external MCP servers.

The MCP Core is distributed with ARGUS. Community MCPs can be packaged as plugins.

### Plugin Manager

Handles the plug-and-play ecosystem:

```text
Discover
Validate manifest
Validate compatibility
Validate permissions
Install
Enable
Disable
Upgrade
Rollback
Remove
```

### Operational State Store

LanceDB stores the environment model, observations/evidence, incidents, plans and historical operational knowledge.

The persistence model stays behind a repository abstraction so other operational stores can be introduced later.

### Event Bus

Primary mode:

```text
Tokio mpsc / broadcast / watch
```

Optional distributed mode:

```text
NATS / JetStream
```

ARGUS must run with no NATS dependency on a single host.

### ARGUS Telemetry

Native `tracing` and OpenTelemetry instrumentation. This layer intentionally leaves room for a future ARGUS telemetry ecosystem.

---

# Level 3 — Component Architecture

## 7. ARGUS Core Components

```plantuml
@startuml ARGUS-Core-Components
!include https://raw.githubusercontent.com/plantuml-stdlib/C4-PlantUML/master/C4_Component.puml
LAYOUT_WITH_LEGEND()

Container_Boundary(core, "ARGUS Core") {
    Component(domain, "Domain Model", "Rust", "Canonical entities, identities and state transitions")
    Component(control, "Control Loop", "Rust + Tokio", "Observe, correlate, reason, authorize, execute and validate")
    Component(registry, "Capability Registry", "Rust", "Registers capabilities and required privileges")
    Component(incident, "Incident Manager", "Rust", "Incident lifecycle and evidence timeline")
    Component(plan, "Plan Manager", "Rust", "Plan graph, preconditions, rollback and expected outcomes")
    Component(autonomy, "Autonomy Manager", "Rust", "Controls autonomy level and approval modes")
    Component(inventory, "Environment Graph", "Rust", "Resource graph and dependency relationships")
    Component(repository, "Domain Repository", "Rust", "Persistence abstraction over LanceDB and future backends")
    Component(events, "Event Coordinator", "Rust + Tokio", "Internal event propagation and durable adapter integration")
}

Container_Ext(discovery, "Discovery Engine")
Container_Ext(observation, "Observation & Correlation Engine")
Container_Ext(planner, "Planning & Reasoning Gateway")
Container_Ext(policy, "Policy Engine")
Container_Ext(executor, "Privileged Executor")
Container_Ext(mcp, "MCP Runtime")
Container_Ext(telemetry, "Telemetry")

Rel(control, domain, "Creates/transitions")
Rel(control, inventory, "Updates")
Rel(control, incident, "Opens/updates/resolves")
Rel(control, plan, "Creates/executes plan lifecycle")
Rel(control, autonomy, "Applies autonomy mode")
Rel(control, registry, "Resolves capabilities")
Rel(control, repository, "Reads/writes")
Rel(control, events, "Publishes/subscribes")
Rel(discovery, inventory, "Provides resources")
Rel(observation, inventory, "Provides correlated evidence")
Rel(observation, repository, "Stores evidence")
Rel(planner, plan, "Produces plan proposals")
Rel(policy, autonomy, "Evaluates action permissions")
Rel(executor, plan, "Executes typed actions")
Rel(mcp, registry, "Registers capabilities")
Rel(events, telemetry, "Emits operational events")

@enduml
```

## 8. Discovery Components

```text
argus-discovery
├── linux
│   ├── procfs
│   ├── sysfs
│   ├── cgroups
│   ├── namespaces
│   ├── pidfd
│   ├── psi
│   ├── netlink
│   ├── mounts
│   └── kernel_features
│
├── systemd
│   └── D-Bus / zbus
│
├── containers
│   ├── bollard / Docker
│   ├── containerd adapter
│   └── CRI adapter
│
├── kubernetes
│   └── kube-rs / API watches
│
├── cloud
│   ├── AWS
│   ├── GCP
│   └── OCI
│
└── mcp
    └── MCP Core + community plugins
```

### Discovery modes

The engine supports three modes:

**Snapshot** — one complete inventory scan.

**Watch** — subscribe to event streams and update state incrementally.

**Probe** — targeted investigation initiated by an incident or hypothesis.

---

# Level 3 — Linux Kernel Domain Architecture

## 9. Kernel-Native Capability Stack

```text
┌───────────────────────────────────────────────────────────┐
│                    ARGUS DOMAIN MODEL                     │
├───────────────────────────────────────────────────────────┤
│ Host │ Process │ Service │ Container │ Workload │ Network │
└──────────────────────────┬────────────────────────────────┘
                           │
┌──────────────────────────▼────────────────────────────────┐
│                LINUX ADAPTER / OBSERVATION                │
├───────────────────────────────────────────────────────────┤
│ procfs  sysfs  cgroupv2  namespace  netlink  pidfd       │
│ PSI     mounts sockets   capabilities eBPF/BTF           │
└──────────────────────────┬────────────────────────────────┘
                           │
┌──────────────────────────▼────────────────────────────────┐
│                       LINUX KERNEL                        │
└───────────────────────────────────────────────────────────┘
```

## 10. `/proc` adapter

Primary use:

```text
process identity
process hierarchy
command lines
FDs
mount namespaces
network namespaces
cgroup membership
memory
virtual memory
scheduler information
kernel runtime state
```

ARGUS should prefer authoritative kernel state over parsing human-oriented command output.

## 11. `/sys` adapter

Primary use:

```text
CPU topology
NUMA
block devices
PCI/USB
network device topology
power state
kernel subsystem attributes
hardware relationships
```

## 12. cgroup adapter

cgroup identity is the primary correlation bridge for containers and workloads.

ARGUS should model:

```text
Cgroup
 ├── parent
 ├── children
 ├── processes
 ├── cpu controller
 ├── memory controller
 ├── io controller
 ├── pids controller
 ├── cpuset controller
 ├── events
 └── pressure
```

## 13. Namespace adapter

Namespace IDs become first-class correlation keys.

```text
Process
  ↓
PID namespace
  ↓
Network namespace
  ↓
Mount namespace
  ↓
User namespace
```

This allows ARGUS to bridge host-visible processes with isolated workloads.

## 14. Network adapter

The preferred flow is:

```text
Netlink / Generic Netlink
        ↓
Canonical Network Graph
        ↓
Interface → Address → Route → Neighbor → Socket
```

CLI parsing of `ip`, `ss`, `route`, etc. is fallback/diagnostic only.

## 15. pidfd adapter

Process operations should prefer pidfd-based handles where kernel support is available. This prevents race conditions caused by PID reuse and supports event-driven process lifecycle handling.

## 16. PSI adapter

PSI becomes a core signal in resource-health assessment:

```text
CPU PSI
Memory PSI
IO PSI
```

ARGUS should maintain both instantaneous samples and historical trends.

PSI is especially valuable for distinguishing high utilization from actual resource contention.

## 17. eBPF Kernel Telemetry Engine

Optional but strategic.

```text
argus-ebpf
   │
   ├── exec/exit
   ├── scheduler
   ├── filesystem
   ├── network
   ├── TCP
   ├── IO
   ├── memory
   └── security
        ↓
    ring buffer
        ↓
Observation Engine
```

This component must degrade cleanly when eBPF is unavailable.

---

# Level 3 — Kubernetes Domain Architecture

## 18. Kubernetes is a Domain Adapter, Not the Core

Kubernetes exists above Linux in the ARGUS model.

```text
ARGUS Domain
      │
      ├── Linux substrate
      │
      └── Kubernetes adapter
             │
             ├── API discovery
             ├── watches
             ├── resources
             ├── controllers
             ├── scheduling
             ├── nodes
             └── workloads
```

The Kubernetes adapter may use `kube-rs` and can optionally integrate with kagent where beneficial, but ARGUS does not depend on kagent for its core runtime.

## 19. Kubernetes correlation model

The key enterprise correlation path is:

```text
Kubernetes Cluster
  ↓
Node
  ↓
Pod
  ↓
Container
  ↓
Container Runtime
  ↓
Cgroup
  ↓
PID
  ↓
PID Namespace
  ↓
Network Namespace
  ↓
Socket / Filesystem / Kernel Resource
```

The reverse direction must also work:

```text
Kernel process
   ↓
cgroup
   ↓
container
   ↓
pod
   ↓
workload
   ↓
service
   ↓
application
```

This is one of the most strategically important capabilities of ARGUS.

## 20. Kubernetes resource domains

```text
Cluster
├── Namespace
├── Node
├── Workload
│   ├── Deployment
│   ├── StatefulSet
│   ├── DaemonSet
│   ├── Job
│   └── CronJob
├── Pod
├── Container
├── Service
├── Ingress / Gateway
├── ConfigMap
├── Secret
├── PVC / PV
├── NetworkPolicy
├── RBAC
├── CRD
└── Event
```

The adapter should use resource watches rather than repeated polling where practical.

---

# Level 3 — Policy and Execution Architecture

## 21. Execution Control Plane

```text
AI-generated proposal
        │
        ▼
Typed Action
        │
        ▼
Precondition validation
        │
        ▼
Risk / blast-radius calculation
        │
        ▼
Cedar + ARGUS Policy
        │
   ┌────┼──────────┐
 deny  approve  human approval
   │      │          │
   │      └────┬─────┘
   │           ▼
   │      Privileged Executor
   │           │
   │           ▼
   │       Linux / K8s / Cloud
   │           │
   └───────────┴───────────────┐
                               ▼
                           Evidence
                               │
                               ▼
                            Validate
```

## 22. Action model

Every action should contain:

```text
ActionId
Principal
Capability
Target
Arguments
Preconditions
RiskClass
BlastRadius
ExpectedOutcome
RollbackPlan
PolicyContext
ApprovalRequirement
Timeout
IdempotencyKey
```

This makes autonomous operation auditable and safe.

---

# Level 3 — Plugin Architecture

## 23. Plugin model

Plugins are discovered through a manifest such as:

```toml
[plugin]
name = "docker"
version = "1.0.0"
api_version = "1"
type = "mcp"

[capabilities]
tools = [
  "docker.list_containers",
  "docker.inspect_container",
  "docker.restart_container"
]

[permissions]
docker_socket = true
network = false
filesystem = false

[health]
check = "docker.health"
```

## 24. Plugin lifecycle

```text
Discover
   ↓
Parse manifest
   ↓
Verify artifact/signature
   ↓
Check compatibility
   ↓
Check declared permissions
   ↓
Sandbox if applicable
   ↓
Register capabilities
   ↓
Enable
   ↓
Observe health
   ↓
Upgrade / rollback / disable / remove
```

Plugins should never receive privileges implicitly.

---

# Level 3 — State and Knowledge Architecture

## 25. LanceDB role

LanceDB is the operational knowledge store for:

```text
resource observations
historical evidence
incidents
plans
execution traces
runbooks
semantic operational knowledge
retrieval context
agent memory
```

The domain model remains database-independent through repository traits.

### Logical collections

```text
resources
observations
evidence
relationships
incidents
plans
actions
executions
policies
runbooks
knowledge
agent_memory
```

Structured records remain authoritative. Embeddings are derived representations and must not become the only source of truth.

---

# Level 3 — Event Architecture

## 26. Event taxonomy

ARGUS uses typed domain events.

```text
environment.discovered
environment.changed
resource.discovered
resource.changed
resource.removed
health.degraded
incident.detected
incident.updated
incident.resolved
hypothesis.created
plan.created
plan.approved
plan.rejected
action.requested
action.approved
action.denied
action.started
action.completed
action.failed
plugin.loaded
plugin.failed
agent.capability.changed
policy.evaluated
telemetry.emitted
```

Internal mode uses Tokio channels. Distributed mode uses NATS/JetStream through a native adapter. The event schema must remain transport-independent.

---

# Level 3 — Observability Architecture

## 27. ARGUS observes itself

Every major operation emits:

```text
structured logs
metrics
traces
correlation IDs
execution IDs
incident IDs
```

Flow:

```text
ARGUS components
      ↓
tracing
      ↓
OpenTelemetry SDK
      ↓
OTLP
      ↓
Telemetry backend(s)
```

The architecture intentionally leaves room for an ARGUS-native telemetry platform later.

---

# Level 4 — Code Architecture / Rust Workspace

## 28. Proposed Cargo workspace

```text
argus-ai/
├── Cargo.toml
├── crates/
│   ├── argus-core/              # Domain model and lifecycle
│   ├── argus-domain/            # Entity/value object definitions
│   ├── argus-cli/               # CLI entry point
│   ├── argus-tui/               # Ratatui presentation layer
│   ├── argus-daemon/            # argusd privileged daemon
│   ├── argus-executor/          # Typed execution layer
│   ├── argus-policy/            # Cedar + ARGUS policies
│   ├── argus-discovery/         # Discovery orchestration
│   ├── argus-linux/             # Linux kernel/userspace adapters
│   ├── argus-systemd/           # systemd / D-Bus integration
│   ├── argus-docker/            # Docker/container adapters
│   ├── argus-kubernetes/        # Kubernetes adapter
│   ├── argus-mcp/               # rmcp runtime and MCP registry
│   ├── argus-plugin/            # Plugin manifests/lifecycle
│   ├── argus-agent/             # Operational capability runtime
│   ├── argus-planner/           # Planning/reasoning gateway
│   ├── argus-llm/               # ModelProvider abstraction
│   ├── argus-state/             # LanceDB repository implementation
│   ├── argus-events/            # Domain events + bus adapters
│   ├── argus-observability/     # tracing + OpenTelemetry
│   ├── argus-secrets/           # Secret providers
│   └── argus-install/           # Installer/update functionality
│
├── mcp/
│   ├── core/
│   └── community/
├── policies/
├── runbooks/
├── deploy/
│   └── systemd/
├── docs/
│   ├── adr/
│   └── architecture/
└── tests/
```

## 29. Core Rust contracts

The most important domain interfaces should resemble:

```rust
trait Resource {
    fn id(&self) -> ResourceId;
    fn kind(&self) -> ResourceKind;
}

trait DiscoverySource {
    async fn snapshot(&self) -> Result<DiscoverySnapshot>;
    async fn watch(&self) -> Result<DiscoveryStream>;
}

trait Capability {
    fn descriptor(&self) -> CapabilityDescriptor;
    async fn execute(&self, request: CapabilityRequest) -> Result<CapabilityResult>;
}

trait PolicyEvaluator {
    async fn evaluate(&self, request: AuthorizationRequest)
        -> Result<PolicyDecision>;
}

trait Executor {
    async fn execute(&self, action: AuthorizedAction)
        -> Result<ExecutionResult>;
}

trait ModelProvider {
    async fn structured(
        &self,
        request: ModelRequest,
    ) -> Result<ModelResponse>;
}

trait DomainRepository {
    async fn put_observation(&self, observation: Observation) -> Result<()>;
    async fn query_resources(&self, query: ResourceQuery) -> Result<Vec<Resource>>;
    async fn store_incident(&self, incident: Incident) -> Result<()>;
}
```

These are architectural contracts, not final APIs. Their purpose is to keep the core domain independent from Linux implementations, AI providers, databases and transport mechanisms.

---

# 30. Deployment Architecture

## Single-host installation

The reference installation is intentionally self-contained.

```text
Linux Host
│
├── /usr/bin/argus
├── /usr/bin/argusd
├── /opt/argus/mcp/
├── /etc/argus/
└── /var/lib/argus/

systemd
│
├── argusd.service
└── optional argus telemetry / helper units
```

The AI reasoning process must not have unrestricted root privileges.

Recommended boundary:

```text
argus TUI / CLI       unprivileged
       │
       ▼
argus AI / planner    unprivileged
       │
       ▼
Unix socket / IPC
       │
       ▼
argusd                 privileged, minimized
       │
       ▼
Linux kernel / systemd / runtime
```

## Kubernetes installation

When Kubernetes is detected:

```text
ARGUS
 │
 ├── host-side ARGUS daemon(s)
 │
 └── optional Kubernetes adapter / controller
             │
             ▼
        Kubernetes API
```

ARGUS may eventually deploy a cluster-level ARGUS component, but the core runtime must remain usable without Kubernetes.

---

# 31. Trust Boundaries

## Boundary 1 — AI / reasoning

Untrusted or probabilistic model output enters the system here.

**Rule:** model output is data, never authority.

## Boundary 2 — policy

Only policy-approved typed actions can move toward privileged execution.

## Boundary 3 — privileged executor

Only `argusd` crosses this boundary.

## Boundary 4 — plugin

Plugins are potentially third-party code and should be sandboxed or capability-restricted when possible.

## Boundary 5 — external infrastructure

Cloud APIs, GitHub, Kubernetes and external MCPs are untrusted integration surfaces.

---

# 32. Autonomous Control Loop

The final end-to-end loop is:

```text
                    ┌─────────────────────┐
                    │     ENVIRONMENT     │
                    └──────────┬──────────┘
                               │
                         Discovery /
                         Observation
                               │
                               ▼
                    ┌─────────────────────┐
                    │  Environment Model  │
                    │  + Evidence Graph   │
                    └──────────┬──────────┘
                               │
                         Correlation
                               │
                               ▼
                    ┌─────────────────────┐
                    │ Incident / Intent   │
                    └──────────┬──────────┘
                               │
                         AI Reasoning
                               │
                               ▼
                    ┌─────────────────────┐
                    │        Plan         │
                    └──────────┬──────────┘
                               │
                       Cedar + ARGUS
                           Policy
                               │
                   ┌───────────┴───────────┐
                   │                       │
                DENY                 APPROVE / HITL
                   │                       │
                   │                       ▼
                   │              ┌────────────────┐
                   │              │    Executor    │
                   │              └───────┬────────┘
                   │                      │
                   │                      ▼
                   │              Linux / K8s / Cloud
                   │                      │
                   └──────────────┬───────┘
                                  │
                               Evidence
                                  │
                                  ▼
                               Validate
                                  │
                           resolved / retry /
                            rollback / escalate
                                  │
                                  └────────────→ loop
```

# 33. Architectural Invariants

The implementation should preserve these invariants:

1. **ARGUS must run without Kubernetes.**
2. **AI output never directly executes privileged operations.**
3. **All privileged actions are typed and policy-controlled.**
4. **Observed evidence is immutable and distinct from inferred state.**
5. **Infrastructure technology-specific logic belongs in adapters/plugins wherever practical.**
6. **Linux kernel state is authoritative when directly available.**
7. **MCP is an extension mechanism, not the security boundary.**
8. **The MCP Core ships with the product.**
9. **The runtime must operate without NATS; NATS is an optional distributed transport.**
10. **The state model is database-independent even though LanceDB is the initial implementation.**
11. **eBPF is strategic but optional; ARGUS must degrade gracefully without it.**
12. **Kubernetes is a domain adapter/control plane, not the definition of ARGUS.**
13. **The plugin system must support community extension without modifying ARGUS core.**
14. **Security privileges must be minimized and explicitly declared.**
15. **Every autonomous action must produce an auditable execution record.**

# 34. Strategic Architectural Outcome

The target architecture makes ARGUS a **kernel-aware autonomous infrastructure control plane** rather than an AI-powered monitoring script.

The key differentiator is the ability to correlate infrastructure across layers:

```text
Business Service
       ↕
Application
       ↕
Kubernetes / Workload
       ↕
Container
       ↕
Runtime
       ↕
Cgroup / Namespace
       ↕
Process / Socket / Filesystem
       ↕
Linux Kernel
       ↕
Hardware / VM
```

That correlation graph is the foundation for enterprise-grade diagnosis, remediation, security, capacity management and future ARGUS telemetry products.
