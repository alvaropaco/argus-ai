# ADR-001: Rust as the Core Implementation Language

- **Status:** Accepted
- **Date:** 2026-09-16

## Context

ARGUS AI is intended to operate as a privileged, autonomous infrastructure runtime on Linux/Unix systems. The core must handle system discovery, process execution, networking, policy enforcement, IPC, observability, agent lifecycle, and security-sensitive operations.

The project needs a language suitable for long-running system software with strong performance, concurrency, portability, and memory-safety characteristics.

## Decision

Rust is the primary implementation language for ARGUS AI.

Rust will be used for the core runtime, privileged daemon, CLI, TUI, infrastructure adapters, policy enforcement, system discovery, execution layer, and other security-sensitive components.

AI-specific orchestration may use other languages where appropriate, provided that the privileged execution boundary remains controlled by the Rust runtime.

## Consequences

### Positive

- Strong memory and thread safety without a garbage collector.
- Excellent suitability for long-running system daemons.
- Good Linux/Unix interoperability.
- Strong ecosystem for CLI, TUI, networking, async runtimes, and systems programming.
- Clear separation between AI decision-making and privileged execution.

### Negative

- Higher development complexity than Go or Python for some components.
- Longer learning curve for contributors unfamiliar with Rust.
- Some emerging AI ecosystem components may have stronger Python support.

## Scope

This ADR establishes Rust as the **base language**. It does not require every ARGUS component, integration, or AI workflow to be implemented in Rust.