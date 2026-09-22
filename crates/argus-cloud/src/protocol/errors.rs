//! Protocol error and pairing-denial vocabularies.
//!
//! These are the cloud's codes verbatim. The installation MUST handle each of
//! them distinctly — a rate limit is not a revocation, and an unsupported
//! version is not a transient failure.

use serde::{Deserialize, Serialize};

/// Protocol-level error codes returned by the cloud.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProtocolErrorCode {
    /// Invalid JSON or envelope.
    Malformed,
    /// The offered protocol version is not supported.
    UnsupportedVersion,
    /// The message type is not registered.
    UnknownType,
    /// Credential missing or invalid.
    Unauthenticated,
    /// The instance is revoked or suspended.
    Revoked,
    /// Too many attempts.
    RateLimited,
    /// Payload exceeds the permitted bound.
    PayloadTooLarge,
    /// Unexpected cloud failure.
    Internal,
}

impl ProtocolErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Malformed => "MALFORMED",
            Self::UnsupportedVersion => "UNSUPPORTED_VERSION",
            Self::UnknownType => "UNKNOWN_TYPE",
            Self::Unauthenticated => "UNAUTHENTICATED",
            Self::Revoked => "REVOKED",
            Self::RateLimited => "RATE_LIMITED",
            Self::PayloadTooLarge => "PAYLOAD_TOO_LARGE",
            Self::Internal => "INTERNAL",
        }
    }
}

/// Reasons a pairing attempt can be refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PairingDenialCode {
    /// Unknown code. Compared in constant time cloud-side; the message is generic.
    Invalid,
    /// The code has already been redeemed.
    Used,
    /// The code's time-to-live elapsed.
    Expired,
    /// The code was cancelled by the operator.
    Cancelled,
    /// The presented public key differs from the enrolled one.
    KeyChanged,
    /// Too many attempts; the installation must back off.
    RateLimited,
}

impl std::fmt::Display for PairingDenialCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl PairingDenialCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Invalid => "INVALID",
            Self::Used => "USED",
            Self::Expired => "EXPIRED",
            Self::Cancelled => "CANCELLED",
            Self::KeyChanged => "KEY_CHANGED",
            Self::RateLimited => "RATE_LIMITED",
        }
    }

    /// A short, actionable, secret-free instruction for the operator.
    ///
    /// `RATE_LIMITED` is deliberately phrased as "wait" rather than "retry now",
    /// because retrying at full rate is what triggered it.
    pub fn remediation(self) -> &'static str {
        match self {
            Self::Invalid => "The pairing code is not valid. Check for typos and try again.",
            Self::Used => "This pairing code has already been used. Generate a new one.",
            Self::Expired => "This pairing code has expired. Generate a new one.",
            Self::Cancelled => "This pairing code was cancelled. Generate a new one.",
            Self::KeyChanged => {
                "This installation is already enrolled with different credentials. \
                 Revoke it in the workspace, or forget the local enrollment first."
            }
            Self::RateLimited => {
                "Too many pairing attempts. Wait before retrying; repeated attempts \
                 will extend the lockout."
            }
        }
    }
}

/// An error body carried by the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolError {
    pub code: ProtocolErrorCode,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_use_screaming_snake_case() {
        assert_eq!(
            serde_json::to_string(&ProtocolErrorCode::RateLimited).unwrap(),
            "\"RATE_LIMITED\""
        );
        assert_eq!(
            serde_json::to_string(&ProtocolErrorCode::UnsupportedVersion).unwrap(),
            "\"UNSUPPORTED_VERSION\""
        );
    }

    #[test]
    fn pairing_denial_codes_use_screaming_snake_case() {
        assert_eq!(
            serde_json::to_string(&PairingDenialCode::KeyChanged).unwrap(),
            "\"KEY_CHANGED\""
        );
        assert_eq!(PairingDenialCode::RateLimited.as_str(), "RATE_LIMITED");
    }

    #[test]
    fn every_denial_code_has_remediation_and_no_secret() {
        for code in [
            PairingDenialCode::Invalid,
            PairingDenialCode::Used,
            PairingDenialCode::Expired,
            PairingDenialCode::Cancelled,
            PairingDenialCode::KeyChanged,
            PairingDenialCode::RateLimited,
        ] {
            let text = code.remediation();
            assert!(!text.is_empty(), "{code:?}");
            assert!(
                !text.to_lowercase().contains("argus-"),
                "{code:?} must not suggest a code value"
            );
        }
    }
}
