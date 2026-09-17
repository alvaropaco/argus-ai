# ADR-014: LLM Provider Abstraction

- **Status:** Accepted
- **Date:** 2026-09-16

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

## Principle

LLM providers are replaceable infrastructure dependencies, not part of the ARGUS core domain model.