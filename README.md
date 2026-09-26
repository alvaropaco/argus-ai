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
`argus`, `argusd`, and the systemd unit. When run as root on a systemd host, it
also enables and starts `argusd`. Set `ARGUS_NO_START=1` to install without
starting the daemon.

Install a specific version:

```bash
ARGUS_VERSION=0.1.7 curl -fsSL https://argus.0x-ai.com | sh
```

### Cargo (crates.io)

```bash
cargo install argus-ai-cli argus-daemon
```

This installs the `argus` CLI and the `argusd` daemon from crates.io into
`~/.cargo/bin`. To use the packaged systemd unit (which expects `/usr/bin/argusd`),
either install with `--root /usr`:

```bash
cargo install --root /usr argus-ai-cli argus-daemon
```

or override the unit's `ExecStart` with a systemd drop-in (see
[Configuration](#configuration)).

### APT (Debian/Ubuntu)

```bash
curl -fsSL https://argus.0x-ai.com/argus-archive-keyring.asc \
  | sudo gpg --dearmor -o /usr/share/keyrings/argus-archive-keyring.gpg

echo "deb [signed-by=/usr/share/keyrings/argus-archive-keyring.gpg] https://argus.0x-ai.com stable main" \
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
argus init       # interactive setup: writes argus.toml + argus.secrets.toml
```

`argus init` opens a setup wizard covering the bootstrap paths plus the AI
provider configuration (provider, model, fallback models, base URL, and API
token). See [Configuration](#configuration).

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

### `argus init` files

The setup wizard writes two files to the current directory:

- **`argus.toml`** — non-secret bootstrap and AI provider settings:

  ```toml
  socket_path = "/run/argus/argusd.sock"
  state_path = "/var/lib/argus/argus.db"
  environment_name = "default"

  [model]
  provider = "openai"
  model = "gpt-4.1"
  fallback_models = ["gpt-4.1-mini"]
  # base_url = "https://api.openai.com/v1"   # optional
  ```

- **`argus.secrets.toml`** — the provider API token, written with mode `0600`:

  ```toml
  api_token = "sk-..."
  ```

Secrets are deliberately kept out of `argus.toml` and out of `argus config`
(which redacts secret values by construction).

### Model providers

Providers are replaceable adapters behind the `ModelProvider` trait in
`argus-ai-core` ([ADR-014](docs/adr/0014-llm-provider-abstraction.md)). The
`provider` id (`openai`, `anthropic`, `ollama`, …) selects the adapter;
`model` and `fallback_models` select the primary and fallback models. The
bootstrap ships the abstraction and configuration boundary; completion,
streaming, and tool calling arrive with the AI/planning runtime.

### Argus Cloud

An installation can enroll with an Argus Cloud control plane, then report
telemetry, health, events, and activities while receiving configuration and
commands. Cloud connectivity is **optional**: with no `[cloud]` section, or with
`enabled = false`, the daemon runs entirely locally.

```bash
argus cloud enroll          # prompts; the code is not echoed
argus cloud enroll --code ARGUS-7F3K-9Q2M-4XZ8   # automation only
argus cloud status          # state, identity, backlog, remediation
argus cloud forget --yes    # remove the enrollment, keep running locally
```

Non-secret settings live in `argus.toml`:

```toml
[cloud]
enabled = false                          # opt-in; the cloud is never required
endpoint = "wss://cloud.example.com/agent"
expected_cloud_id = "argus-cloud"
allow_privileged_execution = true        # local kill switch
telemetry_interval_seconds = 60
report_buffer_max_records = 5000
```

Secrets never appear in `argus.toml`. The session credential and any
cloud-delivered provider credential live in `/etc/argus/secrets/` at mode `0600`,
and `argus config` and `argus cloud status` report *that* a credential exists,
never its value.

**Configuration precedence**, lowest to highest: built-in defaults, `argus.toml`,
cloud-managed settings, then command-line flags. Cloud-managed settings are
written to their own file (`/etc/argus/cloud-settings.toml`) so a cloud
deployment cannot overwrite sections of `argus.toml` the cloud does not own, and
so the effective source of each value is answerable. Command-line flags
deliberately outrank the cloud, so an operator always retains a local remedy.

A configuration kind this release cannot honour — such as the DevOps agent-team
definition, which has no local runtime yet — is **refused with an explicit
reason** rather than accepted and quietly ignored.

> **Installation identity.** Each installation generates an Ed25519 key pair at
> enrollment. The private seed is written to the secret store at mode `0600`
> (`/etc/argus/secrets/cloud-identity-key`) and never leaves the daemon; the
> public key is sent at enrollment, and the cloud's per-connection challenge is
> signed on every `pairing.redeem` and `handshake.authenticate`. **The key is the
> only authenticator**: the enrollment credential is optional and a lost or
> expired one never blocks reconnection. The seed cannot be recovered: if it is
> lost, run `argus cloud forget --yes` and enroll again. See
> [ADR-0024](docs/adr/0024-installation-identity-asymmetric-ed25519.md) and
> [ADR-0026](docs/adr/0026-installation-key-sole-authenticator.md).

### systemd unit

The packaged `argusd.service` runs `/usr/bin/argusd`. To run a binary installed
elsewhere, override `ExecStart` with a drop-in:

```bash
sudo systemctl edit argusd
```

```ini
[Service]
ExecStart=
ExecStart=/home/user/.cargo/bin/argusd
```

(For a binary under `/home`, also relax `ProtectHome` in the same drop-in.)
Then restart:

```bash
sudo systemctl restart argusd
```

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
