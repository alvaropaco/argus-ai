# ADR-008: ARGUS Executor

- **Status:** Accepted
- **Date:** 2026-09-16

## Decision

ARGUS will build a native Rust `argus-executor` responsible for controlled process and system operations.

The executor will expose typed, capability-oriented operations rather than making arbitrary shell execution the primary primitive.

Examples include service restart, process inspection, filesystem operations, network operations, container operations, and other explicitly modeled actions.

Arbitrary command execution may exist as a restricted capability, but it must pass through the same authorization, policy, auditing, timeout, resource, and execution controls.

## Principle

AI agents request actions; `argus-executor` validates and executes authorized operations.

The executor is part of the trusted ARGUS control plane and must not depend on LLM correctness for safety.