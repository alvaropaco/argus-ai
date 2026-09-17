# ARGUS AI APT Repository

This directory contains the configuration used to build the official ARGUS AI APT repository.

## Target repository

The first implementation publishes the repository through the `gh-pages` branch of this repository and is intended to be served at:

`https://apt.argus.0x-ai.com`

## Supported architectures

- `amd64`
- `arm64`

## Release flow

A Git tag such as `v0.1.0` triggers `.github/workflows/publish-apt.yml`.

The workflow builds Debian packages, creates APT metadata, signs repository metadata when the signing secrets are configured, and publishes the repository to the `gh-pages` branch.

## Required GitHub secrets for signed metadata

- `APT_GPG_PRIVATE_KEY`
- `APT_GPG_PASSPHRASE`

The public key should be published at:

`apt.argus.0x-ai.com/argus-archive-keyring.gpg`

The signing key must be dedicated to the ARGUS APT repository and must not be reused for personal or unrelated release signing.

## User installation

After DNS and GitHub Pages are configured:

```bash
curl -fsSL https://apt.argus.0x-ai.com/argus-archive-keyring.gpg \
  | sudo tee /usr/share/keyrings/argus-archive-keyring.gpg >/dev/null

echo "deb [arch=amd64,arm64 signed-by=/usr/share/keyrings/argus-archive-keyring.gpg] https://apt.argus.0x-ai.com stable main" \
  | sudo tee /etc/apt/sources.list.d/argus.list

sudo apt update
sudo apt install argus
```

The public key and custom domain are operational deployment concerns and are intentionally not committed as secrets to this repository.
