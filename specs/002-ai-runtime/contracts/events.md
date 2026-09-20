# Event Contract (Spec 002 extension)

**Spec:** `specs/002-ai-runtime/spec.md`
**Status:** Draft
**Date:** 2026-09-19

Adds reasoning, plan, execution, and provider events to the Spec 001 event
taxonomy. Schema and transport rules are unchanged: events are immutable,
additive, and transported via the local bus by default (NATS optional, ADR-012).

## 1. Added event types

| Type | Emitted when | Payload |
|---|---|---|
| `hypothesis.created` | a validated hypothesis is recorded | `{ "hypothesis_id", "incident_id", "confidence" }` |
| `plan.proposed` | a plan is produced from decisions | `{ "plan_id", "intent_id", "actions" }` |
| `plan.approved` | policy allows a plan | `{ "plan_id", "policy_id" }` |
| `plan.denied` | policy denies a plan | `{ "plan_id", "reason" }` |
| `action.executed` | an executor runs a typed action | `{ "execution_id", "capability", "resource" }` |
| `action.failed` | an action fails | `{ "execution_id", "reason" }` |
| `validation.passed` | desired state reached | `{ "plan_id", "evidence" }` |
| `validation.failed` | desired state not reached | `{ "plan_id", "reason" }` |
| `provider.degraded` | decision provider unreachable/failing | `{ "provider", "reason" }` |

## 2. Correlation

Every reasoning/execution event carries `correlation_id` (request/loop iteration)
and `causation_id` (parent event), forming the Principle 10 audit chain:
`hypothesis.created → plan.proposed → plan.approved → action.executed →
validation.passed`.
