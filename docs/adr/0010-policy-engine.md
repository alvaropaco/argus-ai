# ADR-010: Policy Engine

- **Status:** Accepted
- **Date:** 2026-09-16

## Decision

ARGUS will use **Cedar** as the foundation for authorization and policy evaluation, with a small ARGUS-specific policy layer for infrastructure autonomy rules.

Cedar will provide the general authorization model around principals, actions, resources, and context. ARGUS-specific policy logic will cover operational concepts such as autonomy levels, blast radius, environment sensitivity, approval requirements, rate limits, and remediation constraints.

## Principle

The LLM does not define what it is allowed to do. The ARGUS policy layer independently evaluates whether a requested operation may execute.

## Consequences

- Authorization remains deterministic and auditable.
- Policies can evolve independently from agent prompts.
- High-impact operations can require human approval.
- The system avoids inventing a proprietary policy language.