# ADR-0032: Kernel-Native Observation and Sensor Subsystem

- **Status:** Accepted
- **Date:** 2026-09-30

## Context

The runtime has an `Observation` entity and a `put_observation` write path, but
nothing produces observations: `crates/argus-domain/src/observation.rs` is a
value object only, and no crate reads `/proc`, `/sys`, cgroup v2, PSI, or
Netlink. The "Observe" stage of the control loop therefore has no input, and
CAP-1..3 of the Autonomous Operations Brain cannot be met. The request is to
observe a Linux host continuously — CPU, memory, swap, load, filesystem, disk
I/O, network, PSI, kernel state, devices, temperatures/GPU, boot/OOM, plus a
live process inventory, systemd units, and containers — while honoring
Principle 5 (prefer kernel interfaces over parsed CLI output).

## Decision

### 1. Two new crates split the concern

- **`argus-sensors`** — pure, synchronous, kernel-native readers. Each sensor is
  a small, independent module that reads one authoritative interface and returns
  typed structs: `/proc` (meminfo, loadavg, stat, vmstat, pressure), `/sys`
  (devices, block, class/thermal), cgroup v2 (current/stat files). No tokio, no
  IO beyond reading files; fully unit-testable against fixtures.
- **`argus-observe`** — the observation coordinator. Owns the periodic and
  event-driven collection schedule, the live process inventory (and its
  lifecycle diffing), and converts sensor structs into domain `Observation`
  values. This is where `argus-sensors` meets `argus-domain` and `argus-events`.

### 2. Sensors are data, not decisions

Sensors produce `Observation` (immutable evidence) and domain events only. They
never emit a plan, a risk verdict, or an action. Correlation and anomaly
detection are downstream (`argus-correlate`, `argus-anomaly`), never in a
sensor.

### 3. Kernel-native first, structured-API second, CLI parsing never

Reading order: `/proc` + `/sys` + cgroup v2 + PSI + Netlink → systemd/D-Bus
(`zbus`) → container runtimes via their structured APIs → never parse `ps` /
`top` / `free` / `df` / `ip` output. Where a structured userspace API exists and
a raw kernel reader is not justified (systemd units, container runtimes), the
structured API is used.

### 4. Graceful degradation per sensor

Each sensor is independent; a sensor that cannot run (unsupported kernel,
missing file, no permission) reports `degraded` and contributes nothing, rather
than failing the observation loop. The coordinator keeps collecting the sensors
that work.

### 5. The process inventory lives in `argus-observe`

The live process inventory (PID, PPID, user, exe, cmdline, hash, CPU/mem,
threads, fds, sockets, ports, capabilities, namespaces, cgroup, container
association, systemd unit, start time, parent/child, lifecycle) is assembled
from `/proc/<pid>/*` by `argus-observe` and diffed between ticks to emit
lifecycle change events. pidfd is used for lifecycle/signal operations where
supported.

## Consequences

### Positive

- The Observe stage becomes real and deterministic; testable without a host via
  `/proc`-tree fixtures.
- Sensors are independently replaceable and each degrades alone.
- Kernel-native data honors Principle 5 and keeps the core infrastructure-aware
  without coupling to any one kernel version.

### Negative

- Two new crates; a scheduling/lifecycle coordinator to build and test.
- Kernel interfaces vary across kernels/distros; sensors must be tolerant of
  missing fields (the "where available" contract).

## Implementation Principles

1. A sensor reads one authoritative interface and returns typed data — no more.
2. Sensors never make decisions; they only produce evidence and events.
3. Prefer a kernel/structured API; never parse human-oriented CLI output.
4. Each sensor degrades independently; one failure never breaks observation.
5. Observations stay immutable and are never overwritten by inference.

## Reference

- `.specify/memory/constitution.md` — Principle 5 (kernel-native), 11 (graceful
  degradation), 9 (deterministic control plane).
- `docs/architecture/domain-model.md` — §4 Linux Kernel Substrate, §3.4 Process.
- `_bmad-output/specs/spec-autonomous-operations-brain/SPEC.md` — CAP-1..3.
- `crates/argus-domain/src/observation.rs`, `crates/argus-events/src/lib.rs`.
