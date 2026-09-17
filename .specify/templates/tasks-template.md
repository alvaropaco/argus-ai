# Tasks: [FEATURE NAME]

**Spec:** `specs/[NNN-slug]/spec.md`
**Plan:** `specs/[NNN-slug]/plan.md`

## Task Rules

- Keep tasks independently implementable where possible.
- Identify the affected crate/component.
- Mark security-sensitive work explicitly.
- Do not bypass established domain contracts.
- Tests are tasks, not optional follow-up work.

## Tasks

### Phase 1 — Foundation

- [ ] T001 [P0] Confirm applicable ADRs and constitutional constraints
- [ ] T002 [P0] Define/update domain contracts

### Phase 2 — Implementation

- [ ] T003 [P1] Implement core capability
- [ ] T004 [P1] Implement adapter/plugin integration

### Phase 3 — Security

- [ ] T005 [P0] Implement policy/authorization behavior
- [ ] T006 [P0] Verify least-privilege requirements

### Phase 4 — Observability

- [ ] T007 [P1] Add structured logging
- [ ] T008 [P1] Add OpenTelemetry instrumentation
- [ ] T009 [P1] Add domain/audit events

### Phase 5 — Testing

- [ ] T010 [P0] Unit tests
- [ ] T011 [P1] Integration tests
- [ ] T012 [P1] Failure/recovery tests
- [ ] T013 [P0] Security/policy tests

### Phase 6 — Documentation

- [ ] T014 [P2] Update architecture documentation
- [ ] T015 [P2] Update runbook / operator documentation

## Verification Checklist

- [ ] Constitution satisfied
- [ ] Relevant ADRs satisfied
- [ ] No unauthorized privilege expansion
- [ ] Domain model remains infrastructure-agnostic
- [ ] Tests pass
- [ ] Telemetry present
- [ ] Rollback/failure behavior documented
