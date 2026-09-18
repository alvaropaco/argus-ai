//! Release artifact metadata and verification.

use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A released ARGUS artifact with integrity metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseArtifact {
    pub name: String,
    pub version: Version,
    /// Target triple, e.g. `x86_64-unknown-linux-gnu`.
    pub platform: String,
    /// Lowercase hex SHA-256 checksum.
    pub sha256: String,
    /// Detached signature (Sigstore keyless is the longer-term target).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

/// The outcome of verifying an artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerificationOutcome {
    Verified,
    Rejected(String),
}

impl VerificationOutcome {
    pub fn is_verified(&self) -> bool {
        matches!(self, VerificationOutcome::Verified)
    }
}

/// Verifies a release artifact against its integrity metadata (ADR-017).
pub trait ArtifactVerifier {
    fn verify(&self, artifact: &ReleaseArtifact, contents: &[u8]) -> VerificationOutcome;
}

/// SHA-256 checksum verifier — the bootstrap baseline.
///
/// Sigstore/cosign keyless signing is the longer-term target (research.md § 5);
/// this verifier covers checksum integrity today.
pub struct ChecksumVerifier;

impl ArtifactVerifier for ChecksumVerifier {
    fn verify(&self, artifact: &ReleaseArtifact, contents: &[u8]) -> VerificationOutcome {
        let digest = Sha256::digest(contents);
        let actual: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        if actual == artifact.sha256 {
            VerificationOutcome::Verified
        } else {
            VerificationOutcome::Rejected(format!(
                "checksum mismatch: expected {}, got {}",
                artifact.sha256, actual
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact(sha256: &str) -> ReleaseArtifact {
        ReleaseArtifact {
            name: "argus".into(),
            version: Version::new(0, 1, 0),
            platform: "x86_64-unknown-linux-gnu".into(),
            sha256: sha256.into(),
            signature: None,
        }
    }

    #[test]
    fn verifies_matching_checksum() {
        let contents = b"hello argus";
        let expected: String = Sha256::digest(contents)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();

        let verifier = ChecksumVerifier;
        assert_eq!(
            verifier.verify(&artifact(&expected), contents),
            VerificationOutcome::Verified
        );
    }

    #[test]
    fn rejects_mismatched_checksum() {
        let verifier = ChecksumVerifier;
        let outcome = verifier.verify(&artifact(&"0".repeat(64)), b"hello argus");
        assert!(matches!(outcome, VerificationOutcome::Rejected(_)));
    }
}
