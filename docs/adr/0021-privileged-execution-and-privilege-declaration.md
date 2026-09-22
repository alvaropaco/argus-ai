# ADR-0021: Privileged Execution Path and Capability Privilege Declaration

- **Status:** Accepted
- **Date:** 2026-09-21

## Context

ADR-0020 establishes that a cloud-issued invocation crosses the policy and executor
boundary. This ADR records *how* a privileged operation is actually performed and
how the operating-system privilege it needs is bounded.

Two facts constrain the decision. First, the packaged service currently runs with
an empty `CapabilityBoundingSet` and `RestrictAddressFamilies=AF_UNIX`, so no
privileged operation is possible today — not even the service-control capabilities
already registered. Second, the project's own bootstrap research already settled the
mechanism: hardening plus an explicit capability bounding set is preferable to
running as root, and every capability that needs privilege must declare its exact
requirement.

Constitution Principle 14 requires the smallest practical privilege set and forbids
implicit grants. Principle 5 prefers kernel and structured interfaces over parsing
human-oriented command output. Principles 15 and 16 require this decision to be
recorded rather than assumed.

## Decision

### 1. Every privileged capability declares its privilege requirement

A capability that requires elevated privilege MUST declare
`required_os_privileges`: the exact Linux capabilities, namespaces, `seccomp`
profile, and `Landlock` rules it needs. The declaration is part of the capability
itself, next to its risk class and reversibility, and is subject to the same
registration validation.

A capability that needs privilege but declares none is a configuration error and
MUST be rejected at registration.

### 2. Registration fails closed

A capability whose declared privileges are not granted by the running sandbox MUST
fail registration and MUST NOT be published to the cloud.

Advertising a capability the installation cannot actually perform would mislead the
operator and convert a clear startup error into a confusing runtime failure at the
moment of invocation. Failing closed at startup is the only honest behaviour.

### 3. There is exactly one privileged execution path

```text
cloud command.invoke
  → daemon validation ladder (contracts/privileged-execution.md §2)
  → PolicyEvaluator
  → AuthorizedAction                     (structurally refuses non-Allow)
  → argus-executor privileged path
  → capability implementation
```

No other path exists. The cloud client crate has no access to policy or the
executor. There is no shell-string escape hatch: operations are typed capabilities,
never arbitrary commands.

### 4. Privileged execution lives in the executor, not in adapters

The privileged path is implemented in `argus-executor`, which is the single place
allowed to perform an authorized side effect. Adapters and plugins contribute
capability implementations, never their own execution authority.

### 5. Prefer structured interfaces

Within a capability implementation, kernel interfaces (D-Bus/systemd, `/proc`,
`/sys`, cgroup v2, Netlink, pidfd) are preferred over parsing CLI output
(Principle 5). Where a userspace tool is used, it is a deliberate, documented
fallback — as the existing service controller already does for `systemctl` against
the structured D-Bus API.

### 6. The packaged sandbox grants the union of declared privileges, enumerated

`deploy/debian/argusd.service` MUST grant only privileges that at least one
registered capability declares, each entry commented with the capability that
justifies it. It MUST NOT grant a blanket set. `NoNewPrivileges=true`,
`ProtectSystem=strict`, and the remaining hardening stay in place.

The sandbox is not a substitute for capability declaration: the declaration is the
review surface, and the sandbox is its mechanical expression.

### 7. Reversal crosses the same boundary

Reversing a privileged operation is itself an authorized action. A reversal MUST
NOT bypass policy, the approval requirement, or the executor. This closes the
obvious loophole of an unguarded "undo" path.

## Consequences

### Positive

- The privilege a capability holds is declared, reviewable, and mechanically
  reflected in the sandbox.
- A capability that cannot run is detected at startup rather than at invocation.
- There is a single audit-able path from a cloud request to a side effect.
- Least privilege is enforceable rather than aspirational.

### Negative

- Adding a privileged capability now requires editing the service unit as well as
  the capability — a deliberate friction that forces review.
- Each new capability must be evaluated for its `seccomp`/`Landlock` profile, which
  is more work than granting a broad bounding set.
- A sandbox misconfiguration manifests as a refused capability rather than a silent
  over-permission, which may surface as an operational surprise if the unit and the
  declarations drift.

## Implementation Principles

1. Declare privileges per capability; never grant implicitly.
2. Fail closed at registration when a declared privilege is unavailable.
3. Keep exactly one privileged execution path, through policy and the executor.
4. Prefer structured interfaces; document any userspace fallback.
5. Enumerate sandbox grants with the capability that justifies each.
6. Route reversal through the same boundary as the original action.

## Reference

- `specs/001-argus-cloud-sync/contracts/privileged-execution.md` §7, §9
- `specs/001-argus-cloud-sync/spec.md` FR-043, FR-048, FR-049, FR-062; SC-019
- `specs/001-argus-cloud-sync/research.md` R13
- ADR-0009 (security boundary), ADR-0016 (plugin architecture), ADR-0020
- Constitution Principles 3, 5, 14, 15, 16
