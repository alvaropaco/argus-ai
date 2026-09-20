# Implementation Plan: ARGUS AI Runtime — Reasoning Gateway & Autonomous Control Loop

**Spec:** `specs/002-ai-runtime/spec.md`
**Status:** Draft

## 1. Summary

Implement the "brain" behind ARGUS: a reasoning gateway that poses structured
decisions to a JEV ("System One") decision engine, converts the returned
labels/probabilities into typed domain entities (Hypothesis, Plan), routes every
proposed action through the policy boundary, executes only authorized typed
actions, and records an auditable trail.

Key design choices:

- **Decision-first reasoning.** The gateway does not ask a model to generate
  free text. It authors structured `choice` / `score` / `noul` questions over a
  bounded, deduplicated evidence context and interprets the decision engine's
  probability distributions.
- **Adapter boundary.** The decision data model is native Rust (`argus-ai-core`);
  the inference is an external `simple-jev` sidecar (or hosted) reached through a
  `DecisionProvider` adapter. No ML runtime in the core.
- **JEV prompt-authoring rules are requirements** (FR-011): structural score
  levels, explicit caps, evidence-backed deduplicated state, decision-procedure
  scoring, binary sub-questions for semantic confusions.
- **Security invariant preserved.** Decision output is data, never authority;
  only typed, policy-authorized actions reach the executor.

### Technical Context

| Item | Status |
|---|---|
| Decision engine runtime (local `simple-jev` vs hosted) | Resolved in `research.md` §1 |
| Concrete first action set | Resolved in `research.md` §2 |
| Decision data model: adopt `typesafe-ai-sdk` vs native types | Resolved in `research.md` §3 |
| JEV request/response contract | `contracts/decision-protocol.md` |
| Secret handling for decision providers | Resolved in `research.md` §4 |
| Deterministic testing without a live model | Resolved in `research.md` §5 |
| Decision → typed-entity mapping and confidence thresholds | Resolved in `research.md` §6 |

## 2. Architecture Impact

- **crates:** extend `argus-ai-core` (decision data model + `DecisionProvider`
  trait + reasoning gateway); extend `argus-daemon` (decision adapter wiring,
  control loop, policy dispatch of new action capabilities); extend
  `argus-ai-tui` (plans/approvals/audit views) in a follow-up.
- **domain contracts:** add `DecisionQuestion`/`DecisionAnswer`, `Hypothesis`,
  `Plan`, `Execution`, `AutonomyMode`.
- **adapters/plugins:** new `DecisionProvider` adapters (local `simple-jev`,
  hosted); service-control capability behind systemd/D-Bus.
- **security boundaries:** reasoning → policy → executor boundary is unchanged
  and hardened; the decision engine is an untrusted integration surface.
- **state/persistence:** new logical collections (hypotheses, plans, executions,
  audit) behind the existing `DomainRepository` abstraction.
- **event schemas:** new `hypothesis.*`, `plan.*`, `action.*`, `provider.*`
  events.
- **observability:** spans + correlation IDs across decision → plan → policy →
  execution; audit records per Principle 10.
- **installation/deployment:** optional local `simple-jev` sidecar; hosted
  endpoint as an alternative adapter.

## 3. Proposed Design

See `research.md` for the decision-by-decision rationale. The runtime flow is:

```text
Observe (evidence, bounded + deduplicated)
   → author structured decisions (choice/score/noul)
   → DecisionProvider adapter → JEV engine (logits scoring)
   → validate labels/probabilities against schema + confidence threshold
   → map to Hypothesis / Intent / Plan (typed)
   → policy: allow / deny / require approval
   → executor: typed action (host.service.*)
   → re-observe → validate → learn (audit)
```

Alternatives considered (rejected): free-text generation + parse/validate
(reintroduces schema-drift risk and violates the "data, never authority" posture
for the decision path); embedding a Rust inference runtime in the core (heavy,
MoE support uncertain, see `research.md` §1).

## 4. Interfaces and Contracts

- `argus-ai-core::decision` — `DecisionQuestion` (`Choice | Score | Noul`),
  `DecisionAnswer`, `DecisionProvider` trait, `ReasoningGateway`.
- `contracts/decision-protocol.md` — the JEV `/v1/classifier` request/response
  contract the adapter implements.
- `contracts/capabilities.md` — adds `host.service.restart/stop/start`
  (LowRisk, reversible).
- `contracts/events.md` — adds the reasoning/plan/execution event set.
- IPC: extend operations to expose plan/approval/audit queries (follow-up).

## 5. Security Model

- **Trust boundaries:** decision engine (untrusted) → reasoning gateway (validates)
  → policy (decides) → executor (`argusd`, privileged) → host.
- **Required Linux capabilities:** the reasoner/gateway requires none; the
  service-control executor requires the minimum for systemd/D-Bus control, and
  no ambient capabilities.
- **Privileged operations:** `host.service.restart/stop/start` via systemd/D-Bus
  (kernel/structured API), never arbitrary shell.
- **Policy:** `allow / deny / require approval`, evaluated independently of
  decision output; autonomy modes bound what runs without approval.
- **Approval:** any action above the configured autonomy threshold requires
  operator approval; high-blast-radius actions are deny-by-default.
- **Plugin/MCP trust:** the decision engine and external MCPs are untrusted and
  never bypass the policy/executor boundary.

## 6. Data / State Model

See `data-model.md`. New entities (`Hypothesis`, `Plan`, `Action`, `Execution`,
`Decision`, `AutonomyMode`) persist through `DomainRepository`; the interim
SQLite adapter (ADR-019) is extended, and no LanceDB/backend types leak into
`argus-domain`.

## 7. Event Model

See `contracts/events.md`. Add `hypothesis.created`, `plan.proposed`,
`plan.approved`, `plan.denied`, `action.executed`, `action.failed`,
`validation.passed`, `validation.failed`, `provider.degraded`. Local bus default;
NATS optional (ADR-012).

## 8. Observability

- Logs: structured, with correlation IDs on every control-loop iteration.
- Traces: spans for `reasoning`, `decide`, `validate`, `authorize`, `execute`,
  `validate_outcome`.
- Metrics: decision latency, provider availability, approval counts.
- Audit: append-only records per Principle 10 (intent → plan → decision → action
  → execution → evidence → validation).

## 9. Testing Strategy

- **unit:** decision schema validation, JEV response scoring, confidence
  thresholds, autonomy-mode gating, policy outcomes.
- **contract:** `decision-protocol.md` round-trip with a mock `DecisionProvider`;
  capability and event schemas.
- **integration:** control loop with a deterministic fake decision engine; a real
  local `simple-jev` sidecar only in opt-in integration tests.
- **security/policy:** deny-by-default, approval gating, no-bypass of the
  executor boundary.
- **failure/recovery:** provider unreachable, invalid output, execution failure,
  validation failure.
- **Linux-specific:** systemd/D-Bus service control (skip gracefully when absent).

## 10. Rollout / Migration

- Additive: new capabilities/events/entities; existing read-only capabilities and
  IPC remain compatible.
- `simple-jev` sidecar is optional; the runtime reports `degraded` without it
  (Principle 11).
- No data migration beyond additive schema (append-only audit).

## 11. Risks

| Risk | Impact | Mitigation |
|---|---|---|
| JEV sidecar absent/misconfigured | no decisions possible | degrade gracefully; hosted adapter; clear health reporting |
| Decision engine returns plausible-but-wrong labels | wrong plan | confidence thresholds; policy + approval; evidence re-observation; validation |
| Provider prompt-authoring regressions | poor decisions | FR-011 rules codified; deterministic fixtures; eval harness (research.md §7) |
| `typesafe-ai-sdk`/`simple-jev` immaturity | churn | native Rust data model; adapter isolates the dependency |
| Over-eager execution | unsafe autonomy | conservative default autonomy mode; typed reversible actions only |

## 12. ADR Impact

- ADR-014 requires an **amendment**: the primary model interface shifts from
  "completion/streaming/tool calling" to a **structured-decision** capability
  (`choice`/`score`/`noul`); completion remains only for optional human-facing
  output. This must be recorded before implementation.
- All other ADRs (000, 001, 002, 008, 009, 010, 012, 013, 015, 016, 018, 019)
  remain sufficient.

## 13. Constitution Check

| Principle | Status | Evidence |
|---|---|---|
| P1 Rust Core | PASS | decision model + gateway + policy + executor in Rust; JEV inference is an AI-specific external integration |
| P2 Security Boundary | PASS | decision output is data; policy + executor gate |
| P3 Policy Before Execution | PASS | allow/deny/require approval |
| P4 Evidence Before Inference | PASS | Decision/evidence distinct from Hypothesis/Plan |
| P5 Kernel-Native | PASS | service control via systemd/D-Bus, not shell parsing |
| P6 Infra-Agnostic Core | PASS | no K8s/NATS prerequisite |
| P7 Plug-and-Play | PASS | decision providers are adapters |
| P9 Deterministic Control Plane | PASS | policy/validation testable without LLM |
| P10 Auditable Autonomy | PASS | full append-only audit |
| P11 Graceful Degradation | PASS | provider absent → degraded |
| P12 Storage Independence | PASS | repository abstraction |
| P13 Testability | PASS | mock decision adapter |
| P14 Least Privilege | PASS | reasoner no privileges; executor minimal |
| P15 Stable Core Contracts | PASS | versioned decision/capability/event contracts |
| P16 SpecKit/ADR Separation | PASS | ADR-014 amendment flagged, not silently changed |
| P17 No Generic Agent Framework | PASS | structured decision engine, not agent orchestration |
| P18 Autonomous Control Loop | PASS | explicit loop stages |
