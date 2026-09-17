//! Installation and update primitives: checksum/signature verification (ADR-017).
//!
//! The installer is part of the security boundary: artifacts must be verifiable
//! and the process must not silently execute untrusted code.

mod artifact;

pub use artifact::{ArtifactVerifier, ChecksumVerifier, ReleaseArtifact, VerificationOutcome};
