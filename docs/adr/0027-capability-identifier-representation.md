# ADR-0027: Capability Identifier Representation and Input Validation

- **Status:** Accepted
- **Date:** 2026-09-27

## Context

The brain (the reasoning layer) must invoke only typed capabilities. The SPEC
states the rule twice — "capability identifiers are typed, registry-enumerable
values, never bare or unvalidated strings (ADR-0027)"
(`_bmad-output/specs/spec-argus/SPEC.md`, Constraints) and again as CAP-16's
success — and `tool-calling.md` says the brain emits `{ capability_id, input }`
where `capability_id` is a typed, registry-enumerable value, with `input`
schema-validated and the call routed through policy and then the typed executor.

The shipped implementation represents capability identity differently.
`CapabilityId` is a validated string newtype (`crates/argus-domain/src/id.rs`),
constructible only through `CapabilityId::new` (a dotted-path grammar) or an
associated `const` name; it keys `CapabilityRegistry`
(`crates/argus-domain/src/registry.rs`), is matched by its string form in the
policy (`crates/argus-policy/src/bootstrap.rs`), and is carried as a string on the
agent–cloud wire (`crates/argus-cloud/src/protocol/messages.rs`).

The two statements cannot both hold literally. A closed Rust
`enum CapabilityId` would satisfy the SPEC, but Constitution Principle 7 requires
that "installable and removable capabilities" be added "without requiring changes
to the ARGUS core", and ADR-0016 and the domain model (`§3.26`, `§11`) define the
capability set as plugin-extensible: contributors declare arbitrary capabilities
in a TOML manifest. A closed enum cannot name a plugin capability without a core
change per capability, or a string-carrying variant the SPEC forbids.

The requirement behind "never bare or unvalidated strings" is anti-injection: an
untrusted (model, cloud, or plugin) string must never *become* an executable
capability.
That is already enforced structurally at the executor boundary —
`Executor::execute` accepts only an `AuthorizedAction`, which is constructed only
from a `PolicyOutcome::Allow` decision (`crates/argus-executor/src/action.rs`) —
and by validated construction of `CapabilityId`. What is *not* yet enforced is
that a serialized string cannot construct a `CapabilityId` by bypassing
validation, and that a call's `input` is schema-validated before policy.

## Decision

### 1. The capability identifier is a typed, validated value, not a bare string

Capability identity remains `CapabilityId`: a typed value whose only constructors
are the registry's canonical names and a validating parse (`CapabilityId::new` /
`TryFrom<&str>`). It is never a raw `String` at any boundary that can reach the
executor.

### 2. The SPEC's "enum" is satisfied at the surface, not as a Rust enum

Every capability the product can invoke is *enumerable* from the registry
(`CapabilityRegistry::list`), and the brain's tool-call surface selects from that
enumerated set. The identifier is therefore an enum in the sense the SPEC
requires — a closed, typed, reviewable vocabulary of names — while the underlying
registry stays plugin-extensible.

### 3. A string cannot reach the executor

Deserialization of `CapabilityId` is validated: the derived `Deserialize` (which
bypasses the dotted-path grammar) is replaced by a validated one
(`#[serde(try_from = "String")]` or an equivalent `TryFrom`). Unknown, malformed,
or unregistered identifiers are rejected at the boundary (registry lookup /
policy) and never reach the executor.

### 4. Input is schema-validated before policy

A tool call carries `{ capability_id, input }`; `input` is validated against the
capability's registered `input_schema` (`CapabilityDescriptor::input_schema`)
before the policy decision. Structural validation is not safety (per
`tool-calling.md`); it gates the call, and the executor's per-capability
guardrails remain the safety layer.

### 5. Policy then executor, unchanged

A typed call becomes an `AuthorizationRequest`, is decided by `PolicyEvaluator`,
and executes only through `AuthorizedAction::new` (which refuses any outcome but
`Allow`) and `Executor::execute`. No new path is added.

## Consequences

### Positive

- Constitution Principle 7 (plug-and-play extensibility) and ADR-0016 are
  preserved; a plugin capability is invocable with no core change.
- The stable core contract (`CapabilityId`, `CapabilityRequest`, `Action`) is
  unchanged, so there is no ripple through registry, policy, cloud mapping, or
  executor.
- The security invariant ("a string capability id cannot reach the executor") is
  enforced by validated construction plus the type-level `AuthorizedAction` gate,
  not by convention.
- Existing behavior is preserved: deferred/unknown identifiers such as
  `host.process.signal` and `container.restart` remain expressible and are
  denied/refused as today, rather than being re-labelled "known".

### Negative

- The SPEC's word "enum" is realized by interpretation rather than literally; the
  canonical contract must be read through this ADR (and, optionally, its wording
  clarified to match).
- Identifier integrity now depends on every boundary using validated
  construction. A future deserialize bypass would reintroduce the hole, so the
  validated `Deserialize` is load-bearing and must be covered by a test.

## Considered Options

- **Closed Rust `enum CapabilityId`** (literal SPEC reading) — rejected: violates
  Principle 7 / ADR-0016 (plugin capabilities would require core changes), changes
  a stable core contract (Principle 15), and cannot represent deferred or unknown
  ids without re-labelling them "known".
- **Enum with a validated extension arm** (`enum CapabilityId { …,
  Custom(CapabilityName) }`) — viable if literal compliance is required. It still
  changes the core contract, and this ADR would then need to define "never
  strings" as "never an unvalidated bare string".
- **A separate brain-side enum over `CapabilityId`** — rejected: it introduces a
  second identity vocabulary and leaves plugin capabilities unaddressable by the
  brain.
- **Per-capability typed `input` structs** — rejected: validating each
  capability's `input` against a dedicated Rust struct would change the stable
  `Action` / `CapabilityRequest` contract and break plugin extensibility
  (Principles 7, 15). Typed input is instead realized as schema-validated
  structured `arguments` (Decision §4), one shared validator for local and cloud
  dispatch.

## Implementation Principles

1. Construct `CapabilityId` only through validated constructors; never from a raw
   string.
2. Validate the serialized form on the way in (no deserialize bypass).
3. Resolve the brain's tool call against the registry; an unknown or unregistered
   id is denied, never defaulted.
4. Schema-validate `input` against the descriptor before policy.
5. Route every call policy → `AuthorizedAction` → executor; reversal crosses the
   same boundary.

## Reference

- `_bmad-output/specs/spec-argus/SPEC.md` — CAP-16; Constraints ("capability
  identifiers are typed, registry-enumerable values, never bare or unvalidated
  strings (ADR-0027)"; "Guardrail predicates run at execution time against live
  state").
- `_bmad-output/specs/spec-argus/tool-calling.md` — typed calls; calls route
  through policy then executor; guardrails run at execution time.
- `_bmad-output/specs/spec-argus/brain-architecture.md` — trust boundaries;
  `CapabilityId` as a typed domain entity.
- `argus-ai/.specify/memory/constitution.md` — Principles 2, 7, 9, 14, 15, 16.
- `argus-ai/docs/adr/0016-plugin-architecture.md`;
  `argus-ai/docs/architecture/domain-model.md` §3.26 / §11.
- `argus-ai/docs/adr/0020-external-privileged-operations-authorization.md`,
  `argus-ai/docs/adr/0021-privileged-execution-and-privilege-declaration.md` —
  policy → executor boundary and type-level enforcement.
- Code: `argus-ai/crates/argus-domain/src/id.rs`,
  `argus-ai/crates/argus-domain/src/registry.rs`,
  `argus-ai/crates/argus-domain/src/capability.rs`,
  `argus-ai/crates/argus-executor/src/action.rs`,
  `argus-ai/crates/argus-daemon/src/runtime.rs`,
  `argus-ai/crates/argus-daemon/src/cloud.rs`.
