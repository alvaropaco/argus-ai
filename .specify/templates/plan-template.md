# Implementation Plan: [FEATURE NAME]

**Spec:** `specs/[NNN-slug]/spec.md`
**Status:** Draft

## 1. Summary

Describe the implementation approach and major design choices.

## 2. Architecture Impact

Identify affected:

- crates;
- domain contracts;
- adapters/plugins;
- security boundaries;
- state/persistence;
- event schemas;
- observability;
- installation/deployment.

## 3. Proposed Design

Describe the selected design and alternatives considered.

## 4. Interfaces and Contracts

List changes to Rust traits, structs, schemas, MCP tools, plugin manifests, events, policies, or APIs.

## 5. Security Model

Specify:

- trust boundaries;
- required Linux capabilities;
- privileged operations;
- Cedar policies;
- plugin permissions;
- approval requirements.

## 6. Data / State Model

Describe repository changes and how LanceDB is accessed through abstractions.

## 7. Event Model

Describe domain events and local/NATS transport behavior.

## 8. Observability

Define logs, metrics, traces, OpenTelemetry spans/attributes, and audit records.

## 9. Testing Strategy

- unit tests;
- contract tests;
- integration tests;
- Linux-specific tests;
- plugin/MCP tests;
- failure/recovery tests;
- security/policy tests.

## 10. Rollout / Migration

Describe installation, upgrade, compatibility, rollback, and migration considerations.

## 11. Risks

| Risk | Impact | Mitigation |
|---|---|---|
| ... | ... | ... |

## 12. ADR Impact

State whether existing ADRs are sufficient. If not, identify the ADR that must be created or amended before implementation.
