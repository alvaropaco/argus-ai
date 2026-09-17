# ADR-009: Security Boundary

- **Status:** Accepted
- **Date:** 2026-09-16

## Decision

The ARGUS privileged security boundary will be implemented in Rust.

ARGUS will separate AI reasoning from privileged infrastructure execution. The AI layer must not receive unrestricted root or shell access.

The trusted path will be conceptually:

`AI / Agent -> IPC -> Policy -> Executor -> Operating System`

The privileged daemon will own sensitive operations, authorization enforcement, auditing, and execution controls.

## Security Goals

- Minimize the blast radius of compromised or incorrect AI decisions.
- Prevent unrestricted arbitrary command execution.
- Apply least privilege to capabilities and extensions.
- Record security-sensitive actions.
- Provide explicit controls for high-impact and destructive operations.
- Support progressive autonomy levels without weakening the security boundary.