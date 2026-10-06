# Live Linux Validation — 2026-10-06

Host: `mail.0xcloud.net` (root VPS) — Ubuntu 24.04.4 LTS, kernel
6.8.0-138-generic x86_64, systemd 255, Docker 29.1.3, cgroup v2 unified
hierarchy, 18 cores / 94 GiB. A production `argusd` (Sep-29 build, spec-002
era) was already deployed here as a systemd service; this session upgraded
it to `7f8f06c` and validated the full stack against the live host.

## What was validated

### Toolchain and suite
- Full workspace built on-target (release, 1m30s on 18 cores).
- `cargo test --workspace --release`: **1004 passed / 0 failed** on Linux
  (identical count to macOS after one hermeticity fix below).

### Deployment
- New `argusd`/`argus` installed over the running service (binary swap
  via rename; `Text file busy` requires stopping the unit first).
- SQLite state survived the upgrade (same environment id); daemon came
  back `ready` with the cloud enrollment drop-in intact.
- Unit change (the only one, justified per ADR-0021 as the unit's comments
  demand): `ReadWritePaths=/etc/argus /sys/fs/cgroup` — without it,
  `ProtectControlGroups=` makes every `host.cgroup.freeze` EROFS.

### Live IPC (protocol 0.6.0)
- `capabilities` lists all 23 registered capabilities including the M3
  remediation and M4 k8s families; `k8s.node.drain` and
  `k8s.workload.reschedule` correctly absent (deny-by-default).
- `sentinel.get`: healthy / safe_mode none / provider ready over the real
  socket.
- `report.generate` (`daily`): deterministic text over real tables.

### Live effects (via `crates/argus-daemon/examples/live_effects.rs`,
### all against disposable targets only)
- **cgroup freeze** (`host.cgroup.freeze`, L4, classified BestEffort):
  executed through governor → policy → escalation-gate → real kernel
  write; `/sys/fs/cgroup/.../cgroup.freeze` read `1` afterwards.
- **thaw as a standalone plan was policy-denied** — thaw is
  rollback-scoped in the bootstrap policy; a bare thaw plan refuses.
  (Verified by design, not by accident.)
- **container restart** (`container.restart` on a stopped test container):
  L4 pause (no declared rollback → not AUTO-FIX) → grant → resume → real
  Docker-socket restart → post-execution validation (`running`, fresh
  StartedAt). A running container idempotently no-ops
  (`already_desired`) — the desired-state semantics working as designed.
- **service restart** (`host.service.restart`, zbus D-Bus): desired-state
  read from live systemd; a restart of a vanished transient unit
  propagated systemd's real error through the executor cleanly (the
  D-Bus write path proven by the round-trip).
- **process signal** (`host.process.signal`, L4 + approval): SIGSTOP
  delivered via kill(2); process state `Ss` → `Ts` observed.

## Bugs found live (and fixed)

1. **Chunked Docker responses** (`030f034`): the Docker daemon answers
   `/containers/{id}/json` with `Transfer-Encoding: chunked`; both
   hand-rolled HTTP parsers fed the chunk framing to serde — every live
   inspect failed with "trailing characters". Fixture sockets with
   identity-framed bodies could never catch it. Fixed with a shared
   `http_parse` module (chunk decoding, RFC 9112 §7.1) used by both the
   sync executor client and the async observer client.
2. **Non-hermetic config test** (`6bbafa5`): `load(None)` walks the
   machine's search paths; on a host with a deployed `/root/argus.toml`
   the "no file anywhere" test failed. Rewritten to the real invariant.

## Not yet validated live

- Kubernetes (no cluster on this host); the kube backend remains
  compile-checked only, and PATCH-based effects still degrade.
- The sentinel's risk/prediction/incident fields (no live telemetry
  source is wired into the daemon yet — they render empty by design).
- The reasoning loop against the configured DeepSeek provider (the daemon
  is enrolled; no diagnosis was triggered during this session).

Commits: `e107b09` (live harness), `6bbafa5` (hermetic test), `2c1da96`/
`8d74a2b`/`a744d74` (harness round-trips), `030f034` (chunked fix),
`7f8f06c` (cleanup).
