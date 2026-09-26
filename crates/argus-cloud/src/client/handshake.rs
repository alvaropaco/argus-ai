//! Connection handshake: proving identity and agreeing terms.
//!
//! The sequence is fixed by `contracts/agent-protocol-conformance.md` §2: the
//! cloud welcomes with `handshake.hello`, the installation answers with
//! `handshake.authenticate`, and the cloud responds with `handshake.ready` or
//! `handshake.reject`.
//!
//! Rejections are classified rather than collapsed into one failure, because the
//! correct reaction differs: revocation is permanent, rate limiting means back
//! off, and a version mismatch means stop and tell the operator.

use uuid::Uuid;

use crate::client::identity::InstallationKey;
use crate::error::CloudError;
use crate::protocol::envelope::{Envelope, MessageType};
use crate::protocol::messages::{
    HandshakeAuthenticatePayload, HandshakeHelloPayload, HandshakeReadyPayload,
    HandshakeRejectPayload,
};
use crate::protocol::version::{PROTOCOL_VERSION, negotiate};
use crate::state::EnrolledIdentity;
use crate::transport::websocket::WssTransport;
use crate::transport::{Transport, TransportError};

/// An authenticated session, agreed terms included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedSession {
    pub negotiated_protocol_version: String,
    pub instance_id: Uuid,
    pub tenant_id: Uuid,
    pub heartbeat_interval_seconds: u64,
    /// Whether the cloud wants the applied configuration reported back.
    pub config_pull_required: bool,
}

/// Why an authentication attempt produced no session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandshakeFailure {
    /// The cloud refused the session, with its own code.
    Rejected { code: String, message: String },
    /// The peer was not the cloud we expected to talk to.
    CloudIdentityMismatch { expected: String, actual: String },
    /// The cloud closed the connection before answering.
    CloudClosed,
    /// The cloud sent a message the handshake does not accept.
    Unexpected { received: String },
    /// No mutually supported protocol version.
    UnsupportedVersion { offered: Vec<String> },
    /// The reply could not be decoded.
    Malformed(String),
}

impl HandshakeFailure {
    /// Revocation is the only outcome that must stop reconnection permanently.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Rejected { code, .. } if code == "REVOKED")
    }

    /// Whether the cloud asked us to slow down rather than try again at once.
    pub fn is_rate_limited(&self) -> bool {
        matches!(self, Self::Rejected { code, .. } if code == "RATE_LIMITED")
    }

    /// Whether the handshake should be retried at all.
    pub fn allows_retry(&self) -> bool {
        !self.is_terminal()
    }

    /// A plain-language next step for the operator.
    pub fn remediation(&self) -> String {
        match self {
            Self::Rejected { code, message } if code == "REVOKED" => {
                "this installation was revoked; re-enrollment is required".to_string()
            }
            Self::Rejected { code, .. } if code == "RATE_LIMITED" => {
                "the cloud is rate limiting this installation; it will retry more slowly"
                    .to_string()
            }
            Self::Rejected { code, .. } if code == "UNSUPPORTED_VERSION" => {
                "this installation and the cloud share no protocol version; upgrade is required"
                    .to_string()
            }
            Self::Rejected { code, message } => {
                format!("the cloud refused the session ({code}): {message}")
            }
            Self::CloudIdentityMismatch { expected, actual } => {
                format!("refused to talk to '{actual}'; expected '{expected}'")
            }
            Self::CloudClosed => "the cloud closed the connection during the handshake".to_string(),
            Self::Unexpected { received } => {
                format!("the cloud sent an unexpected message during the handshake ({received})")
            }
            Self::UnsupportedVersion { .. } => {
                "no mutually supported protocol version; upgrade is required".to_string()
            }
            Self::Malformed(detail) => {
                format!("the cloud sent a malformed handshake reply: {detail}")
            }
        }
    }
}

/// The installation's cryptographic identity: the enrolled metadata plus the
/// signing key that proves possession of it (ADR-0024).
pub struct InstallationIdentity<'a> {
    pub enrolled: &'a EnrolledIdentity,
    pub key: &'a InstallationKey,
}

/// Authenticates an already-enrolled installation.
///
/// `session_proof` is the credential issued at enrollment, when the installation
/// still holds one. It is no longer required: identity is the Ed25519 key that
/// signs the challenge (ADR-0026).
pub async fn authenticate(
    transport: &mut dyn Transport,
    expected_cloud_id: &str,
    installation: InstallationIdentity<'_>,
    session_proof: Option<&str>,
    hostname: &str,
    agent_version: &str,
    capability_schema_version: Option<&str>,
) -> Result<AuthenticatedSession, HandshakeFailure> {
    let hello = await_hello(transport, expected_cloud_id).await?;

    let Some(_negotiated) = negotiate(&hello.supported_protocol_versions) else {
        return Err(HandshakeFailure::UnsupportedVersion {
            offered: hello.supported_protocol_versions.clone(),
        });
    };

    let authenticate = HandshakeAuthenticatePayload {
        instance_id: installation.enrolled.installation_id,
        protocol_version: PROTOCOL_VERSION.to_string(),
        agent_version: agent_version.to_string(),
        hostname: hostname.to_string(),
        challenge_signature: installation.key.sign_challenge(&hello.challenge),
        session_proof: session_proof.map(str::to_string),
        capability_schema_version: capability_schema_version.map(str::to_string),
    };
    send_json(transport, MessageType::HandshakeAuthenticate, &authenticate).await?;

    let reply = transport
        .recv()
        .await
        .map_err(classify_transport)?
        .ok_or(HandshakeFailure::CloudClosed)?;

    match reply.kind() {
        Some(MessageType::HandshakeReady) => {
            let ready: HandshakeReadyPayload = serde_json::from_value(reply.payload.clone())
                .map_err(|e| HandshakeFailure::Malformed(format!("handshake.ready: {e}")))?;
            Ok(AuthenticatedSession {
                negotiated_protocol_version: ready.negotiated_protocol_version,
                instance_id: ready.instance_id,
                tenant_id: ready.tenant_id,
                heartbeat_interval_seconds: hello.heartbeat_interval_seconds,
                config_pull_required: ready.config_pull_required,
            })
        }
        Some(MessageType::HandshakeReject) => {
            let rejected: HandshakeRejectPayload = serde_json::from_value(reply.payload.clone())
                .map_err(|e| HandshakeFailure::Malformed(format!("handshake.reject: {e}")))?;
            Err(HandshakeFailure::Rejected {
                code: rejected.code,
                message: rejected.message,
            })
        }
        _ => Err(HandshakeFailure::Unexpected {
            received: reply.message_type,
        }),
    }
}

async fn await_hello(
    transport: &mut dyn Transport,
    expected_cloud_id: &str,
) -> Result<HandshakeHelloPayload, HandshakeFailure> {
    let frame = transport
        .recv()
        .await
        .map_err(classify_transport)?
        .ok_or(HandshakeFailure::CloudClosed)?;

    if frame.kind() != Some(MessageType::HandshakeHello) {
        return Err(HandshakeFailure::Unexpected {
            received: frame.message_type,
        });
    }

    let hello: HandshakeHelloPayload = serde_json::from_value(frame.payload.clone())
        .map_err(|e| HandshakeFailure::Malformed(format!("handshake.hello: {e}")))?;

    WssTransport::verify_cloud_identity(expected_cloud_id, Some(hello.cloud_id.as_str())).map_err(
        |error| match error {
            TransportError::CloudIdentityMismatch { expected, actual } => {
                HandshakeFailure::CloudIdentityMismatch { expected, actual }
            }
            other => HandshakeFailure::Malformed(other.to_string()),
        },
    )?;

    Ok(hello)
}

fn classify_transport(error: TransportError) -> HandshakeFailure {
    match error {
        TransportError::CloudIdentityMismatch { expected, actual } => {
            HandshakeFailure::CloudIdentityMismatch { expected, actual }
        }
        other => HandshakeFailure::Malformed(other.to_string()),
    }
}

async fn send_json<T: serde::Serialize>(
    transport: &dyn Transport,
    ty: MessageType,
    payload: &T,
) -> Result<(), HandshakeFailure> {
    let payload = serde_json::to_value(payload)
        .map_err(|e| HandshakeFailure::Malformed(format!("cannot encode authenticate: {e}")))?;
    transport
        .send(&Envelope::new(ty, payload, None))
        .await
        .map_err(classify_transport)
}

impl From<CloudError> for HandshakeFailure {
    fn from(error: CloudError) -> Self {
        HandshakeFailure::Malformed(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_revocation_is_terminal() {
        let revoked = HandshakeFailure::Rejected {
            code: "REVOKED".into(),
            message: "instance revoked".into(),
        };
        assert!(revoked.is_terminal());
        assert!(!revoked.allows_retry());

        for code in ["UNAUTHENTICATED", "RATE_LIMITED", "INTERNAL", "MALFORMED"] {
            let failure = HandshakeFailure::Rejected {
                code: code.into(),
                message: String::new(),
            };
            assert!(!failure.is_terminal(), "{code} must remain retryable");
            assert!(failure.allows_retry(), "{code}");
        }
    }

    #[test]
    fn a_suspension_is_retryable_not_terminal() {
        // The cloud signals suspension through handshake.reject; treating it as
        // terminal would strand a correctly-behaving installation.
        let suspended = HandshakeFailure::Rejected {
            code: "SUSPENDED".into(),
            message: "instance suspended".into(),
        };
        assert!(!suspended.is_terminal());
        assert!(suspended.allows_retry());
    }

    #[test]
    fn rate_limiting_is_identifiable_so_the_supervisor_can_back_off() {
        let limited = HandshakeFailure::Rejected {
            code: "RATE_LIMITED".into(),
            message: String::new(),
        };
        assert!(limited.is_rate_limited());
        assert!(!limited.is_terminal());
    }

    #[test]
    fn every_failure_gives_the_operator_something_actionable() {
        let failures = [
            HandshakeFailure::Rejected {
                code: "REVOKED".into(),
                message: "revoked".into(),
            },
            HandshakeFailure::Rejected {
                code: "UNSUPPORTED_VERSION".into(),
                message: "old".into(),
            },
            HandshakeFailure::CloudIdentityMismatch {
                expected: "argus-cloud".into(),
                actual: "someone-else".into(),
            },
            HandshakeFailure::CloudClosed,
            HandshakeFailure::Unexpected {
                received: "ping".into(),
            },
            HandshakeFailure::UnsupportedVersion {
                offered: vec!["9.0.0".into()],
            },
            HandshakeFailure::Malformed("bad json".into()),
        ];
        for failure in failures {
            let text = failure.remediation();
            assert!(!text.is_empty(), "{failure:?}");
            assert!(
                !text.to_lowercase().contains("secret"),
                "{failure:?} must not mention secrets"
            );
        }
    }
}
