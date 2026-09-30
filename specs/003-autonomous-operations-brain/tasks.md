# Tasks: ARGUS Autonomous Operations Brain

**Spec:** `specs/003-autonomous-operations-brain/spec.md`
**Plan:** `specs/003-autonomous-operations-brain/plan.md`

## Task Rules

- Keep tasks independently implementable where possible.
- Identify the affected crate/component.
- Mark security-sensitive work explicitly.
- Do not bypass established domain contracts.
- Tests are tasks, not optional follow-up work (Constitution P13, AGENTS.md).

## Capability → milestone mapping

| Milestone | Capabilities (SPEC) | FRs |
|---|---|---|
| M1 | CAP-1..4 | FR-001..005 |
| M2 | CAP-5..10 | FR-006..011 |
| M3 | CAP-14 (host), CAP-15 | FR-016 (host), FR-017 |
| M4 | CAP-12..14 | FR-014..016 |
| M5 | CAP-11, CAP-18 | FR-012..013 |
| M6 | CAP-16..24 | FR-018..025 |

## Tasks

### Phase 1 — Milestone 1: Environment Intelligence (CAP-1..4)

- [ ] T001 [P0] Confirm ADRs 0032 and constitutional invariants (kernel-native, graceful degradation)
- [ ] T002 [P0] Add graph/situation/baseline/risk/prediction/change/runbook typed entities + `AutonomyMode` L0–L5 to `crates/argus-domain` (data-model.md §1–§7)
- [ ] T003 [P1] Create `crates/argus-sensors` with the `Sensor` trait and `/proc` readers: `meminfo`, `loadavg`, `vmstat`, `stat` (cpu), `pressure` (PSI)
- [ ] T004 [P1] Add `crates/argus-sensors` readers for `/sys` (block devices, thermal) and cgroup v2 (current/stat)
- [ ] T005 [P0] Unit-test sensors against `/proc`-tree fixtures (no live host required)
- [ ] T006 [P1] Create `crates/argus-observe` with the observation coordinator (periodic + event-driven schedule, per-sensor degradation)
- [ ] T007 [P1] Implement the live process inventory in `argus-observe` (PID/PPID/user/exe/cmdline/hash/cpu/mem/threads/fds/sockets/ports/caps/namespaces/cgroup/container/unit/start/lifecycle) with lifecycle diffing
- [ ] T008 [P1] Implement process anomaly detection (unexpected listener/binaries, explosions, churn, runaway) as evidence-backed anomaly events
- [ ] T009 [P1] Create `crates/argus-systemd` (zbus) and `crates/argus-container` (runtime adapters) for service + container monitoring (state, failures, restart loops, restart counts)
- [ ] T010 [P1] Implement baseline generation + deviation detection (static rules + historical + multi-signal) in `crates/argus-anomaly`
- [ ] T011 [P0] Emit observation/sensor/baseline events per `contracts/events.md` in `crates/argus-events`
- [ ] T012 [P1] Integration tests: observation loop, process-inventory diff, baseline deviation (deterministic, fake decision engine)
- [ ] T013 [P0] Security tests: no arbitrary command execution; readers hold no privileges

### Phase 2 — Milestone 2: Operational Brain (CAP-5..10)

- [ ] T014 [P1] Create `crates/argus-correlate`: typed environment graph (nodes/edges) + event→situation correlation
- [ ] T015 [P1] Add anomaly detection (multi-signal) to `argus-anomaly`; create `crates/argus-risk` (advisory risk signals)
- [ ] T016 [P1] Create `crates/argus-incidents`: incident lifecycle (open→investigating→mitigated→resolved→closed) with deduplication
- [ ] T017 [P1] Create `crates/argus-investigate`: investigation state machine + RCA (hypothesis generation via DecisionEngine; deterministic testing/elimination)
- [ ] T018 [P0] Security tests: risk signals and investigation output never authorize; no model→command path

### Phase 3 — Milestone 3: Autonomous Remediation (CAP-14 host, CAP-15)

- [ ] T019 [P0] Register typed remediation capabilities (`host.process.signal`, `container.restart`, resource-control) in `crates/argus-executor` with risk class + blast radius + guardrails
- [ ] T020 [P0] Implement resource-autopilot governance (criticality, budgets, quotas, rollback, rate limiting) in `crates/argus-policy`
- [ ] T021 [P0] Wire remediation loop + rollback + validation in `crates/argus-daemon`
- [ ] T022 [P0] Security/policy tests: deny-by-default, approval gating, executor no-bypass, rollback

### Phase 4 — Milestone 4: Kubernetes Brain (CAP-12..14)

- [ ] T023 [P1] Create `crates/argus-kubernetes` (kube-rs, optional provider, graceful degradation)
- [ ] T024 [P1] Implement k8s resource monitoring + pod→container→cgroup→PID correlation
- [ ] T025 [P1] Implement deterministic troubleshooting evidence bundles (CrashLoop/OOM/ImagePull/Pending)
- [ ] T026 [P0] Register typed self-healing capabilities (`k8s.*`) + security tests (no kubectl, deny-by-default for drain/reschedule)

### Phase 5 — Milestone 5: Predictive Operations (CAP-11, CAP-18)

- [ ] T027 [P1] Implement trend analysis + capacity/failure prediction (labeled, with uncertainty) in `argus-anomaly`/`argus-risk`
- [ ] T028 [P1] Implement change intelligence (git/deploy/image/config/k8s/terraform/cloud/package/kernel/service-config) in `argus-correlate`

### Phase 6 — Milestone 6: Adaptive ARGUS (CAP-16..24)

- [ ] T029 [P1] Create `crates/argus-memory` (working/operational/episodic/semantic/procedural layers, deterministic)
- [ ] T030 [P1] Create `crates/argus-runbooks` (declarative runbooks + gated promotion)
- [ ] T031 [P1] Implement incident similarity (typed fields) + impact simulation (blast radius, recovery time, alternative)
- [ ] T032 [P1] Create `crates/argus-reporting` (real-time/daily/weekly/security/capacity/root-cause/postmortem, no invented metrics)
- [ ] T033 [P1] Implement sentinel mode + escalation decision (OBSERVE/EXPLAIN/RECOMMEND/ASK-HUMAN/AUTO-FIX) + autonomy L0–L5 gating in `argus-daemon`
- [ ] T034 [P1] Implement self-observability (CAP-24) in `argus-observability`/`argus-daemon`
- [ ] T035 [P0] End-to-end success-signal scenario + full failure/security/deterministic-AI test suites

## Dependencies

```text
M1 (sensors → observe → baselines)
  └─► M2 (graph → correlation → anomaly/risk → incidents → investigation)
        ├─► M3 (typed remediation + autopilot)      [parallel]
        ├─► M4 (Kubernetes provider)                 [parallel]
        └─► M5 (prediction + change intelligence)    [parallel, needs M2 baselines + graph]
              └─► M6 (memory + runbooks + learning + sentinel + autonomy)
```

- M2 depends on M1 (correlation consumes observations/events; baselines feed anomaly).
- M3 and M4 are parallel after M2 (disjoint: host remediation vs k8s provider).
- M5 depends on M2 (baselines + graph) but not on M3/M4.
- M6 depends on all prior milestones (evidence histories feed memory/learning).

## Parallel execution examples

- **Within M1:** T003/T004 (sensor readers) and T002 (domain entities) in parallel
  (disjoint files/crates); T006/T007 (coordinator + inventory) after T003.
- **Within M2:** T014 (graph) and T016 (incidents) in parallel after T002.
- **Across milestones:** M3 and M4 proceed in parallel after M2.

## Implementation strategy (MVP first)

1. **MVP = M1**: kernel-native observation + process inventory + baselines with a
   deterministic, fixture-tested sensor stack. No execution, no model.
2. **Brain = +M2**: graph, correlation, incidents, investigation → evidence-backed
   root cause with a fake decision engine.
3. **Safety = +M3/+M4**: typed remediation and Kubernetes self-healing behind
   policy + executor.
4. **Prediction = +M5**: labeled forecasts + change intelligence.
5. **Adaptive = +M6**: memory, runbooks, learning, sentinel, autonomy, reports.

## Verification Checklist

- [ ] Constitution satisfied (all 18 principles; see plan.md §13)
- [ ] New ADRs 0032–0038 satisfied
- [ ] No unauthorized privilege expansion
- [ ] Domain model remains infrastructure-agnostic (no store/k8s types in `argus-domain`)
- [ ] Tests pass (`cargo test --workspace`)
- [ ] Telemetry present (logs/traces/metrics/audit)
- [ ] Rollback/failure behavior documented and tested
