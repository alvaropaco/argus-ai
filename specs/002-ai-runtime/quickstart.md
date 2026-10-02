# Quickstart / Validation Guide: ARGUS AI Runtime

**Spec:** `specs/002-ai-runtime/spec.md`
**Status:** Validated
**Date:** 2026-10-02

Runnable scenarios that prove the reasoning gateway and control loop work.
Implementation details belong in `tasks.md`; this is a validation/run guide.
References: [data-model.md](data-model.md), [contracts/](contracts/).

## Validation results (2026-10-02)

Scenarios 1–6 were executed on the development host (macOS, Rust stable
1.99.0). The full workspace suite passed: **818 tests, 0 failures**, plus
`cargo build --workspace` clean. Scenarios 7–8 require a Linux/systemd host
and the `simple-jev` sidecar; they were not executable in this environment
and remain the opt-in steps below. With this pass, all acceptance criteria
except AC-009 (Linux-host demonstration, scenario 7/8) are covered by
automated tests.

## Prerequisites

- Linux/Unix host, Rust stable toolchain, no Kubernetes/NATS/eBPF required.
- A decision engine: a local `simple-jev` sidecar (optional for most scenarios —
  the deterministic fake covers the rest), or a hosted JEV endpoint.

## Scenario 1 — Build and lint

```bash
cargo build --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

**Expected:** clean build; decision schema, confidence-threshold, autonomy-gating,
policy, and event tests pass.
**Result (2026-10-02):** build clean; `cargo test --workspace` 818 passed / 0
failed; fmt and clippy clean (see the spec's verification checklist).

## Scenario 2 — Decision schema round-trip (no engine)

```bash
cargo test -p argus-ai-core decision
```

**Expected:** `DecisionQuestion`/`DecisionAnswer` serialize/deserialize per
[contracts/decision-protocol.md](contracts/decision-protocol.md); malformed
answers (missing labels, bad ranges) are rejected.

## Scenario 3 — Reasoning gateway with a fake decision engine

```bash
cargo test -p argus-ai-core gateway
```

**Expected:** given a bounded evidence context, the gateway authors structured
`choice`/`score`/`noul` questions, maps the fake answers to a typed `Hypothesis`
and `Plan`, and applies the confidence threshold (below-threshold ⇒ no plan).

## Scenario 4 — Policy gating

```bash
cargo test -p argus-policy
```

**Expected:** a proposed action produces exactly `allow`/`deny`/`require approval`;
a denied action never reaches the executor; an approval-required action stays
blocked until approved.

## Scenario 5 — Autonomy modes

```bash
cargo test -p argus-ai-core autonomy
```

**Expected:** `ObserveOnly` never proposes execution; `Propose` blocks all actions
pending approval; `Assisted` allows only explicitly permitted low-risk actions.

## Scenario 2 — Decision schema round-trip (no engine)

```bash
cargo test -p argus-ai-core decision
```

**Expected:** `DecisionQuestion`/`DecisionAnswer` serialize/deserialize per
[contracts/decision-protocol.md](contracts/decision-protocol.md); malformed
answers (missing labels, bad ranges) are rejected.
**Result (2026-10-02):** 60 passed / 0 failed.

## Scenario 3 — Reasoning gateway with a fake decision engine

```bash
cargo test -p argus-ai-core gateway
```

**Expected:** given a bounded evidence context, the gateway authors structured
`choice`/`score`/`noul` questions, maps the fake answers to a typed `Hypothesis`
and `Plan`, and applies the confidence threshold (below-threshold ⇒ no plan).
**Result (2026-10-02):** 7 passed / 0 failed.

## Scenario 4 — Policy gating

```bash
cargo test -p argus-policy
```

**Expected:** a proposed action produces exactly `allow`/`deny`/`require approval`;
a denied action never reaches the executor; an approval-required action stays
blocked until approved.
**Result (2026-10-02):** 19 passed / 0 failed.

## Scenario 5 — Autonomy modes

```bash
cargo test -p argus-ai-core autonomy
```

**Expected:** `ObserveOnly` never proposes execution; `Propose` blocks all actions
pending approval; `Assisted` allows only explicitly permitted low-risk actions.
**Result (2026-10-02):** 3 passed / 0 failed.

## Scenario 6 — Provider degradation and health

```bash
cargo test -p argus-daemon --test end_to_end unavailable_engine
cargo test -p argus-daemon --lib provider_health
```

**Expected:** with the decision engine unreachable, the loop fails closed (no
plan is fabricated), the daemon's provider health flips to `degraded`, a
`provider.degraded` event is published on the `Ready → Degraded` transition,
and `status.get` reports `"provider": { "status": "degraded", ... }` until a
successful step marks it ready again (FR-009, AC-008).
**Result (2026-10-02):** both filters passed (fail-closed loop, health
transitions, and status reporting).

## Scenario 6b — Reasoning history and operator surfaces

```bash
cargo test -p argus-state                      # hypotheses/plans/executions persist
cargo test -p argus-daemon --test end_to_end   # the loop records them end to end
```

**Expected:** every reasoning step persists its hypothesis, plan, and
executions behind `DomainRepository` keyed by the correlation id (FR-008);
the daemon exposes them over IPC as `plan.list` and `audit.list`, alongside
`approval.list`/`approval.grant`/`approval.deny`, and the TUI shows views
**6 Plans** (pending approvals + plan history) and **7 Audit**.
**Result (2026-10-02):** all state, IPC, and daemon tests passed.

## Scenario 7 — End-to-end loop with a local `simple-jev` sidecar (opt-in, Linux)

```bash
# terminal 1: start the decision engine
git clone https://github.com/featherless-ai/simple-jev.git && cd simple-jev
python3.13 -m venv .venv && source .venv/bin/activate
python -m pip install -e './hf-server'
python hf-server/hf_server.py --model Qwen/Qwen3.5-0.8B --device cpu --dtype float32

# terminal 2: point ARGUS at the sidecar and exercise the loop
argusd --decision-endpoint http://127.0.0.1:8000
argus status            # health + provider health
argus propose <fixture> # produce a plan for a simulated incident
argus approve <plan>    # (or deny)
```

**Expected:** a validated `Plan` is produced and, when approved within the autonomy
boundary, a low-risk typed action is executed and its evidence recorded
(AC-001, AC-005, AC-006).
**Result (2026-10-02):** not executed — requires the sidecar and a Linux host.
The deterministic-fake equivalent (loop → policy → executor → validate →
record, AC-002/003/006/008) is covered by `argus-daemon`'s `end_to_end` and
`brain_e2e` suites, which passed.

## Scenario 8 — Service action via systemd (Linux only)

```bash
# with argusd running and a policy that permits LowRisk service actions
argus exec host.service.restart '{"unit":"nginx.service"}'
```

**Expected:** the action is routed through policy → executor, performed via
systemd/D-Bus, and `action.executed` + evidence are recorded. Skips gracefully on
non-systemd hosts.
**Result (2026-10-02):** not executed on this (non-systemd) host; the policy,
guardrail, and no-bypass behavior is covered by `argus-daemon`'s policy and
capability suites, which passed.

---

Full detail: `data-model.md`, `contracts/decision-protocol.md`,
`contracts/capabilities.md`, `contracts/events.md`.
