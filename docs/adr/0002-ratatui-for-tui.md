# ADR-002: Ratatui for the Terminal User Interface

- **Status:** Accepted
- **Date:** 2026-09-16

## Context

ARGUS AI requires a first-class terminal user interface for installation, setup, discovery, configuration, monitoring, incident review, agent management, autonomy controls, and operational diagnostics.

Building a terminal UI framework internally would add unnecessary complexity and maintenance cost.

## Decision

ARGUS AI will use **Ratatui** as the primary TUI framework, with **Crossterm** as the terminal backend where appropriate.

The ARGUS TUI will be implemented as a dedicated Rust component and will communicate with the ARGUS runtime through well-defined internal interfaces rather than embedding business logic directly in the presentation layer.

## Consequences

### Positive

- Mature Rust ecosystem for terminal interfaces.
- Reusable widgets, layouts, tables, lists, gauges, tabs, and event handling.
- Native fit with the project's Rust foundation.
- Avoids maintaining a custom terminal rendering stack.

### Negative

- TUI-specific interaction patterns must be designed carefully for usability.
- Some advanced component behavior may require ARGUS-specific abstractions on top of Ratatui.

## Scope

Ratatui is the selected framework for the ARGUS terminal interface. The project may adopt additional Ratatui-compatible component libraries when they provide clear value without undermining the core architecture.