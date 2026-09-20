# Quickstart / Validation Guide: ARGUS AI Runtime

**Spec:** `specs/002-ai-runtime/spec.md`
**Status:** Draft
**Date:** 2026-09-19

Runnable scenarios that prove the reasoning gateway and control loop work.
Implementation details belong in `tasks.md`; this is a validation/run guide.
References: [data-model.md](data-model.md), [contracts/](contracts/).

## Prerequisites

- Linux/Unix host, Rust stable toolchain, no Kubernetes/NATS/eBPF required.
- A decision engine: a local `simple-jev` sidecar (optional for most scenarios —
  the deterministic fake covers the rest), or a hosted JEV endpoint.

## Scenario 1 — Build and lint

```bash
cargo build --workspace
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

**Expected:** clean build; decision schema, confidence-threshold, autonomy-gating,
policy, and event tests pass.

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

## Scenario 6 — Provider degradation

```bash
cargo test -p argus-daemon provider_degraded
```

**Expected:** with the decision engine unreachable, the runtime reports
`provider.degraded` and does not fabricate a plan.

## Scenario 7 — End-to-end loop with a local `simple-jev` sidecar (opt-in)

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

## Scenario 8 — Service action via systemd (Linux only)

```bash
# with argusd running and a policy that permits LowRisk service actions
argus exec host.service.restart '{"unit":"nginx.service"}'
```

**Expected:** the action is routed through policy → executor, performed via
systemd/D-Bus, and `action.executed` + evidence are recorded. Skips gracefully on
non-systemd hosts.

---

Full detail: `data-model.md`, `contracts/decision-protocol.md`,
`contracts/capabilities.md`, `contracts/events.md`.
