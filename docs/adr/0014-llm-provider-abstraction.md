# ADR-014: LLM Provider Abstraction

- **Status:** Accepted
- **Date:** 2026-09-16
- **Amended:** 2026-09-19

## Decision

ARGUS will implement its own provider abstraction in Rust rather than coupling the core to a single LLM vendor or external gateway.

The abstraction will expose capabilities needed by ARGUS such as completion, streaming, structured output, tool calling, model metadata, and provider health/capability discovery.

Providers will be implemented as adapters, including as appropriate:

- OpenAI
- Anthropic
- Google/Gemini
- OpenRouter
- Ollama/local models
- Future providers

The provider abstraction must allow ARGUS to configure primary, fallback, specialized, and local models independently.

## Amendment (2026-09-19): Structured decisions are the primary interface

For the autonomous reasoning path, the **structured-decision** capability is the
primary provider interface, superseding free-text completion. The reasoning
gateway poses `choice` / `score` / `noul` questions over a shared context and
consumes probability distributions over predefined labels, rather than asking a
model to generate free-form output that must be parsed and validated.

This decision interface is realized by a JEV ("System One") decision engine
(`simple-jev` sidecar or a hosted endpoint), reached through a `DecisionProvider`
adapter. The decision data model is native Rust; the inference is an external
integration.

Consequences:

- `completion`, `streaming`, and `tool calling` remain supported only for
  optional human-facing output and are never inputs to authorization or
  execution.
- Decision output is data, never authority: policy remains the sole
  authorization boundary (ADR-009, ADR-010).

## Principle

LLM providers are replaceable infrastructure dependencies, not part of the ARGUS core domain model.
