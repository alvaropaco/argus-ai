# ADR-0030: Pending-Approval Round-Trip

- **Status:** Accepted
- **Date:** 2026-09-27

## Context

Story 1.8 (CAP-17) must pause a plan at `require approval`, resume it after
operator approval, bind the approval to a context hash, and consume the token
exactly once. Nothing of the sort exists:

- `RequireApproval` is handled three different ways and never pauses: the local
  runtime collapses it into `DispatchError::Denied`
  (`crates/argus-daemon/src/runtime.rs:269-298`); the local plan executor records
  it and ends the plan `Denied`
  (`crates/argus-daemon/src/control.rs:75-211`); the cloud path refuses the
  command (`crates/argus-daemon/src/cloud.rs:1135,1234-1262`).
- `ExecutionApproval` is bound to `command_id` + an expiry only — no context
  hash, no consumed flag (`crates/argus-domain/src/cloud.rs:261-269`);
  `ApprovalStore::authorizes` never consumes, and `revoke` is the only consume
  primitive (`crates/argus-policy/src/approval.rs:17-88`).
- No operator approve/deny exists (IPC `Operation`, `handler.rs`, CLI, TUI).
- `Refused` is terminal (`crates/argus-domain/src/cloud.rs:121-133`), so a
  same-`command_id` refuse→grant→resume is impossible.
- ADR-0028 §6 deferred plan/execution persistence, and none exists.

SPEC.md:111 fixes the approver surface ("the local TUI and Argus Cloud"), and
tool-calling.md:30 fixes idempotency layer 3 ("a consumed approval token is
never replayed"). These choices are architectural, so they are recorded here.

## Decision

### 1. Operator approval is a local IPC operation, surfaced on the CLI

Add `ApprovalList`, `ApprovalGrant`, and `ApprovalDeny` operations to the IPC
`Operation` enum (`crates/argus-ipc/src/protocol.rs`, MINOR protocol bump), a
daemon handler arm, and CLI subcommands. The TUI approval view is a follow-up;
the CLI is the local operator surface delivered now. Cloud approval continues to
re-enter local policy as input, never authority.

### 2. A paused plan is held in an in-memory pending store

`control::authorize_and_run` no longer executes past a step whose policy returns
`RequireApproval`: it returns a `Pending` outcome carrying the plan, a
single-use token, and the binding hash, and the daemon stores it in a
`PendingApprovals` map. Persistence across a daemon restart remains out of scope
(ADR-0028 §6); a restart drops pending plans, which is fail-closed.

### 3. The approval is bound to a context hash

The binding hash is the canonical digest already produced for decisions
(`DecisionProvenance.context_hash` semantics,
`crates/argus-ai-core/src/decision/provenance.rs:17-54`) taken over the paused
plan's context. A resume whose recomputed hash differs is refused as stale.

### 4. The token is consumed exactly once

Granting stores an approval bound to `(token, context_hash, granted_by,
expires_at)`; granting again replaces it. Resuming consumes it (removes it); a
second resume with the same token is refused. A denied or expired approval never
executes.

### 5. Resume does not re-observe

Resume takes the stored plan (no new observe step) and re-enters the plan
executor with the granted approval; `PlanStatus` gains `AwaitingApproval` for the
paused state, and `PolicyOutcome::RequireApproval` on resume resolves to `Allow`
only through a consumed, hash-matching grant (the pattern the cloud path already
uses at `crates/argus-daemon/src/cloud.rs:1252-1256`).

### 6. Scope

The local plan path is the primary deliverable. The cloud `ApprovalStore` gains
the context-hash binding and consume-once for consistency; its command-scoped
flow (ADR-0020) is otherwise unchanged.

## Consequences

### Positive

- CAP-17's four verify clauses each map to one mechanism: pause (pending store),
  resume-without-re-observing (stored plan), stale-invalidation (context hash),
  single consumption (consume-on-use).
- The approval re-enters the same policy/executor boundary as everything else.

### Negative

- A new IPC operation and CLI surface; a new `PlanStatus` variant; pending plans
  do not survive a restart.
- The TUI approval view is not delivered here (recorded follow-up).

## Implementation Principles

1. Never execute a plan whose approval is missing, expired, consumed, or
   hash-mismatched.
2. Consume the token on the successful resume; never replay it.
3. Resume from the stored plan; never re-observe.
4. Keep approvals out of the authority path — they re-enter policy as input.
5. Fail closed on a changed world.

## Reference

- `_bmad-output/specs/spec-argus/SPEC.md` — CAP-17; approver-role/approval-surface
  and cloud-boundary constraints.
- `_bmad-output/specs/spec-argus/tool-calling.md` — plan state machine and
  idempotency layer 3.
- `argus-ai/docs/adr/0020-external-privileged-operations-authorization.md`,
  `argus-ai/docs/adr/0028-execution-time-guardrails-and-plan-execution-contract.md`.
- Code: `crates/argus-daemon/src/{control.rs,cloud.rs,handler.rs,runtime.rs}`,
  `crates/argus-domain/src/{cloud.rs,reasoning.rs}`,
  `crates/argus-policy/src/approval.rs`,
  `crates/argus-ai-core/src/decision/provenance.rs`,
  `crates/argus-ipc/src/protocol.rs`, `crates/argus-ai-cli/src/main.rs`.
