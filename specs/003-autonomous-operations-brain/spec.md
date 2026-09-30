# Feature Specification: ARGUS Autonomous Operations Brain

**Status:** Draft
**Spec ID:** 003
**Created:** 2026-09-30

> This spec realizes the capabilities distilled in the BMad contract
> `_bmad-output/specs/spec-autonomous-operations-brain/SPEC.md` (CAP-1..CAP-24),
> extending the foundation in specs 001/002 and spec-argus. CAP → FR mapping is
> noted inline. Architectural decisions are in ADR-0032..0038.

## 1. Problem Statement

The secure autonomous runtime exists — typed executor, deterministic policy,
structured-decision brain loop, durable state, and a cloud control plane that now
syncs correctly — but it has nothing to observe, no baseline of "normal", no graph
connecting a process to its pod to its deployment, no investigation machinery, and
no memory of what it has already diagnosed. The runtime can reason and execute but
cannot perceive, correlate, investigate, predict, or learn. Operators still get
"CPU = 92%" from fragmented tooling instead of an explanation and a safe
remediation.

## 2. Objective

Evolve ARGUS from a secure autonomous runtime into an autonomous infrastructure
operations brain: continuously observe the environment through kernel-native
interfaces, build baselines and a live environment graph, correlate events into
situations, detect anomalies and advisory risks, investigate to evidence-backed
root causes, predict failures, and (where policy permits) remediate through typed
capabilities — validating and learning from every outcome, all while preserving
the invariant "AI output is data, never authority."

## 3. Scope

### In scope

- [ ] Kernel-native observation: host resources, live process inventory, systemd
      services, containers (CAP-1..3).
- [ ] Operational baselines and multi-signal deviation detection (CAP-4).
- [ ] Operational environment graph and event correlation into situations
      (CAP-5..6).
- [ ] Advisory risk engine (CAP-7).
- [ ] Autonomous investigation engine and root-cause analysis (CAP-8..9).
- [ ] Incident management with deduplication (CAP-10).
- [ ] Predictive operations (CAP-11) and change intelligence (CAP-18).
- [ ] Kubernetes intelligence, autonomous troubleshooting, and typed self-healing
      (CAP-12..14).
- [ ] Resource autopilot (CAP-15).
- [ ] Operational memory layers (CAP-16), skills and runbooks (CAP-17).
- [ ] Impact simulation (CAP-19), autonomous reporting (CAP-20), sentinel mode
      (CAP-21).
- [ ] Autonomous escalation decision (CAP-22), autonomy model L0–L5 (CAP-23),
      self-observability (CAP-24).

### Out of scope

- A generic monitoring dashboard that only reports metrics.
- A general-purpose agent-orchestration framework.
- Arbitrary shell or `kubectl` execution.
- Kubernetes as a prerequisite.
- Fleet-level raw high-cardinality log ingestion and long-term analytics.
- Production-grade eBPF telemetry (eBPF is an optional enhancement layer).
- Full digital-twin simulation (only pre-action impact estimation is in scope).
- Uncontrolled self-modification of privileged policies or runbooks.
- Re-specifying the runtime, executor, policy, or cloud covered by specs 001/002
  and spec-argus.

## 4. Actors

- **Operator:** configures autonomy level, reviews/approves incidents and
  remediations, monitors the sentinel view and reports.
- **AI capability (decision engine):** scores structured `choice`/`score`/`noul`
  decisions (hypothesis generation, escalation inputs); it has no authority.
- **System/runtime (`argusd`/core):** runs the observation, correlation,
  investigation, remediation, and learning loops; enforces policy → executor.
- **Sensors/providers (plugins):** kernel readers, systemd/container/Kubernetes
  adapters; untrusted extension surfaces behind the capability boundary.

## 5. Functional Requirements

### Observation (CAP-1..3, ADR-0032)

**FR-001** The system MUST continuously observe host resources (CPU, memory,
swap, load, filesystem capacity/health, disk I/O, network traffic/interfaces/
packet errors/drops, PSI, kernel state, devices, temperatures, GPU, boot/reboot,
OOM events, cgroups, namespaces) through kernel interfaces (`/proc`, `/sys`,
cgroup v2, PSI, Netlink) and emit structured observations with source, timestamp,
and provenance. (CAP-1)

**FR-002** The system MUST maintain a live process inventory (PID, PPID, user,
executable, command line, exe path, binary hash, CPU/memory, threads, open files,
sockets, ports, capabilities, namespaces, cgroup, container association, systemd
unit, start time, parent/child, lifecycle) and detect unexpected processes/binaries,
process explosions, abnormal resource use, suspicious parent/child, unexpected
listeners, unusual network behavior, privilege anomalies, churn, and runaway
processes. (CAP-2)

**FR-003** The system MUST monitor systemd units (state, failures, restart loops,
startup time, dependencies, resource use, recent changes) and containers
(runtime, lifecycle, resource consumption, restart counts, network, filesystem,
processes, cgroups, namespaces). (CAP-3)

**FR-004** Observation MUST prefer a structured API over parsed human-oriented CLI
output; where a kernel interface is authoritative it MUST be used instead of
`ps`/`top`/`free`/`df`/`ip`. Each sensor MUST degrade independently; one failing
sensor MUST NOT break the observation loop.

### Baselines (CAP-4)

**FR-005** The system MUST generate historical baselines (CPU, memory, network,
disk I/O, process counts, service, container, Kubernetes workload, restart
frequency, resource consumption, application behavior) and detect deviations via
static rules + historical baselines + multi-signal correlation. A single high
sample MUST NOT become a critical incident on its own.

### Environment graph and correlation (CAP-5..6, ADR-0033)

**FR-006** The system MUST maintain a live environment graph correlating the
entity types and typed relationships defined in `data-model.md`, and answer
which-application-owns-this-process, which-deployment-owns-this-container,
which-host-process-drives-this-unhealthy-pod, what-depends-on-this-database, and
what-changed-before-this-incident. (CAP-5)

**FR-007** The system MUST correlate related events (Linux, systemd, processes,
containers, Kubernetes, cloud, application telemetry, deployments, configuration,
ARGUS itself) into operational situations by subject + temporal proximity + causal
linkage, so a deploy → restart → OOM → latency → incident chain becomes one
situation. (CAP-6)

### Anomaly and risk (CAP-7)

**FR-008** The system MUST detect reliability, security, capacity, and
configuration risks and emit them as typed, evidence-backed, advisory signals.
A risk signal MUST NEVER authorize, deny, or approve an action.

### Investigation and root cause (CAP-8..9, ADR-0034)

**FR-009** The system MUST run incident → evidence collection → hypothesis
generation → hypothesis testing → additional evidence → hypothesis elimination →
root cause → remediation plan, tracking supporting/contradicting evidence,
confidence, tests performed, conclusion, and unresolved uncertainty — and MUST
NOT fabricate certainty when evidence is insufficient. (CAP-8)

**FR-010** The system MUST determine what failed, why, what changed, what was
affected, blast radius, and what can safely be done, producing an evidence-backed
root cause and a *candidate* remediation that still crosses policy and the typed
executor. (CAP-9)

### Incidents (CAP-10)

**FR-011** The system MUST manage incidents (id, severity, status, start time,
affected resources, detection source, timeline, evidence, hypotheses, root cause,
actions, validation, resolution, impact, postmortem) through
open → investigating → mitigated → resolved → closed, deduplicating repeated
observations of the same underlying condition.

### Prediction and change intelligence (CAP-11, CAP-18)

**FR-012** The system MUST predict disk exhaustion, memory pressure, CPU
saturation, network saturation, Kubernetes capacity, workload growth, restart
trends, and recurring failures as explicitly-labeled predictions carrying
evidence and uncertainty; predictions MUST never be presented as fact. (CAP-11)

**FR-013** The system MUST correlate incidents with Git commits, deployments,
container images, configuration, Kubernetes, Terraform, cloud, package, kernel,
and service-config changes, and answer "what changed before the incident" with an
evidence-backed change hypothesis. (CAP-18)

### Kubernetes (CAP-12..14, ADR-0037)

**FR-014** As an optional provider, the system MUST monitor clusters, nodes,
namespaces, deployments, ReplicaSets, StatefulSets, DaemonSets, Jobs, CronJobs,
pods, containers, services, ingress, PVC/PV, HPA, PDB, NetworkPolicy, events,
requests/limits, scheduling, and readiness/liveness state, correlating Kubernetes
resources with Linux evidence (pod → container → cgroup → PID → namespace →
socket). Absence of the provider MUST NOT degrade the core. (CAP-12)

**FR-015** The system MUST deterministically gather evidence before any AI
reasoning for CrashLoopBackOff, OOMKilled, ImagePullBackOff, Pending, failed
probes, failed deployments, excessive restarts, node/disk/memory pressure,
scheduling, networking, DNS, service-discovery, PVC, and dependency failures.
(CAP-13)

**FR-016** The system MUST expose typed self-healing capabilities
(`k8s.pod.restart/delete`, `k8s.deployment.restart/rollback`, `k8s.workload.scale`,
`k8s.node.cordon/uncordon`, `k8s.job.cleanup`) through live-state → guardrails →
policy → authorization → executor → validation, never `kubectl` strings.
`k8s.node.drain` and `k8s.workload.reschedule` MUST remain deny-by-default.
(CAP-14)

### Resource autopilot (CAP-15)

**FR-017** The system MUST govern CPU, memory, I/O, network, cgroups, process
priority, container resources, and Kubernetes resources with criticality,
priorities, protected resources, budgets, quotas, blast radius, rollback, and rate
limiting; a memory-pressure scenario MUST protect a critical service, apply a
policy-approved adjustment to a non-critical consumer, and validate recovery.

### Memory, skills, runbooks (CAP-16..17, ADR-0036/0038)

**FR-018** The system MUST maintain working/operational/episodic/semantic/
procedural memory as structured deterministic records over the repository
abstraction, remembering incidents, root causes, remediations, topology,
baselines, runbooks, dependencies, and recurring failures; incident similarity
MUST be computed over typed fields, never embeddings. (CAP-16)

**FR-019** The system MUST run declarative runbooks (trigger, required evidence,
investigation steps, decision criteria, allowed actions, rollback, validation,
historical success rate); a runbook's allowed actions are candidate typed
capabilities that still cross policy. Learned skills/runbooks MUST pass
evaluation → simulation → validation → policy → approval → promotion before use.
(CAP-17)

### Simulation, reporting, sentinel (CAP-19..21)

**FR-020** The system MUST estimate, before risky operations, blast radius,
affected dependencies, resource impact, recovery time, service impact, and
rollback feasibility. (CAP-19)

**FR-021** The system MUST produce real-time, daily, weekly, security, capacity,
root-cause, and postmortem reports containing evidence, timelines, actions,
outcomes, and uncertainty, with no invented metrics or conclusions. (CAP-20)

**FR-022** The system MUST run a continuous 24/7 sentinel watch over processes,
services, containers, Kubernetes, network, resources, security signals,
dependencies, incidents, and changes, surfaced in the TUI/cloud as environment
health, risk, active incidents, pending approvals, recent actions, resource
pressure, and predicted failures. (CAP-21)

### Autonomy and escalation (CAP-22..23, ADR-0035)

**FR-023** The system MUST decide OBSERVE / EXPLAIN / RECOMMEND / ASK-HUMAN /
AUTO-FIX from evidence quality, confidence, reversibility, blast radius,
criticality, environment, historical success, autonomy mode, and policy — never
bypassing policy; a policy deny or require-approval MUST always win. (CAP-22)

**FR-024** The system MUST support autonomy levels L0 Observe through L5 Adaptive,
where a higher level grants more autonomy but never more privilege (same security
boundary, explicit policy, stronger validation, stronger audit); the level MUST
default to the most conservative setting. (CAP-23)

### Self-observability (CAP-24)

**FR-025** The system MUST track sensor health, event lag, investigation/decision
latency, provider health, memory, queue backlog, executor errors, failed
validations, policy denials, autonomous-action success rate, false-positive rate,
and remediation success, and MUST degrade safely rather than act on stale data.

## 6. Non-Functional Requirements

### Security

- Least privilege; sensors/providers receive no implicit privileges.
- Every privileged/self-healing action passes policy + typed executor.
- No arbitrary shell or `kubectl`; no model → command path; no cloud authority.
- Risk signals and predictions are advisory, never authorization.

### Reliability

- Graceful degradation when any optional component (Kubernetes, eBPF, cloud,
  decision sidecar) is absent.
- Deterministic core testable without a live LLM.
- Idempotent actions; rollback where possible; high-risk/irreversible
  deny-by-default or approval-required.

### Performance

- Observation, correlation, and investigation must not stall the control loop;
  context assembly and validation must be bounded.

### Observability

- Structured logs with correlation IDs spanning observe → correlate →
  investigate → plan → policy → execute → validate.
- OpenTelemetry traces per control-loop iteration and per investigation.
- Domain events for situations, incidents, hypotheses, risk signals, predictions,
  changes, and remediation outcomes.

## 7. Domain Impact

### Entities

- New: Situation, Baseline, Risk, Prediction, Change, Runbook, and the memory
  layers (see `data-model.md`). Existing Observation/Incident/Plan/Action/
  Execution/Hypothesis are extended (investigation evidence fields).

### Capabilities

- New typed capabilities: `host.process.signal`, `container.*`, `k8s.*`
  (see `contracts/capabilities.md`); existing `host.service.*` unchanged.

### Events

- New event families: observation/sensor, situation, baseline, risk, incident,
  investigation, prediction, change, runbook (see `contracts/events.md`).

### State transitions

- Incident: open → investigating → mitigated → resolved → closed (see
  `state-machines.md` in the BMad spec). Investigation loop and escalation
  decision states as defined there.

## 8. Infrastructure / Platform Impact

- Linux: new kernel-native readers (`/proc`, `/sys`, cgroup v2, PSI, Netlink);
  systemd via D-Bus; container runtimes via their structured APIs.
- Kubernetes: optional `kube-rs` provider; `kube-rs` gated so the single-host
  build stays lean (ADR-0037).
- eBPF: optional, deferred (enhancement layer).
- State: new logical collections behind `DomainRepository` (SQLite interim,
  LanceDB target); no backend types in the domain core.

## 9. Security and Policy

- **Required privileges:** sensors/readers require none beyond reading `/proc`,
  `/sys`, cgroup v2; the executor keeps the minimal capability set per action.
- **Capabilities:** every actionable operation is a registered typed capability
  with risk class, blast radius, and validation strategy (ADR-0027/0028).
- **Policy:** allow / deny / require approval, evaluated independently of model
  output; resource governance enforces criticality/budgets/quotas/rollback/rate
  limiting.
- **Approval:** anything above the autonomy threshold requires approval; cloud
  approval re-enters local policy as input, never authority.
- **Blast radius & rollback:** plans and runbooks declare both; irreversible or
  high-blast-radius actions are deny-by-default.
- **Plugin/MCP trust:** sensors/providers are untrusted extension surfaces; a
  risk signal or prediction never becomes an authorization.

## 10. Failure and Recovery

- Sensor unavailable/missing kernel file → sensor reports `degraded`, loop
  continues (ADR-0032).
- Kubernetes absent/unreachable → provider `degraded`, core unaffected.
- Decision sidecar absent → fail closed to `noul` (observe-only or escalate),
  never a default action.
- Malformed model output → rejected, logged; no action.
- Investigation with insufficient evidence → reports uncertainty, no fabricated
  root cause.
- Executor/rollback/validation failure → recorded, escalate; never silently loop.
- Stale observations → flagged and excluded from decisions; ARGUS degrades to a
  safe mode rather than acting on stale data (CAP-24).

## 11. Acceptance Criteria

- [ ] AC-001 — On a single Linux host, a running sensor emits structured
      observations sourced from `/proc`, `/sys`, cgroup v2, PSI with provenance,
      with no CLI parsing (CAP-1; deterministic, no live LLM).
- [ ] AC-002 — An injected unexpected listener is detected and emitted as an
      anomaly event with the inventory entry as evidence (CAP-2).
- [ ] AC-003 — A restart-looping service and a restarting container are each
      detected with restart counts and correlated process/cgroup evidence
      (CAP-3).
- [ ] AC-004 — A single transient CPU spike produces no incident while a
      sustained multi-signal deviation above baseline does (CAP-4).
- [ ] AC-005 — The graph answers all five reference questions (ownership,
      deployment-ownership, host-process-for-pod, dependency, change-before-
      incident) (CAP-5).
- [ ] AC-006 — The deploy → restart → OOM → latency → incident chain correlates
      into one situation (CAP-6).
- [ ] AC-007 — Crash-loop, unexpected-binary, future-disk-exhaustion, and
      unexpected-config-change risks are each detected and emitted as advisory
      signals that never authorize anything (CAP-7).
- [ ] AC-008 — A CPU-saturation incident is investigated to an evidence-backed
      root cause with stated confidence and uncertainty (CAP-8/9).
- [ ] AC-009 — Repeated observations deduplicate into one incident that advances
      through the lifecycle (CAP-10).
- [ ] AC-010 — Disk at 72% with 3.8 GB/day growth yields a labeled "~8.4 days"
      prediction with growth rate and uncertainty (CAP-11).
- [ ] AC-011 — A CrashLoopBackOff pod yields a deterministic evidence bundle
      before any AI reasoning; pod → container → cgroup → PID correlation holds
      (CAP-12/13).
- [ ] AC-012 — A policy-permitted `deployment.rollback` executes via the typed
      executor and is validated; a non-permitted one is denied at policy; no
      `kubectl` is ever executed (CAP-14).
- [ ] AC-013 — A memory-pressure scenario protects a critical service, applies a
      policy-approved adjustment, and validates recovery (CAP-15).
- [ ] AC-014 — A new incident is matched against prior similar incidents with
      cited evidence, computed over typed fields (CAP-16).
- [ ] AC-015 — A candidate learned skill cannot be promoted without the full
      evaluation → simulation → validation → policy → approval → promotion gate
      (CAP-17).
- [ ] AC-016 — "What changed before the incident" returns an evidence-backed
      change hypothesis (CAP-18); pre-action impact estimation returns blast
      radius, dependencies, recovery time, and a lower-impact alternative
      (CAP-19).
- [ ] AC-017 — A postmortem contains a sourced timeline, actions, and outcomes
      with no fabricated figures (CAP-20); the TUI/cloud sentinel shows live
      health, risk, incidents, approvals, and predictions (CAP-21).
- [ ] AC-018 — A high-confidence reversible low-blast-radius action auto-fixes
      in an autonomous mode, while an irreversible high-blast-radius action
      routes to ASK-HUMAN (CAP-22); raising the autonomy level never grants a
      new privilege path (CAP-23).
- [ ] AC-019 — A broken sensor or saturated queue is surfaced and the system
      degrades to a safe mode (CAP-24).
- [ ] AC-020 — Security suite green: no policy bypass, no arbitrary command
      execution, no privilege escalation, no invalid capability registration, no
      approval bypass, no cloud-authority bypass.

## 12. Architectural Constraints

- ADR-000 — product boundary; ADR-001 — Rust core; ADR-002 — Ratatui.
- ADR-008 — executor in Rust; ADR-009 — security boundary; ADR-010 — policy.
- ADR-012 — local event bus, NATS optional; ADR-013 — not a generic agent
  framework; ADR-014 — LLM provider abstraction.
- ADR-016 — plugin architecture; ADR-018 — kernel-native domain model.
- ADR-019 — SQLite interim state backend.
- ADR-027 — capability identifiers typed; ADR-028 — execution-time guardrails.
- ADR-031 — validator and separate learning pass.
- **ADR-032** — sensor subsystem; **ADR-033** — environment graph; **ADR-034** —
  investigation engine; **ADR-035** — autonomy L0–L5; **ADR-036** — skills vs
  runbooks; **ADR-037** — Kubernetes provider; **ADR-038** — memory layers.

## 13. Open Questions

- (Resolved in ADR-0032..0038 and the BMad spec; none outstanding for Milestone
  1. See `plan.md` §12 for ADR impact.)
