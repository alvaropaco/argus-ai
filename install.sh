#!/usr/bin/env bash
#
# ARGUS installer
#
# Downloads a release tarball from GitHub Releases, verifies its SHA-256
# checksum, and installs `argus` and `argusd` (plus the systemd unit when
# running as root on a systemd host).
#
# Usage:
#   curl -fsSL https://argus.0x-ai.com | sh
#   ARGUS_VERSION=0.1.0 sh install.sh
#   sh install.sh --version 0.1.0
#
set -euo pipefail

REPO="alvaropaco/argus-ai"
GITHUB="https://github.com/${REPO}"
API="https://api.github.com/repos/${REPO}"
DEFAULT_VERSION="latest"

# Installation destinations.
BIN_DIR="/usr/local/bin"
SYSTEMD_UNIT_DIR="/etc/systemd/system"
SYSTEMD_UNIT_NAME="argusd.service"

log()  { printf '\033[1;34m[argus]\033[0m %s\n' "$*"; }
err()  { printf '\033[1;31m[argus] error:\033[0m %s\n' "$*" >&2; }

usage() {
    cat <<'EOF'
Usage: install.sh [--version VERSION] [--prefix DIR]

Options:
  --version VERSION   Install a specific version (default: latest).
  --prefix DIR        Install binaries into DIR (default: /usr/local/bin).

Environment:
  ARGUS_VERSION       Version to install (overrides --version).
EOF
}

VERSION="${ARGUS_VERSION:-$DEFAULT_VERSION}"
PREFIX="${BIN_DIR}"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --version) VERSION="${2:?--version requires a value}"; shift 2 ;;
        --prefix)  PREFIX="${2:?--prefix requires a value}"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) err "unknown argument: $1"; usage; exit 1 ;;
    esac
done

# Detect platform.
os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
    Linux) ;;
    *) err "ARGUS currently supports Linux only (detected: $os)."; exit 1 ;;
esac
case "$arch" in
    x86_64)  asset="linux-x86_64" ;;
    aarch64) asset="linux-aarch64" ;;
    *) err "unsupported architecture: $arch"; exit 1 ;;
esac

# Resolve version → release tag.
if [[ "$VERSION" == "latest" ]]; then
    log "resolving latest version"
    tag="$(curl -fsSL --proto '=https' "${API}/releases/latest" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p')"
    [[ -n "$tag" ]] || { err "could not resolve latest version"; exit 1; }
else
    tag="v${VERSION#v}"
fi
version="${tag#v}"
log "installing ARGUS $version ($asset)"

tarball="argus-${version}-${asset}.tar.gz"
url="${GITHUB}/releases/download/${tag}/${tarball}"
workdir="$(mktemp -d)"
trap 'rm -rf "$workdir"' EXIT

# Download.
log "downloading ${tarball}"
curl -fL --proto '=https' --retry 3 -o "${workdir}/${tarball}" "$url"
curl -fL --proto '=https' --retry 3 -o "${workdir}/SHA256SUMS" "${GITHUB}/releases/download/${tag}/SHA256SUMS"

# Verify checksum.
log "verifying checksum"
( cd "$workdir" && grep " ${tarball}\$" SHA256SUMS | sha256sum -c - >/dev/null ) \
    || { err "checksum verification failed"; exit 1; }

# Extract and install.
log "installing to ${PREFIX}"
mkdir -p "$PREFIX"
tar -xzf "${workdir}/${tarball}" -C "$PREFIX"

# Install the systemd unit when running as root on a systemd host.
if [[ "$(id -u)" -eq 0 ]] && command -v systemctl >/dev/null 2>&1 && [[ -f "${PREFIX}/argusd.service" ]]; then
    log "installing ${SYSTEMD_UNIT_NAME}"
    install -m 644 "${PREFIX}/argusd.service" "${SYSTEMD_UNIT_DIR}/${SYSTEMD_UNIT_NAME}"
    systemctl daemon-reload
    log "start with: systemctl enable --now ${SYSTEMD_UNIT_NAME}"
fi

log "done:"
log "  argus   -> ${PREFIX}/argus"
log "  argusd  -> ${PREFIX}/argusd"
log "  run 'argus --help' or 'argusd --help' to get started."
