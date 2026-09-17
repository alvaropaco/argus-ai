# ADR-017: Installation and Updates

- **Status:** Accepted
- **Date:** 2026-09-16

## Decision

The primary ARGUS installation experience will be:

```bash
curl -fsSL https://argus.0x-ai.com | sh
```

The installer will bootstrap the ARGUS runtime and establish the required system integration for a Linux/Unix installation.

ARGUS releases will be distributed through **GitHub Releases** and protected with cryptographic signatures and/or checksums. ARGUS will provide an `argus upgrade` command for controlled self-updates.

The update mechanism must verify artifact authenticity/integrity before installation and must support safe rollback or recovery from a failed update where practical.

## Goals

- Minimal installation friction.
- Reproducible release artifacts.
- Cryptographic verification.
- Explicit versioning.
- Safe upgrades.
- Compatibility with unattended/server environments.

## Principle

The ARGUS installer is part of the security boundary: installation and update artifacts must be verifiable and the process must not silently execute untrusted code.