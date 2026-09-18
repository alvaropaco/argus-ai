# ARGUS Release & Installer

Installation and updates are part of the ARGUS security boundary (ADR-017):
artifacts are cryptographically verifiable and the process never silently
executes untrusted code.

## Release artifact layout (GitHub Releases)

Each release (`vX.Y.Z`) publishes, per target triple:

```text
argus-<version>-<target>.tar.gz       # binaries (argus, argusd) + MCP Core
argus-<version>-<target>.tar.gz.sha256 # lowercase-hex SHA-256 checksum
argus-<version>-<target>.tar.gz.sig   # detached signature (Sigstore keyless target)
```

- **Targets:** Linux (`x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`).
- **Contents:** `/usr/bin/argus`, `/usr/bin/argusd`, the bundled MCP Core under
  `mcp/core/`, and the systemd unit (`argusd.service`).
- **Integrity:** SHA-256 checksums are the bootstrap baseline; Sigstore/cosign
  keyless signing is the target (research.md § 5). The `argus-install` crate
  provides the verification interface (`ArtifactVerifier`, `ChecksumVerifier`).

## Installer

The primary install path (ADR-017) is:

```bash
curl -fsSL https://argus.0x-ai.com | sh
```

The script downloads the matching release artifact, verifies its checksum
before extraction, and installs `argus`/`argusd` plus the systemd unit.
`argus upgrade` is the in-band self-update entry point (bootstrap skeleton).

## Packaging

- `deploy/debian/argusd.service` — hardened systemd unit.
- `deploy/apt/` — Debian repository metadata and signing (see
  `deploy/apt/README.md`).

## Cargo installs

`cargo install --path crates/argus-daemon` puts `argusd` in `~/.cargo/bin`, which
the hardened unit cannot execute (it masks `/home` via `ProtectHome=true`). To
keep systemd working, either:

- install into the packaged location so the unit needs no changes:

  ```bash
  cargo install --root /usr --path crates/argus-ai-cli
  cargo install --root /usr --path crates/argus-daemon
  ```

- or override `ExecStart` with a systemd drop-in
  (`systemctl edit argusd` creates `/etc/systemd/system/argusd.service.d/override.conf`):

  ```ini
  [Service]
  ExecStart=
  ExecStart=/home/user/.cargo/bin/argusd
  ```

  Note: with a binary under `/home`, you must also relax `ProtectHome` in the
  drop-in (`ProtectHome=read-only` or `false`).
