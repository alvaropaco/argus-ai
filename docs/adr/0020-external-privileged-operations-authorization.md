# ADR-0020: Authorization and Approval Model for Externally Requested Privileged Operations

- **Status:** Accepted
- **Date:** 2026-09-21

## Context

Argus Cloud can ask an installation to perform a capability invocation, including
environment-changing and privileged operations (feature `001-argus-cloud-sync`,
operator decision Q2). This is the first time ARGUS accepts a privileged request
originating outside the host.

Constitution Principle 2 establishes that generated output is data, never
authority, and that privileged operations must cross the policy and executor
boundary. Principles 3, 15, and 16 require explicit authorization before execution
and an ADR for any change to policy semantics or the security boundary. The cloud's
own pairing contract states it plainly: pairing grants cloud identity, not
infrastructure authority.

Without a recorded model, two failure modes are likely: treating a cloud message as
an authorization grant, and classifying every remote request as harmless because
the requesting principal is trusted.

## Decision

### 1. The cloud is a request origin, never an authority

A cloud-issued invocation arrives as a `CapabilityRequest` proposed with the cloud
as its `Principal`. It carries no authority of its own. The only authority remains
the local `PolicyEvaluator` in `argus-policy`, whose decision gates execution
through `AuthorizedAction::new`, which structurally refuses any outcome other than
`Allow`.

### 2. Risk class and blast radius are derived from the capability, never supplied by the request

Every invocation's `RiskClass` and `BlastRadius` are taken from the invoked
capability's registered `CapabilityDescriptor` and its privilege declaration.
Neither the cloud's message nor a built-in default may supply them. A capability
that cannot declare its blast radius MUST be treated as `Host`, never as `None`.

The hardcoded `RiskClass::Read, BlastRadius::None` currently applied in
`argus-daemon` is removed by this decision. An invocation classified as harmless
bypasses every downstream control, so under-classification is the single most
dangerous defect this feature could ship with.

### 3. Approval is per invocation, attributed and time-bounded

When policy returns `RequireApproval`, the installation requires a local
`ExecutionApproval` that is bound to that specific `command_id`, attributed to the
granting local principal, and bounded by an expiry.

Standing approvals and capability-level approvals are forbidden: they silently
convert a reviewed action into an unreviewed one.

An invocation requiring approval that has not been granted is reported to the cloud
immediately as `refused` with reason `approval_required`. It is not parked, because
the cloud's `command.result` admits only `acknowledged | failed | refused` and has
no pending state to receive a later answer; a parked command would appear hung and
then expire cloud-side. To proceed, the operator grants approval locally and the
cloud re-issues the invocation under a new `command_id`.

### 4. The refusal vocabulary is closed at five reasons

`unknown_capability`, `invalid_input`, `execution_disabled`, `policy_denied`,
`approval_required`. Each corresponds to exactly one step of the validation ladder
in `contracts/privileged-execution.md` §2, so every refusal is diagnosable from its
reason alone. Adding a sixth reason requires amending this ADR.

### 5. The local kill switch is not remotely controllable

`allow_privileged_execution` is settable only from the host. When false it refuses
privileged invocations with `execution_disabled` and does not affect reporting,
configuration application, or read-only invocations. Any cloud message purporting
to change it is ignored and surfaced as a diagnostic. A remote system able to
re-enable its own privileged execution does not have a kill switch at all.

### 6. A cloud-issued operation enters the control loop at Authorize

A cloud-issued operation enters the canonical loop at **Authorize**, never at
Execute. Its audit record carries the intent (command and correlation identity),
the authorization decision, the action, the execution, the resulting evidence, and
the validation outcome, so Principle 10's chain and Principle 18's stages remain
explicit.

## Consequences

### Positive

- The cloud cannot obtain privilege the policy engine has not granted, and the
  guarantee is enforced at the type level by `AuthorizedAction` rather than by
  convention.
- Approval is attributable to a person and expires.
- Refusals are distinguishable and machine-checkable (SC-005, SC-016, SC-017).
- The installation stays sovereign over its own privileged execution.

### Negative

- An approval-required operation cannot complete unattended; this is deliberate and
  costs convenience on the highest-risk operations.
- Removing the hardcoded classification makes each capability's declaration
  load-bearing — a mis-declared capability is now consequential rather than
  inert.
- The closed refusal vocabulary means a new class of refusal needs an ADR
  amendment rather than a new enum variant.

## Implementation Principles

1. Derive risk class and blast radius from the capability; never default them.
2. Never construct an `AuthorizedAction` from a non-`Allow` decision, and never add
   a path that reaches the executor without one.
3. Bind approval to a command id, a granting principal, and an expiry.
4. Keep the kill switch local-only and honour it immediately.
5. Record every invocation's decision and evidence in the audit trail.

## Reference

- `specs/001-argus-cloud-sync/contracts/privileged-execution.md` §1–§4, §8, §10
- `specs/001-argus-cloud-sync/spec.md` FR-039, FR-040, FR-042, FR-044, FR-045, FR-050
- ADR-0009 (security boundary), ADR-0010 (policy engine), ADR-0016 (plugin architecture)
- Constitution Principles 2, 3, 10, 15, 16, 18
