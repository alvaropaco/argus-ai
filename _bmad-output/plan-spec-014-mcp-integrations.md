---
title: 'Spec 014 — MCP integrations: external tools as capabilities'
type: 'feature'
ticket: ''
created: '2026-10-11'
status: 'in-review'
baseline_revision: '653a1276de9110ae932ad4f8b27a85e9d695af50'
route: 'full'
route_source: 'auto'
review: 'quick'
review_source: 'pinned'
lenses_ran: ['quick']
followup_review_recommended: false
context: ['argus-ai/specs/014-mcp-integrations/spec.md', 'argus-ai/docs/adr/0044-mcp-tools-are-capabilities.md']
warnings: []
deferred: []
---

<intent-contract>

## Intent

**Problem:** The capability registry is closed to everything except compiled-in Rust — the wall the MCP protocol exists to remove. External tools (metrics, databases, the ecosystem's MCP servers) can neither become evidence nor actions without a release, so the organism is perception- and action-limited to what the kernel exposes.

**Approach:** A new `argus-mcp` crate (rmcp-backed, behind the existing `McpRuntime` trait) connects the daemon to configured MCP servers (stdio subprocess or streamable HTTP), discovers their tools, and registers each as a typed capability — `mcp.<server>.<tool>`, the tool's own JSON Schema, **HighRisk by default** (per-tool risk override in config), approval required — whose calls route through the ordinary policy → escalation → approval → budget → ledger boundary with the tool's output as bounded execution evidence. Per-server isolation: independent reconnect with backoff, typed failures, tools stop being offered while the server is down.

## Boundaries & Constraints

**Always:** deterministic namespaced ids (`mcp.<server>.<tool>`); HighRisk default risk with per-tool config override as the ONLY trust mechanism (server self-reports/annotations never change authority); every call crosses the ordinary boundary (no MCP message bypasses policy/approval/budget/ledger); tool output returns as bounded execution evidence (size-capped); per-server independent lifecycle (reconnect with backoff, never blocking the loop or other servers); fail-closed everywhere (crash/timeout/disconnect → typed errors, tools stop being offered); tool calls bounded by timeout (default 30s, per-server config); zero `[mcp]` servers → byte-identical behavior; the `McpRuntime` trait stays the domain seam (argus-mcp implements it; the domain never depends on rmcp).

**Never:** no trusting server self-reports (annotations/hints) for authority; no auto-execution of HighRisk tools at any level (the matrix is untouched); no Argus-as-MCP-server (outbound, second phase); no MCP resources/prompts as evidence in this spec; no per-tool budget overrides; no command execution for the integration itself (stdio servers are subprocesses the runtime owns — that is the transport, not a shell); do not commit `.mimosa/` or user artifacts.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Server connects | configured stdio/http server, enabled | Tools discovered → `mcp.<server>.<tool>` capabilities registered (HighRisk default, tool schema), logged + traced | Discover failure → server marked failed, retry with backoff |
| Tool called | plan step (or boundary-authorized call) targets `mcp.<server>.<tool>` | Runtime invokes the tool with the step's arguments; output ≤ cap becomes evidence; Completed | Timeout (30s default) / crash / disconnect → typed error → Failed |
| Server crashes | subprocess exits / HTTP unreachable mid-life | That server's tools fail typed; other servers unaffected; reconnect with backoff; tools re-offered on return | Repeated failures → backoff grows, logged |
| Server disabled/removed | config change (restart) | Capabilities unregistered; calls refused as unsupported | In-flight call fails typed |
| Risk override | `[mcp.servers.X.tools.Y] risk = "controlled"` | The tool's descriptor carries `Controlled` (the autonomy matrix treats it accordingly) | Absent → HighRisk |
| Zero servers | no `[mcp]` section | Byte-identical behavior; no runtime spawned | — |
| Duplicate tool ids | two servers expose the same tool | Namespaced by server — no collision by construction | — |
| Oversized output | tool returns > evidence cap | Output truncated to the cap with a `truncated: true` marker in evidence | — |

</intent-contract>

## Code Map

**argus-ai** (test: `export PATH="$HOME/.cargo/bin:$PATH"; cargo test --locked --workspace --all-targets`; baseline 1185 passed)
- `crates/argus-domain/src/mcp.rs` — the `McpRuntime` trait seam (register/capabilities). NEW `crates/argus-mcp/` crate: rmcp-backed client runtime. Check crates.io for the current `rmcp` version (the official modelcontextprotocol Rust SDK; client via `rmcp::ServiceExt`, transports `TokioChildProcess` (stdio) and the streamable-HTTP client; the crate also ships test utilities). Add `rmcp` + `tokio` to the new crate only.
- `crates/argus-daemon/src/config.rs` — `[mcp]` section: `servers: Vec<McpServerConfig>` (`name`, `transport`: `stdio{command, args}` | `http{url}`, `enabled: bool = false`, `timeout_seconds = 30` clamp 5–300, `tools: Vec<{name, risk}>` overrides + optional allowlist). Clone the `AutonomyConfig` pattern; names validated `[a-z0-9.-]`.
- Registration: `runtime.rs` `bootstrap_capabilities()/bootstrap_descriptors()` — after the bootstrap set, the MCP runtime's discovered tools register as descriptors (id `mcp.<server>.<tool>` — check `CapabilityId::new` accepts the namespace; if the id grammar rejects `mcp.`, extend the grammar test-first with the constitution's naming rules). Schema = the tool's `inputSchema` verbatim; risk per override or HighRisk; approval `true`; reversibility `None`; blast `Host`. The registry behind the bootstrap set must accept LATE registration (spec 011's registry is built at init — investigate `CapabilityRegistry` mutability in `Daemon`; the pattern from `init_with_extra_capabilities` (spec 011) may need a runtime-registration method, or the MCP set registers at init after connecting with a startup timeout).
- Executor wiring: `RemediationExecutor` (spec 012's pattern — ports per family) gains an `McpToolController` port backed by the runtime: call = invoke tool with the step's arguments; output JSON-serialized, size-capped (e.g. 8 KiB) as execution evidence. `risk_for`/`handles()`/`is_mcp_capability` arms: `mcp.` prefix.
- Lifecycle: per-server task (spawned beside the brain loop) — connect → discover → register → watch; on disconnect: unregister or mark (keep registered failing, per spec FR-004 default) → reconnect backoff. All state changes → `tracing` + ledger trace events optional.
- Tests: `argus-mcp` unit tests against rmcp's in-process test utilities (an in-memory server pair); daemon tests with a FAKE McpRuntime (the trait seam — deterministic tools, no subprocess in unit tests); one integration test with a REAL stdio subprocess (a tiny JSON-RPC echo server implemented in the test as a rust binary target or via rmcp's server macro) proving connect → register → call → evidence end-to-end.

**app-platform-argus-ai** — no required changes: the tools publish through the existing capabilities surface and execute through the ledger/Activity flows; the cloud's host-changing policy denies them over the control channel by the existing rule (verify the cloud policy test still passes with mcp.* ids in the published schema — `apps/api` validates against the published schema only).

## Tasks & Acceptance

**Execution:**
- [ ] NEW `crates/argus-mcp` — rmcp client runtime behind `McpRuntime`: connect (stdio/http), discover tools, register descriptors, invoke with timeout+output cap, per-server lifecycle (backoff reconnect), unit tests against rmcp's in-process pair
- [ ] `crates/argus-daemon/src/config.rs` — `[mcp]` servers config (validated names, clamped timeout, risk overrides, allowlist) + tests
- [ ] `crates/argus-daemon` — startup wiring (connect configured servers → register tools into the registry beside the bootstrap set), `McpToolController` executor port + `mcp.` arms (`handles`/`risk_for`), lifecycle tasks + typed failures
- [ ] `crates/argus-domain` — extend the id grammar/naming rules for `mcp.<server>.<tool>` if needed (test-first), the trait seam respected (no rmcp in domain/daemon deps)
- [ ] integration test — a real stdio MCP subprocess (rust test binary): connect → register → boundary-authorized call → evidence → crash → typed failure → recovery
- [x] both repos — full verification; DEPLOYED 2026-10-11 (daemon only; cloud unchanged): argus-ai `87aaa44` + review fixes `b7c8ac0` + deadlock fix `13ae103`. **Live-proven with a real stdio MCP server on the VPS** (a minimal python JSON-RPC echo at /etc/argus/mcp/echo-server.py — ProtectHome=true hides /root, so drill artifacts live under /etc/argus/mcp/): `mcp server connected; tools adopted server=test tools=1`, **`mcp.test.echo` in `argus capabilities` and the cloud surface**, and an attributed procedure plan (`procedure: mcp-memory-echo` driving the external tool) crossed the boundary at L0 → Denied (correct: nothing executes at L0). **Two live-only bugs found and fixed during bring-up**: (1) the deployment was initially missed (the 012-era binary stayed swapped — the [mcp] section was silently ignored by serde unknown-field semantics; lesson: verify the SWAPPED BINARY is the new build, not just the build); (2) **rustls crash-loop**: rmcp's reqwest stack enabled aws-lc-rs beside tungstenite's ring — rustls 0.23 panicked at builder time (the cloud dial) → fixed by installing the ring provider as main's first statement (`b7c8ac0`); (3) a recovery deadlock from spec 010's validated-work gate (it applied to L0→L2 where nothing ever executes — one SafeMode = L0 forever) → the requirement now applies to escalation above L2 only (`13ae103`), and the base re-earn is live (clean-cycle counter climbing, with honest resets on provider hiccups). Runbook config notes: runbooks_dir landed in production ([brain] runbooks_dir=/etc/argus/runbooks); [mcp] grammar is array-of-tables with transport as an internally-tagged table (`[mcp.servers.transport] type="stdio"`); overrides want `name`, not `tool`.

**Acceptance Criteria:**
- Given a configured enabled stdio server, when the daemon starts, then its tools register as `mcp.<server>.<tool>` capabilities with the tool's schema and HighRisk default, visible in `argus` capabilities and the cloud's published surface (AC-001).
- Given a boundary-authorized call to the tool, when it executes, then the runtime invokes the tool and the result lands as bounded evidence through the ledger (AC-002).
- Given a per-tool risk override to `controlled`, when the autonomy matrix evaluates the step at L4, then it may auto-execute within budget; without the override it pauses at every level (AC-002's override semantics).
- Given the server crashes mid-life, when the next call arrives, then a typed error fails the step and the daemon stays healthy; after the server returns, the tools are re-offered (AC-003).
- Given the server disabled in config and a restart, when capabilities are listed, then the tools are unregistered and calls refuse as unsupported (AC-004).
- Given zero `[mcp]` servers, when the daemon starts, then behavior is byte-identical to today (AC-006).

## Implementation Notes

- Implemented by the full-route subagent (2026-10-11): new `crates/argus-mcp` (rmcp 3.5.1 client runtime — stdio via TokioChildProcess + streamable HTTP; per-server slots; connect/discover/adopt; invoke with per-server timeout and 8 KiB evidence cap + `truncated` marker; typed errors incl. ConnectTimeout/InvalidArguments; HighRisk-by-default descriptors from the tool's own inputSchema; annotations never read), the `McpToolController` executor port (mcp. routing in RemediationExecutor), `[mcp]` config (servers, stdio/http, enabled=false default, timeout clamp 5-300, per-tool risk overrides, tools allowlist), daemon startup wiring + per-server lifecycle tasks (probe/reconnect with 1-64s backoff), policy: the local remediation evaluator admits the `mcp.` PREFIX (plain `new()` — the cloud channel — holds no prefix, so cloud-issued mcp.* is denied by the host-changing rule), id grammar extended test-first for `mcp.<server>.<tool>`, registry behind RwLock with late registration, rmcp in the new crate only (domain/daemon stay implementation-free).
- Review (quick lens, 2026-10-11): 9 findings — 4 medium (cloud surface never refreshed; double+sequential startup connects; registry guard held across execution; missing schema gate + silent `{}` args), 5 low — ALL patched and re-verified (1221 passed, +2 new tests; clippy/fmt clean).
- Deploy-relevant: rmcp 3.5 requires rustc >= 1.88 (workspace rust-version moved 1.85 → 1.88) — the VPS toolchain (1.98.1) satisfies it. `reqwest` added as a direct dep of argus-mcp only.

## Auto Run Result

- **Summary:** Spec 014 opened the MCP track: configured external MCP servers (stdio subprocess or streamable HTTP) connect at startup, their tools register as typed capabilities (`mcp.<server>.<tool>`, the tool's own JSON Schema, HighRisk by default with per-tool config override, approval required), and calls route through the ordinary policy → escalation → approval → budget → ledger boundary with bounded evidence — per-server isolated lifecycle with backoff reconnects, fail-closed everywhere, zero-config byte-identical behavior. The cloud surface now refreshes on late registration.
- **Files changed:** argus-ai — new `crates/argus-mcp` (rmcp runtime + error + stdio subprocess integration test), argus-domain (id grammar, McpRuntime reshaped, registry Clone + late registration), argus-executor (McpToolController port + mcp. routing), argus-policy (prefix admission local-only), argus-daemon (`[mcp]` config, startup wiring, lifecycle tasks, surface refresh, tests incl. a real-subprocess e2e). app-platform: none.
- **Review findings:** quick lens, 9 findings — 8 patched (4 medium, 4 low), 1 folded; 0 rejected. All re-verified.
- **Follow-up review recommendation: true** — four medium entries patched. Specific unverified risks: the concurrent startup path and the lifecycle adoption are integration-tested but not yet live-exercised; the live VPS check (real stdio server → register → procedure → grant → evidence) is outstanding.
- **Verification performed:** argus-ai `cargo test --locked --workspace --all-targets` 1221 passed / 2 ignored post-patches (pre-patch 1219; +34 spec tests, +2 review tests), clippy 0 warnings, fmt clean. app-platform untouched (8/8, 12/12). Diff reviewed by the orchestrator + quick lens.
- **Residual risks:** late-registered tools reach the cloud only after the next publish tick or session (documented); a `controlled` override lifts per-invocation approval (the operator wrote it — ADR-0044 §1); rmcp MSRV 1.88 on deploy hosts; the live VPS drill is the remaining gate.

## Plan Change Log

## Review Triage Log

### 2026-10-11 — Review pass (quick lens)
- verdicts: 9 findings — high 0, medium 4, low 5, false 0, maybe-false 0
- findings:
  - `[medium]` `[patch]` the cloud surface never sees late-registered tools (the supervisor's capability list is a one-shot snapshot; the advertised retry path registers locally only) — verified. Fix: a refreshable CapabilitySurface + republish on (re)registration.
  - `[medium]` `[patch]` every enabled server connects TWICE at startup (startup_descriptors then the lifecycle's first connect kills the first session), and the startup connects run SEQUENTIALLY (startup delayed by the sum of timeouts, stalling argusd readiness; the success log printed a backoff on attempt 0) — verified. Fix: `connect_startup` connects once per server concurrently, adopts live slots; the lifecycle gates on `has_connection` and only reconnects down slots; attempt-0 log fixed.
  - `[medium]` `[patch]` the registry read guard held across synchronous execution (reclaim/vacuum/mcp calls take seconds), blocking registration writes — violates the field's documented discipline. Fix: `registry_snapshot()` clone; policy and execution run against the owned snapshot.
  - `[medium]` `[patch]` the claimed schema gate for MCP arguments does not exist on the plan-execution path (input_matches only at procedure construction and the dispatch path, which denies all mcp.* under the plain evaluator), and non-object arguments were silently converted to `{}` — a provider-proposed plan reaches the external tool unvalidated — verified. Fix: `invoke` resolves the adopted descriptor, rejects non-object arguments with a typed `InvalidArguments`, and validates via `input_matches` against the adopted schema before any wire bytes; overstated docs corrected.
  - `[low]` `[patch]` AC-005 had no test evidence for the new family. Fix: the subprocess e2e asserts the ledger ActionEvent for the call.
  - `[low]` `[patch]` AC-004's daemon test substituted an empty-table fake; the real enabled=false chain had no daemon-level coverage. Fix: a real-config disabled-server test (no wiring, no tools, call refused).
  - `[low]` `[patch]` the allowlist grammar diverged from FR-001 (`allow` vs `tools = [...]`). Fix: overrides under their own key; the allowlist is `tools` per the spec (unreleased config).
  - `[low]` `[patch]` mangled doc comment (`here.///`). Fix: restored.
  - `[low]` `[folded]` the attempt-0 backoff log folded into the double-connect fix.
- patches applied (re-engaged step-03 implementer; scoped suites green, full re-verification on the orchestrator side): refreshable CapabilitySurface with mid-session republish; concurrent single-connect startup adopted by the lifecycle; registry snapshot clone before execution; schema gate + typed InvalidArguments in invoke; ledger assertion in the subprocess e2e; real-config disabled-server test; config grammar per FR-001; doc comment restored.

### 2026-10-11 — Review pass (quick lens)
- verdicts: 9 findings — high 0, medium 4, low 5, false 0, maybe-false 0
- findings:
  - `[medium]` `[patch]` the cloud surface never sees late-registered tools: the supervisor's capability list is a one-shot `Arc<Vec>` snapshot built at startup, and nothing refreshes it — the advertised retry path ("the lifecycle task will retry") registers into the local registry only — verified (`main.rs:116`, `cloud.rs:1279`). Fix: the snapshot becomes refreshable and a successful (re)registration triggers `publish_capabilities`.
  - `[medium]` `[patch]` every enabled server connects TWICE at startup (startup_descriptors then the lifecycle's first connect, dropping the first healthy session — the dropped subprocess kill is by design of `attach`), and the connects are SEQUENTIAL (startup delayed by the sum of timeouts, stalling argusd readiness; the success log prints a backoff on attempt 0) — verified. Fix: one connect per server, concurrent, adopted by the lifecycle (which owns reconnects after).
  - `[medium]` `[patch]` the registry read guard is held across synchronous execution (journal vacuum/reclaim/MCP calls take seconds), blocking MCP registration writes for the whole execution — violates the field's own doc — verified (`runtime.rs:796-805`). Fix: short guards; clone what authorization needs, drop the guard before executing.
  - `[medium]` `[patch]` the claimed schema gate for MCP arguments does not exist on the plan-execution path (`input_matches` only at procedure construction and the dispatch path, which denies all mcp.* under the plain evaluator), and non-object arguments are silently converted to `{}` — a provider-proposed plan reaches the external tool unvalidated — verified. Fix: `call_mcp_tool` validates arguments against the registered descriptor's schema (fail-closed typed error) and rejects non-object arguments instead of silently emptying them.
  - `[low]` `[patch]` AC-005 has no test evidence for the new family (no ledger/budget assertion on an mcp.* call). Fix: the subprocess e2e asserts the ledger ActionEvent lands for the call.
  - `[low]` `[patch]` AC-004's daemon test substitutes an empty-table fake for "the server was disabled" — the actual chain (enabled=false → no wiring → call refused) has no daemon-level coverage, and the production wiring is untested. Fix: a daemon-level disabled-server test (config → init → no tools registered, call refused as unsupported).
  - `[low]` `[patch]` the allowlist grammar diverges from the spec (`allow` key vs FR-001's `tools = [...]`; `tools` is taken by the overrides array). Fix: overrides move under their own key and the allowlist becomes `tools` per the spec — unreleased config, no compat concern.
  - `[low]` `[patch]` a mangled doc comment (`here.///`) left in config.rs by the insertion. Fix: restore the paragraph break.
  - `[low]` `[folded]` the success log printing a backoff on attempt 0 folds into the double-connect fix.

## Design Notes

- The trust gradient is config, not the wire: HighRisk default + per-tool override is the operator writing law (ADR-0044 §1).
- The `mcp.` id namespace keeps tool ids collision-free by construction and greppable in the ledger.
- rmcp's in-process test pair (client + server in one tokio test) is how the runtime is tested without subprocesses; the ONE subprocess integration test proves the transport.
- Output capping: the tool's JSON output, serialized and truncated to the evidence cap with a marker — bounded memory, honest evidence.

## Verification

**Commands:**
- argus-ai: `export PATH="$HOME/.cargo/bin:$PATH"; cargo test --locked --workspace --all-targets` — expected: all pass (baseline 1185), `cargo clippy --workspace --all-targets` 0 warnings, `cargo fmt --check` clean.
- app-platform: `pnpm build && pnpm test` — expected: 8/8, 12/12 (untouched; verify no drift).

**Manual checks (post-deploy, cloud first):**
- Configure a stdio MCP server on the VPS (e.g. a minimal echo tool), restart, and watch `argus capabilities` + the cloud's published surface gain `mcp.<server>.*` entries; a promoted runbook driving the tool pauses → grant → executes → evidence.
