# Verification: ARGUS Bootstrap Runtime

**Spec:** `specs/001-bootstrap/spec.md`
**Status:** Verified (bootstrap)
**Date:** 2026-09-16

Evidence-backed verification of the bootstrap against the constitution, ADRs,
and the Linux-first/no-optional-dependency requirements.

## Constitution compliance (T050)

All 18 principles hold for the bootstrap; the full check is in `plan.md` § 13.
Notable: the interim SQLite backend is recorded in ADR-019, satisfying
Principle 16 (no silent architectural redefinition) and Principle 12
(storage independence).

## ADR compliance (T051)

| ADR | Status | Evidence |
|-----|--------|----------|
| 000/001 Rust | PASS | workspace is 100% Rust |
| 002 Ratatui | PASS | `argus-tui` uses `ratatui` + `crossterm` |
| 003/005 MCP | PASS | `McpRuntime` trait + `mcp/README.md` boundary; `rmcp` deferred |
| 006 Tokio | PASS | single async runtime |
| 007 Host discovery | PASS (deferred) | `host.status.read` stubbed; native discovery is later scope |
| 008/009/010 | PASS | `argus-executor` + `PolicyEvaluator` + typed boundary |
| 011/019 LanceDB/SQLite | PASS | SQLite interim (ADR-019); LanceDB optional feature |
| 012 Events | PASS | `LocalEventBus` (Tokio), NATS-ready `EventBus` trait |
| 014 LLM abstraction | PASS (deferred) | no LLM coupling in bootstrap |
| 015 Observability | PARTIAL | `tracing` done; OpenTelemetry deferred (T035) |
| 016 Plugins | PASS | `PluginManifest` TOML + `CapabilityRegistry` |
| 017 Install/updates | PASS | `argus-install` verification + `argus upgrade` skeleton |
| 018 Domain model | PASS | kernel-native layered model documented; bootstrap subset implemented |

## Linux-first, no optional dependencies (T052–T054)

`cargo tree --workspace` contains **zero** matches for `kube`, `nats`, or
`ebpf`/`aya`/`bpf`. The runtime is a single Linux/Unix host binary with no
Kubernetes, NATS, or eBPF dependency (FR-006).

## Graceful degradation (T055)

OpenTelemetry is not yet wired (T035), so the daemon runs with local `tracing`
only and degrades cleanly by construction. When OTel is added, it must remain
optional (ADR-015).

## No secrets in logs (T056)

Bootstrap config holds no secrets; `config.get` returns only non-secret fields
(socket/state paths, environment name, log format). Logs emit paths and
correlation IDs only.

## Privileged paths cross policy/executor (T057)

Covered by `crates/argus-executor/tests/pipeline.rs` and
`crates/argus-daemon/tests/capability.rs`: a privileged capability is denied by
policy, and `AuthorizedAction` cannot be constructed from a non-`Allow` decision.

## Architecture documentation (T058)

`plan.md` § 6/11/12 and `research.md` § 4 record the SQLite interim decision and
the LanceDB upstream-compile-bug finding; ADR-019 records the durable decision.
