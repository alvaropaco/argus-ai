# Implementation Plan: ARGUS Autonomous Operations Brain

**Spec:** `specs/003-autonomous-operations-brain/spec.md`
**Status:** Draft

## 1. Summary

Evolve ARGUS from a secure autonomous runtime into an autonomous operations
brain, in six milestones. The approach is layered and incremental, reusing the
existing foundation (specs 001/002, spec-argus) and adding new crates only where
architecture justifies them (per ADR-0032..0038). Key design choices:

- **Sensors are data, not decisions** (`argus-sensors` readers + `argus-observe`
  coordinator); correlation/anomaly/investigation are downstream.
- **The graph is a projection** over the canonical event/observation store
  (`argus-correlate`), never a separate database.
- **One stochastic step** remains: hypothesis generation and escalation inputs go
  through the existing `DecisionEngine` gateway; everything else is deterministic.
- **Kubernetes is optional** (`argus-kubernetes` behind the provider trait) and
  its absence never degrades the core.
- **Memory is deterministic** (`argus-memory`): semantic memory is typed facts,
  never embeddings.

### Milestone map

| Milestone | Delivers | Crates |
|---|---|---|
| M1 Environment Intelligence | observation, process inventory, services/containers, baselines, event stream | `argus-sensors`, `argus-observe`, `argus-systemd`, `argus-container`, `argus-anomaly` (baselines) |
| M2 Operational Brain | graph, correlation, anomaly, incidents, investigation, RCA | `argus-correlate`, `argus-anomaly`, `argus-risk`, `argus-investigate`, `argus-incidents` |
| M3 Autonomous Remediation | typed remediation, resource autopilot, rollback, validation | extend `argus-executor`, `argus-policy`, `argus-daemon` |
| M4 Kubernetes Brain | k8s discovery, troubleshooting, self-healing | `argus-kubernetes` |
| M5 Predictive Operations | trends, capacity/failure prediction, change intelligence | extend `argus-anomaly`/`argus-risk`, `argus-correlate` |
| M6 Adaptive ARGUS | memory, runbooks, learning, similarity, simulation, sentinel, reporting, autonomy | `argus-memory`, `argus-runbooks`, extend `argus-investigate`, `argus-daemon`, `argus-reporting` |

## 2. Architecture Impact

- **crates (new):** `argus-sensors`, `argus-observe`, `argus-correlate`,
  `argus-anomaly`, `argus-risk`, `argus-investigate`, `argus-incidents`,
  `argus-kubernetes`, `argus-systemd`, `argus-container`, `argus-network`,
  `argus-memory`, `argus-runbooks`, `argus-reporting`. `argus-ebpf` deferred.
- **crates (extended):** `argus-domain` (graph node/edge types, Situation,
  Baseline, Risk, Prediction, Change, Runbook, AutonomyMode L0–L5),
  `argus-executor` (new typed capabilities), `argus-policy` (resource
  governance), `argus-daemon` (loop driver, sentinel mode, self-observability),
  `argus-events` (new event families), `argus-state` (new collections).
- **domain contracts:** new typed entities; no backend/store types leak into
  `argus-domain`.
- **adapters/plugins:** sensor/reader modules, systemd (zbus), container
  runtimes, Kubernetes (kube-rs).
- **security boundaries:** unchanged and reinforced — sensors/providers are
  untrusted; every action crosses policy + typed executor.
- **state/persistence:** new collections behind `DomainRepository`.
- **event schemas:** observation/situation/baseline/risk/incident/investigation/
  prediction/change/runbook event families.
- **observability:** spans per loop iteration and investigation; self-metrics
  (CAP-24).
- **installation/deployment:** no new hard dependencies; kube-rs optional.

## 3. Proposed Design

```text
argus-sensors (kernel readers) ──► argus-observe (coordinator + process inventory)
                                        │ observations + events
                                        ▼
argus-correlate (graph + situations) ──► argus-anomaly (baselines + deviation)
                                        │                    │
                                        ▼                    ▼
argus-risk (advisory) ◄───────────── argus-incidents ──► argus-investigate (RCA)
                                        │                         │ candidate plan
                                        │                         ▼
                                        │                  argus-daemon (plan → policy → executor → validate)
                                        │                         │
argus-memory ◄──────────────────────────┴───────────────────── argus-runbooks
```

Alternatives considered (rejected): a single monolithic `argus-brain` crate
(couples unrelated concerns, harms testability and degradation); building the
graph as a graph database (violates "projection over the canonical store" and
the repository abstraction).

## 4. Interfaces and Contracts

- `argus-domain::graph` — typed node/edge enums (ADR-0033).
- `argus-domain::situation` / `baseline` / `risk` / `prediction` / `change` /
  `runbook` — new typed entities.
- `argus-domain::reasoning::AutonomyMode` — expanded to L0–L5 (ADR-0035).
- `argus-observe::Sensor` trait — `read() -> Result<Vec<Observation>>` plus a
  `degraded()` health hook.
- `argus-correlate::Graph` / `Correlator` — graph CRUD and situation folding.
- `argus-investigate::Investigator` — the investigation state machine.
- `contracts/events.md` — new event families; `contracts/capabilities.md` — new
  capability IDs.

## 5. Security Model

- **Trust boundaries:** sensors/providers (untrusted) → correlation/anomaly/risk
  (read-only) → investigation (proposes, data) → policy (decides) → executor
  (`argusd`, privileged) → host.
- **Required Linux capabilities:** readers need only file read on `/proc`,
  `/sys`, cgroup v2; `argus-observe` needs none ambient; the executor keeps the
  minimal set per action (ADR-008/021).
- **Privileged operations:** typed capabilities only (`host.process.signal`,
  `container.*`, `k8s.*`); never shell/kubectl; guardrail predicates at execution
  time (ADR-028).
- **Policy:** allow / deny / require approval; resource governance enforces
  criticality, protected resources, budgets, quotas, blast radius, rollback,
  rate limiting (CAP-15).
- **Approval:** bounded by autonomy level (ADR-035); cloud approval is input,
  never authority.
- **Plugin/MCP trust:** risk signals, predictions, and runbooks never authorize.

## 6. Data / State Model

See `data-model.md`. New entities persist behind `DomainRepository`; the interim
SQLite adapter (ADR-019) is extended with new collections. LanceDB remains the
target for operational memory (ADR-011/038) and is always behind the
abstraction; no vector/semantic search in the core.

## 7. Event Model

See `contracts/events.md`. New families: `sensor.*` (health), `situation.*`,
`baseline.*`, `risk.detected`, `incident.*`, `investigation.*`,
`prediction.*`, `change.detected`, `runbook.*`. Local bus default; NATS optional
(ADR-012).

## 8. Observability

- **Logs:** structured, correlation IDs across observe → correlate → investigate
  → plan → policy → execute → validate.
- **Traces:** spans per loop iteration and per investigation step.
- **Metrics (self-observability, CAP-24):** sensor health, event lag, decision
  latency, provider health, queue backlog, executor errors, failed validations,
  policy denials, autonomous-action success, false-positive rate, remediation
  success.
- **Audit:** append-only, per Principle 10.

## 9. Testing Strategy

- **unit:** sensor readers (against `/proc` fixtures), process-inventory diff,
  baseline deviation, graph queries, correlation, investigation state machine,
  risk evaluation, escalation decision, autonomy gating, validation.
- **integration:** observation loop, event flow, executor, policy; deterministic
  fake decision engine throughout.
- **Linux-specific:** systemd/container/Kubernetes tests skip gracefully when
  absent.
- **security/policy:** deny-by-default, no-bypass, no arbitrary command, no
  invalid capability registration, no approval bypass, no cloud-authority bypass.
- **failure/recovery:** unavailable provider, malformed output, missing sensors,
  unavailable Kubernetes, stale observations, executor/rollback/validation
  failure, event duplication/loss, state corruption.
- **deterministic AI:** all core control-plane behavior testable without a live
  LLM (Constitution P9/P13).

## 10. Rollout / Migration

- Additive: new crates, entities, events, capabilities; existing contracts
  remain compatible.
- `AutonomyMode` migrates from three variants to six (ADR-035); a compatible
  mapping is provided for persisted values.
- Optional components (Kubernetes, eBPF, decision sidecar) remain optional; the
  single-host build stays lean.
- No data migration beyond additive collections.

## 11. Risks

| Risk | Impact | Mitigation |
|---|---|---|
| Kernel interface variance across distros/kernels | sensors missing fields | tolerant readers; per-sensor degradation; fixtures |
| Process inventory cost at scale | observation lag | bounded sampling; lifecycle diffing; backpressure |
| Graph/correlation explosion | memory growth | bounded retention; projection rebuild |
| Hypothesis quality | wrong root cause | confidence + uncertainty; policy + approval; validation |
| Over-eager automation | unsafe autonomy | conservative default; risk class + blast radius bounds |
| kube-rs bloat | lean build lost | gate kube-rs behind the optional provider; feature flag |
| Self-modification risk | unsafe learned runbooks | gated promotion (ADR-036) |

## 12. ADR Impact

Existing ADRs are sufficient for the runtime/executor/policy/cloud. This phase
adds ADR-0032 (sensors), ADR-0033 (graph), ADR-0034 (investigation), ADR-0035
(autonomy L0–L5), ADR-0036 (skills vs runbooks), ADR-0037 (Kubernetes provider),
ADR-0038 (memory layers). No existing ADR requires amendment.

## 13. Constitution Check

| Principle | Status | Evidence |
|---|---|---|
| P1 Rust Core | PASS | all new crates in Rust |
| P2 Security Boundary | PASS | sensors/providers untrusted; policy + executor gate |
| P3 Policy Before Execution | PASS | allow/deny/require approval for every action |
| P4 Evidence Before Inference | PASS | observations immutable; graph derived; predictions labeled |
| P5 Kernel-Native | PASS | `/proc`/`/sys`/cgroup v2/PSI/Netlink, not CLI parsing |
| P6 Infra-Agnostic Core | PASS | K8s optional provider; core works without it |
| P7 Plug-and-Play | PASS | sensors/providers behind traits |
| P8 MCP Core | PASS | unchanged |
| P9 Deterministic Control Plane | PASS | no live LLM required for core behavior |
| P10 Auditable Autonomy | PASS | full audit per action |
| P11 Graceful Degradation | PASS | per-sensor and per-provider degradation |
| P12 Storage Independence | PASS | repository abstraction; no backend types in domain |
| P13 Testability | PASS | fixtures, fakes, mocks |
| P14 Least Privilege | PASS | readers need only file read |
| P15 Stable Core Contracts | PASS | additive, versioned contracts |
| P16 SpecKit/ADR Separation | PASS | new ADRs created, not silent changes |
| P17 No Generic Agent Framework | PASS | investigation is infrastructure-specific |
| P18 Autonomous Control Loop | PASS | explicit Observe→…→Learn stages |
