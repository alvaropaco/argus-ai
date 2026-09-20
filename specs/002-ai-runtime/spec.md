# Feature Specification: ARGUS AI Runtime — Reasoning Gateway & Autonomous Control Loop

**Status:** Draft
**Spec ID:** 002
**Created:** 2026-09-19

## Clarifications

### Session 2026-09-19

- Q: How should the JEV-style structured-decision engine integrate with the Rust core? → A: Option A — adopt the JEV structured-decision data model in the Rust core and treat inference as an adapter (local `simple-jev` sidecar or a hosted endpoint).
- Q: Should the reasoning gateway rely exclusively on JEV-style structured decisions, or also keep a completion-based model? → A: Option A — JEV structured decisions are authoritative for anything that can lead to execution; completion is kept only for optional human-facing output.
- Decision: the decision engine is JEV ("System One" structured decisions — `choice`, `score`, `noul` over shared context, scored from next-token logits). The prompt-authoring rules from the JEV prompt guide (structural levels, explicit caps, evidence-backed deduplicated state, decision-procedure scoring, binary sub-questions) are adopted as requirements.
- Decision: CUA S1 (a computer-use specialist) is NOT the decision engine; it may be added later as an optional computer-use adapter for legacy UIs without an API (out of scope for the brain).

## 1. Problem Statement

The bootstrap runtime (Spec 001) establishes the secure skeleton — the `argusd`
daemon, authenticated IPC, a policy/executor boundary, read-only capabilities,
state storage, a local event bus, and observability plumbing — but it has no
intelligence. The model-provider configuration collected by `argus init` is
stored but never consumed; no model is ever called; nothing reasons, plans, or
acts.

As a result, an operator can install and inspect ARGUS, but the runtime cannot
diagnose an incident, propose a remediation, or move observed state toward
desired state. The "brain" — the reasoning gateway and the autonomous control
loop described in the architecture — is missing.

## 2. Objective

Implement the AI reasoning layer and the autonomous control loop so that ARGUS
can, within an operator-defined autonomy boundary:

1. build a bounded, evidence-backed context for the current situation;
2. pose decisions to a configured decision engine (JEV-style `choice`, `score`,
   `noul` questions) rather than asking for free-form generation;
3. validate that decision output against the domain schema before it is treated
   as a hypothesis or plan;
4. route every proposed plan through policy (allow / deny / require approval);
5. execute only typed, policy-authorized actions — never model output directly;
6. validate the outcome and record an auditable, provenance-complete trail.

The governing invariant remains: **AI output is data, never authority.**

## 3. Scope

### In scope

- A decision-provider abstraction with adapters exposing JEV-style structured
  decisions (`choice`, `score`, `noul`); completion is supported only for
  optional human-facing output, never for execution-relevant decisions.
- Selection of primary and fallback decision models per provider.
- The reasoning gateway: context building, evidence/history retrieval, decision
  authoring (structured questions), and decision-output validation.
- Typed domain entities for Intent, Hypothesis, Plan, Action, and Decision.
- The control loop: observe → hypothesize → plan → authorize → execute →
  validate.
- Policy routing with allow / deny / require-approval outcomes before any
  execution.
- Autonomy modes that bound what the runtime may do without human approval.
- A small, reversible, typed action set available for execution in this phase
  (e.g. restart/stop/start a service), each requiring explicit capability
  registration and policy authorization.
- Full audit and observability of every reasoning and execution step.
- Secure handling of provider credentials (never in configuration, logs, or
  state).

### Out of scope

- A general-purpose agent orchestration framework (ADR-013).
- Arbitrary shell execution as the primary action model.
- Full Kubernetes controller, full cloud provider integrations, or NATS as a
  hard dependency.
- Production-grade eBPF telemetry and complete enterprise discovery/correlation
  (delivered separately); the AI runtime consumes whatever evidence the
  discovery layer currently exposes.
- A full external policy runtime (e.g. Cedar); the existing deterministic policy
  boundary is sufficient for this phase.
- Computer-use (GUI) automation; a specialist model such as CUA S1 may be added
  later as an optional adapter for legacy interfaces without an API.

## 4. Actors

- **Operator:** configures the decision provider and autonomy mode, reviews and
  approves/denies plans, and monitors the audit trail.
- **AI capability (reasoner/planner):** produces hypotheses and plans as
  *structured data*; it has no authority and cannot act on its own.
- **Decision engine (JEV-style):** scores structured `choice`/`score`/`noul`
  questions over shared context and returns labeled probabilities/confidences;
  it is invoked through an adapter and is an untrusted integration surface.
- **Runtime (`argusd` / core):** orchestrates the control loop, authors the
  structured decisions, persists domain state, and enforces the reasoning →
  policy → executor boundary.
- **Policy engine:** evaluates each proposed plan/action and returns
  allow / deny / require approval.
- **Executor:** performs typed, policy-authorized actions and records evidence.

## 5. Functional Requirements

### FR-001 — Structured decisions, not free text

The system MUST pose reasoning as structured decision questions (`choice`,
`score`, `noul`) over a shared context and MUST validate the returned labels and
probabilities against the domain schema before any hypothesis or plan enters
planning or execution. Malformed, partial, or schema-invalid decision output
MUST be rejected and reported, never executed.

### FR-002 — Bounded, evidence-backed context

Before each decision request, the system MUST assemble a bounded context from
current evidence and relevant history, and MUST not send the model unbounded or
privileged data (secrets, raw credentials, unrelated operational state). The
context MUST be deduplicated and MUST contain the specific evidence that any
score level or decision rule references.

### FR-003 — Typed intent, hypothesis, and plan

The system MUST express the result of reasoning as typed entities (Intent,
Hypothesis, Plan composed of typed Actions), not free text. A Plan MUST carry
objective, preconditions, ordered actions, expected outcomes, rollback, blast
radius, and a confidence indicator.

### FR-004 — Policy before execution

Every proposed action MUST pass through policy evaluation and produce exactly
one of `allow`, `deny`, or `require approval`. A denied action MUST never reach
the executor. An action requiring approval MUST remain blocked until an
authorized operator approves it.

### FR-005 — No direct privileged execution from model output

Model output MUST never be able to trigger a privileged operation directly. The
only path from reasoning to execution is via validated, typed actions and an
explicit policy decision.

### FR-006 — Multiple decision providers and fallback

The system MUST support multiple decision providers behind one abstraction, and
MUST be able to configure a primary decision model with ordered fallbacks that
are tried when the primary is unavailable or fails validation.

### FR-007 — Autonomy modes

The system MUST support at least the autonomy modes: **observe-only** (reason
but never propose execution), **propose** (produce plans requiring approval),
and **assisted** (execute only low-risk, explicitly permitted actions; higher
risk requires approval). The mode MUST be operator-configurable and MUST default
to the most conservative setting.

### FR-008 — Auditable execution record

Every reasoning result, policy decision, execution attempt, and validation
outcome MUST be recorded with correlation identifiers, provenance, actor, and
timestamps, forming an append-only operational history that an operator can
review.

### FR-009 — Provider health and graceful degradation

The system MUST expose provider health/availability and MUST degrade gracefully
when a provider is unreachable: report the condition, fall back where
configured, and never fabricate a plan from a failed call.

### FR-010 — Secret isolation

Provider credentials MUST be stored separately from non-secret configuration
and MUST be redacted from configuration output, logs, events, and state.

### FR-011 — Decision-authoring quality

The reasoning gateway MUST author structured decisions following these rules:

- score levels MUST anchor to observable *structure/behavior*, not topic names,
  so the decision engine can classify situations outside any predefined taxonomy;
- score levels MUST be written as rules with explicit caps, not vague
  adjectives;
- the shared context MUST contain the specific evidence each level or rule
  references, deduplicated so evidence artifacts are not misread as signals;
- the top level of a score MUST enumerate sufficient conditions and the score
  MUST read as a decision procedure ("pick the highest level whose conditions
  hold");
- semantic confusions MUST be split into their own binary (`noul`) question and
  wired into the level rules that depend on them.

## 6. Non-Functional Requirements

### Security

- Least privilege: the reasoner holds no privileged execution rights.
- Explicit authorization for every action.
- No secrets in plaintext configuration, logs, or state.
- Decision providers and external MCPs are untrusted surfaces.

### Reliability

- Graceful degradation when a provider or the discovery layer is unavailable.
- Timeout on every model call and execution attempt.
- Deterministic behavior independent of the LLM: policy and validation must be
  testable and reproducible without a live model.

### Performance

- A reasoning request completes within operator-acceptable bounds (see
  acceptance criteria); context assembly and validation do not stall the control
  loop.

### Observability

- Structured logs with correlation IDs spanning reasoning → policy → execution.
- Traces for each control-loop iteration.
- Domain events (plan proposed/approved/denied, action executed, validation
  outcome, provider degraded).

## 7. Domain Impact

### Entities

- `Intent` — a desired condition (e.g. "service nginx is running").
- `Hypothesis` — a proposed explanation, with confidence and supporting evidence.
- `Plan` — an ordered set of typed actions plus objective, preconditions,
  expected outcomes, rollback, blast radius, confidence.
- `Action` — a typed, capability-backed operation (restart/stop/start service,
  …).
- `Execution` — a single attempted action with policy decision, timings, result,
  and evidence.
- `Decision` — a structured `choice` / `score` / `noul` question posed to the
  decision engine, together with its returned labels, probabilities, and
  confidence.
- `AutonomyMode` — the configured level of autonomous authority.
- `ProviderConfig` — the (existing) decision-provider selection, extended with
  credential reference and health state.

### Capabilities

- Existing read-only capabilities remain.
- New typed, policy-checkable action capabilities (e.g. `host.service.restart`,
  `host.service.stop`, `host.service.start`), each declaring risk class and
  blast radius.

### Events

- `hypothesis.created`, `plan.proposed`, `plan.approved`, `plan.denied`,
  `action.executed`, `action.failed`, `validation.passed`, `validation.failed`,
  `provider.degraded`, `incident.resolved`.

### State transitions

- Plan: `proposed → approved | denied | superseded`, then
  `approved → executing → completed | failed | rolled_back`.
- Incident: `open → investigating → mitigated → resolved` (or `closed`).

## 8. Infrastructure / Platform Impact

- The runtime needs network access to the configured decision endpoint — a local
  `simple-jev` sidecar or a hosted decision service — with the JEV structured
  decision protocol as the integration contract.
- The decision-provider data model (question/answer types) lives in the Rust
  core; the inference itself stays behind an adapter, so no Python/ML runtime is
  required in the core.
- Provider credentials require a protected local store with restricted
  permissions (no plaintext config).
- The single-host baseline must remain functional without NATS, Kubernetes, or
  eBPF.

## 9. Security and Policy

- **Required privileges:** the reasoner requires none; the executor requires the
  minimum privileges for each typed action, declared per capability.
- **Capabilities:** each executable action is a registered capability with a
  risk class and blast radius.
- **Policy decisions:** `allow / deny / require approval`, evaluated
  independently of model output.
- **Approval requirements:** any action above the configured autonomy threshold
  requires operator approval.
- **Blast radius & rollback:** plans must declare blast radius and rollback;
  irreversible or high-blast-radius actions are denied or require approval.
- **Plugin/MCP trust:** external MCPs and providers remain untrusted extension
  surfaces and never bypass the policy/executor boundary.

## 10. Failure and Recovery

- **Provider unreachable/timeout:** mark the provider degraded, fall back if
  configured, and surface the condition without producing a plan.
- **Invalid model output:** reject, log, and optionally retry with the fallback
  model; never execute.
- **Policy denial:** record the denial; no execution occurs.
- **Execution failure:** record the failure, offer rollback where the plan
  declares one, and escalate for approval when retries are exhausted.
- **Validation failure (desired state not reached):** re-observe, re-plan, or
  escalate; never silently loop indefinitely.

## 11. Acceptance Criteria

- [ ] AC-001 — Given a valid model configuration, the runtime produces a
  schema-validated Plan for a simulated incident without human intervention,
  and that Plan is visible to the operator with its objective, actions, and
  confidence.
- [ ] AC-002 — Malformed or schema-invalid model output is rejected and logged,
  and no action is executed as a result.
- [ ] AC-003 — A plan requiring approval is blocked until an operator approves
  it; a denied plan never reaches execution.
- [ ] AC-004 — In the most conservative autonomy mode, the runtime never
  executes an action without explicit approval.
- [ ] AC-005 — A validated, approved, low-risk typed action (e.g. restart a
  stopped service) is executed and its outcome and evidence are recorded and
  queryable.
- [ ] AC-006 — Every reasoning, decision, execution, and validation step carries
  a correlation identifier and is present in the audit trail.
- [ ] AC-007 — Provider credentials never appear in configuration output, logs,
  events, or state.
- [ ] AC-008 — With the provider unreachable, the runtime reports degraded
  status (or falls back) and does not fabricate a plan.
- [ ] AC-009 — The control loop is demonstrable end-to-end on a single Linux
  host with no Kubernetes, NATS, or eBPF.
- [ ] AC-010 — Policy and output-validation behavior are testable and
  deterministic without a live LLM.
- [ ] AC-011 — The reasoning gateway authors structured decisions (`choice`,
  `score`, `noul`) that follow the decision-authoring rules (structural levels,
  explicit caps, evidence-backed deduplicated context, decision-procedure
  scoring), verifiable by inspection of the questions produced for a given
  scenario.
- [ ] AC-012 — The decision engine is reached through an adapter, and the Rust
  core contains no model-inference runtime (the decision data model is native;
  inference is external).

## 12. Architectural Constraints

- ADR-000 — product boundary.
- ADR-001 — Rust core.
- ADR-002 — Ratatui TUI.
- ADR-008 — executor in Rust.
- ADR-009 — security boundary (AI output is data, never authority).
- ADR-010 — policy layer (`allow/deny/require approval`).
- ADR-012 — local event bus, NATS optional.
- ADR-013 — not a generic agent framework.
- ADR-014 — LLM provider abstraction.
- ADR-015 — tracing + OpenTelemetry.
- ADR-016 — plugin architecture + capability manifests.
- ADR-018 — kernel-native domain model.
- ADR-019 — interim SQLite state backend behind the repository abstraction.

## 13. Assumptions and Open Questions

### Assumptions

- The first executable action set is limited to reversible, low-blast-radius
  host service actions; destructive and high-blast-radius actions remain
  deny-by-default or require approval.
- The discovery layer continues to expose the current bootstrap-level evidence
  (read-only host status); richer discovery is a separate feature.
- The deterministic bootstrap policy evaluator is sufficient for this phase; a
  Cedar runtime may replace it later behind the same trait.
- The default autonomy mode is `propose` (no autonomous execution).
- The decision engine is JEV ("System One" structured decisions); the decision
  data model is native to the Rust core and the inference is an adapter.

### Open Questions

- Which additional typed actions (beyond service restart/stop/start) are
  required for the first usable milestone?
- Is a local `simple-jev` sidecar required for the first milestone (offline,
  deterministic testing), or is a hosted decision endpoint sufficient to start?
