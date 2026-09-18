# ARGUS AI

**Autonomous infrastructure operations runtime for Linux/Unix.**

ARGUS continuously discovers infrastructure, builds an evidence-backed model of
the environment, reasons about desired and observed state, plans changes,
authorizes actions, executes permitted operations, validates outcomes, and
maintains the environment autonomously.

ARGUS is **not** a Kubernetes-only platform, a generic agent-orchestration
framework, or a monitoring product. It treats the Linux kernel as the primary
substrate and Kubernetes as an optional control plane layered on top.

> **Status:** Bootstrap. This release establishes the secure runtime skeleton —
> daemon, CLI/TUI, IPC, policy/executor boundary, state, events, observability,
> plugin/MCP boundary, and installer — on which discovery, remediation, and
> autonomous operations are built incrementally.

---

## Table of contents

- [Architecture](#architecture)
- [Requirements](#requirements)
- [Installation](#installation)
  - [Bash installer](#bash-installer)
  - [Cargo (crates.io)](#cargo-cratesio)
  - [APT (Debian/Ubuntu)](#apt-debianubuntu)
  - [Build from source](#build-from-source)
- [Usage](#usage)
  - [Start the daemon](#start-the-daemon)
  - [Query the daemon](#query-the-daemon)
  - [Interactive TUI](#interactive-tui)
  - [Self-update](#self-update)
- [Configuration](#configuration)
- [Security model](#security-model)
- [Development](#development)
- [Documentation](#documentation)
- [License](#license)

---

## Architecture

```text
argus (CLI/TUI, unprivileged)
      │  Unix-domain socket (SO_PEERCRED)
      ▼
argusd (privileged, minimized)
      ├── policy  ── PolicyEvaluator (Cedar-ready)
      ├── executor ── typed, policy-authorized operations
      ├── state   ── DomainRepository (SQLite interim, LanceDB optional)
      └── events  ── local event bus (NATS-ready)
```

Key invariants:

- **AI output is data, never authority.** Every privileged operation passes
  through the policy engine and the typed executor.
- **Kernel-native.** `/proc`, `/sys`, cgroup v2, namespaces, Netlink, pidfd,
  PSI, and eBPF/BTF are preferred over parsing CLI output.
- **Works without Kubernetes, NATS, or eBPF.** Optional components degrade
  cleanly on a single host.
- **Evidence is immutable.** Observed, inferred, desired, and historical state
  remain distinct.

See [`docs/architecture/`](docs/architecture/) and [`docs/adr/`](docs/adr/) for
the full architecture and the decision record.

## Requirements

- Linux/Unix host (x86_64 or aarch64).
- systemd (for the packaged service; not required to run the binaries).
- No external database, NATS, Kubernetes, or eBPF required.

## Installation

### Bash installer

```bash
curl -fsSL https://argus.0x-ai.com | sh
```

The installer detects your OS/architecture, downloads the matching release
tarball from GitHub Releases, verifies its SHA-256 checksum, and installs
`argus`, `argusd`, and the systemd unit.

Install a specific version:

```bash
ARGUS_VERSION=0.1.0 curl -fsSL https://argus.0x-ai.com | sh
```

### Cargo (crates.io)

```bash
cargo install argus-cli argus-daemon
```

This installs the `argus` CLI and the `argusd` daemon from crates.io.

### APT (Debian/Ubuntu)

```bash
curl -fsSL https://argus.0x-ai.com/apt/argus-archive-keyring.gpg \
  | sudo gpg --dearmor -o /usr/share/keyrings/argus-archive-keyring.gpg

echo "deb [signed-by=/usr/share/keyrings/argus-archive-keyring.gpg] https://argus.0x-ai.com/apt stable main" \
  | sudo tee /etc/apt/sources.list.d/argus.list

sudo apt-get update
sudo apt-get install -y argus argusd
```

### Build from source

```bash
git clone https://github.com/alvaropaco/argus-ai.git
cd argus-ai
cargo build --release
# binaries in target/release/{argus,argusd}
```

## Usage

### Start the daemon

```bash
# as root (or via the systemd unit)
argusd

# or with explicit paths
argusd --socket /run/argus/argusd.sock --state /var/lib/argus/argus.db
```

Enable and start it as a service:

```bash
sudo systemctl enable --now argusd
```

The packaged `argusd.service` is hardened (no capabilities, read-only root,
private temp, restricted address families).

### Query the daemon

```bash
argus --version
argus health          # runtime health
argus status          # environment id, uptime, plugin count
argus capabilities    # registered capabilities
argus plugins         # loaded plugins
argus config          # non-secret configuration
```

### Interactive TUI

```bash
argus            # status dashboard
argus init       # interactive setup (writes argus.toml)
```

### Self-update

```bash
argus upgrade     # verifies artifacts before install (bootstrap skeleton)
```

## Configuration

`argusd` flags:

| Flag | Default | Description |
|------|---------|-------------|
| `--socket` | `/run/argus/argusd.sock` | Unix socket path |
| `--state` | `/var/lib/argus/argus.db` | SQLite state path |
| `--environment` | `default` | Environment name |
| `--authorized-uids` | *(all)* | Comma-separated peer UID allowlist |
| `--otel-endpoint` | *(none)* | OpenTelemetry OTLP endpoint |
| `--log-format` | `text` | `text` or `json` |

Log filtering uses `RUST_LOG` (default `argus=info`).

## Security model

- **Trust boundary:** the AI/planning layer is unprivileged; `argusd` is the
  only component that crosses into privileged execution.
- **Policy before execution:** bootstrap capabilities are read-only; unknown and
  privileged operations are denied by default.
- **Typed capabilities** (`host.status.read`, `argus.health.read`, …), never
  arbitrary shell strings.
- **Least privilege:** the daemon runs with an empty capability set and hardened
  systemd sandboxing.

## Development

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The project is developed with a spec-driven workflow (SpecKit): features are
specified under `specs/`, planned, and implemented against the constitution in
`.specify/memory/constitution.md`.

## Documentation

- [Constitution](.specify/memory/constitution.md)
- [Architecture (C4)](docs/architecture/c4-solution-architecture.md)
- [Domain model](docs/architecture/domain-model.md)
- [ADRs](docs/adr/)

## License

[Apache-2.0](LICENSE)
