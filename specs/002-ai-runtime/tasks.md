# Tasks: ARGUS AI Runtime — Reasoning Gateway & Autonomous Control Loop

**Spec:** `specs/002-ai-runtime/spec.md`
**Plan:** `specs/002-ai-runtime/plan.md`

## Task Rules

- Keep tasks independently implementable where possible.
- Identify the affected crate/component.
- Mark security-sensitive work explicitly.
- Do not bypass established domain contracts.
- Tests are tasks, not optional follow-up work (Constitution P13, AGENTS.md).

## User-story mapping

The spec expresses requirements as FRs; the mapping to implementation increments is:

| Story | FRs | Goal |
|---|---|---|
| US1 | FR-001, FR-006, FR-010 | Structured decision foundation (native decision types, provider trait, JEV adapter, fake, secrets) |
| US2 | FR-002, FR-003, FR-011 | Reasoning gateway produces typed Hypothesis/Plan from decisions |
| US3 | FR-004, FR-005, FR-007 | Safety: policy routing, autonomy modes, typed action execution |
| US4 | FR-008, FR-009 | Audit, observability, graceful degradation |

## Tasks

### Phase 1 — Setup

- [X] T001 Amend ADR-014 to add the structured-decision capability (`choice`/`score`/`noul`; completion demoted to optional human-facing output) in `docs/adr/0014-llm-provider-abstraction.md`
- [X] T002 Add `pub mod decision;` and create the module skeleton in `crates/argus-ai-core/src/decision/mod.rs`

### Phase 2 — Foundational (blocking for all stories)

- [X] T003 Add native `DecisionQuestion` (`Choice | Score | Noul`) and `DecisionAnswer` types in `crates/argus-ai-core/src/decision/types.rs`; enforce "2–50 criteria" and "Score levels ordered lowest→highest" (data-model.md §1)
- [X] T004 Add the `DecisionProvider` trait (`Send + Sync`) in `crates/argus-ai-core/src/decision/provider.rs`
- [X] T005 [P] Add domain entities `Intent`, `Hypothesis`, `Plan`, `Action`, `Execution`, `AutonomyMode` in `crates/argus-domain/src/reasoning.rs` with fields and transitions from data-model.md §2–§3
- [ ] T006 [P] Add `credential_ref` to the decision-provider config and extend the `0600` secret store read path (reuse `argus.secrets.toml`) in `crates/argus-ai-tui/src/setup.rs`

### Phase 3 — US1: Structured decision foundation

- [X] T007 [US1] Implement the JEV HTTP adapter (`POST /v1/classifier` request/response per `contracts/decision-protocol.md`) in `crates/argus-ai-core/src/decision/adapters/jev.rs`
- [X] T008 [P] [US1] Implement a deterministic fake `DecisionProvider` for tests in `crates/argus-ai-core/src/decision/adapters/fake.rs`
- [X] T009 [US1] Implement decision-output validation (reject malformed/missing-label/out-of-range answers) in `crates/argus-ai-core/src/decision/validate.rs`
- [X] T010 [US1] Unit + contract tests: type round-trip, adapter round-trip (mock HTTP), validation in `crates/argus-ai-core/tests/decision.rs`

### Phase 4 — US2: Reasoning gateway → typed plan

- [X] T011 [US2] Implement bounded context assembly with deduplicated evidence (FR-002) in `crates/argus-ai-core/src/decision/context.rs`
- [X] T012 [US2] Implement decision authoring: structural score levels, explicit caps, decision-procedure scoring, `noul` sub-questions (FR-011) in `crates/argus-ai-core/src/decision/author.rs`
- [X] T013 [US2] Implement `ReasoningGateway` (author → call provider → map answers to `Hypothesis`/`Plan` with confidence threshold) in `crates/argus-ai-core/src/decision/gateway.rs`
- [X] T014 [US2] Gateway tests with the fake provider (below-threshold ⇒ no plan; schema-invalid ⇒ rejected) in `crates/argus-ai-core/tests/gateway.rs`

### Phase 5 — US3: Policy, autonomy, execution (security-sensitive)

- [X] T015 [US3] Extend the policy evaluator to route `Action` capabilities through `allow`/`deny`/`require approval` in `crates/argus-policy/src/lib.rs`
- [X] T016 [P] [US3] Implement `AutonomyMode` gating (`ObserveOnly`/`Propose`/`Assisted`) in `crates/argus-ai-core/src/decision/autonomy.rs`
- [X] T017 [US3] Register `host.service.restart`/`stop`/`start` capabilities (`LowRisk`, reversible) in `crates/argus-daemon/src/runtime.rs`
- [X] T018 [US3] Implement the service-control executor over systemd/D-Bus (no shell) in `crates/argus-executor/src/service.rs`
- [X] T019 [US3] Wire the control loop (reasoning → policy → executor → validate) in `crates/argus-daemon/src/control.rs`
- [X] T020 [US3] Security/policy tests: deny-by-default, approval gating, executor no-bypass in `crates/argus-daemon/tests/policy.rs`

### Phase 6 — US4: Audit, observability, degradation

- [X] T021 [US4] Add `hypothesis.*`/`plan.*`/`action.*`/`validation.*`/`provider.degraded` events per `contracts/events.md` in `crates/argus-events/src/lib.rs`
- [ ] T022 [US4] Persist hypotheses/plans/executions/audit behind `DomainRepository` (SQLite interim) in `crates/argus-state/src/repository.rs`
- [ ] T023 [US4] Add decision-provider health + `degraded` status (FR-009) in `crates/argus-daemon/src/runtime.rs`
- [ ] T024 [US4] Add tracing spans + correlation IDs across `decide → plan → policy → execute` in `crates/argus-daemon/src/control.rs`
- [X] T025 [US4] Failure/recovery tests (provider unreachable, invalid output, execution failure, validation failure) in `crates/argus-daemon/tests/failure.rs`

### Phase 7 — Polish & cross-cutting

- [X] T026 Decision-authoring eval harness with fixed fixtures (research.md §7) in `crates/argus-ai-core/tests/decision_eval.rs`
- [ ] T027 Expose plan/approval/audit IPC operations in `crates/argus-ipc/src/lib.rs`
- [ ] T028 [P] Add plans/approvals/audit views to the TUI in `crates/argus-ai-tui/src/app.rs`
- [ ] T029 Run and document the validation scenarios end-to-end in `specs/002-ai-runtime/quickstart.md`

## Dependencies

```text
Phase 1 (Setup) → Phase 2 (Foundational) → US1 → US2 → US3 → US4 → Polish
```

- US2 depends on US1 (gateway calls a provider).
- US3 depends on US2 (execution consumes plans) and on the Foundational domain entities.
- US4 depends on US2/US3 (audit records reasoning + execution).
- US1, US2, US3, US4 each produce an independently testable increment.

## Parallel execution examples

- **Within US1:** T007 (JEV adapter) and T008 (fake adapter) in parallel (separate files).
- **Within Foundational:** T005 (domain entities) and T006 (secrets) in parallel.
- **Within US3:** T015 (policy) and T016 (autonomy gating) in parallel (separate crates).
- **Across stories after US2:** US3 (safety) and US4 (observability) can proceed in parallel
  once the gateway produces plans, since they touch disjoint crates.

## Implementation strategy (MVP first)

1. **MVP = US1 + US2**: ship a deterministic reasoning gateway that poses structured
   JEV decisions against a fake provider and returns validated typed plans. No execution.
2. **Safety milestone = +US3**: add policy/autonomy/execution so approved low-risk
   service actions can run — still default `Propose` (no autonomous execution).
3. **Observability = +US4**: audit, events, tracing, degradation.
4. **Polish**: eval harness, IPC/TUI surfaces, quickstart validation.

## Verification Checklist

- [X] Constitution satisfied (all 18 principles; see plan.md §13)
- [X] ADR-014 amendment recorded (T001)
- [X] No unauthorized privilege expansion (US3 tasks)
- [X] Domain model remains infrastructure-agnostic (data-model.md)
- [X] Tests pass (`cargo test --workspace`)
- [ ] Telemetry present (T024 — deferred)
- [X] Rollback/failure behavior documented (T025, quickstart.md Scenario 6–8)
