//! Enrollment: redeeming an operator-supplied pairing code.
//!
//! The flow is fixed by `contracts/pairing.md`: connect, receive
//! `handshake.hello`, redeem the code, and handle `pairing.granted` or
//! `pairing.denied`. Every denial code maps to a distinct, actionable
//! remediation so the operator knows whether to fix a typo, fetch a new code, or
//! simply wait.

use crate::client::identity::InstallationKey;
use crate::error::CloudError;
use crate::protocol::envelope::{Envelope, MessageType};
use crate::protocol::errors::PairingDenialCode;
use crate::protocol::messages::{
    HandshakeHelloPayload, MAX_AGENT_VERSION_LEN, MAX_HOSTNAME_LEN, MAX_PAIRING_CODE_LEN,
    MIN_PAIRING_CODE_LEN, PairingDeniedPayload, PairingGrantedPayload, PairingRedeemPayload,
};
use crate::protocol::version::PROTOCOL_VERSION;
use crate::transport::{Transport, TransportError};

/// The outcome of a redemption attempt.
#[derive(Clone, PartialEq)]
pub enum EnrollmentOutcome {
    Granted(Box<PairingGrantedPayload>),
    Denied {
        code: PairingDenialCode,
        remediation: &'static str,
    },
}

impl std::fmt::Debug for EnrollmentOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Granted(payload) => f
                .debug_tuple("Granted")
                .field(&format_args!("{payload:?}"))
                .finish(),
            Self::Denied { code, remediation } => f
                .debug_struct("Denied")
                .field("code", code)
                .field("remediation", remediation)
                .finish(),
        }
    }
}

/// Rejects a code that cannot possibly be valid, before spending a connection.
pub fn validate_code(code: &str) -> Result<(), CloudError> {
    let len = code.len();
    if !(MIN_PAIRING_CODE_LEN..=MAX_PAIRING_CODE_LEN).contains(&len) {
        return Err(CloudError::Protocol(format!(
            "pairing code must be {MIN_PAIRING_CODE_LEN}–{MAX_PAIRING_CODE_LEN} characters, got {len}"
        )));
    }
    if code.trim() != code || code.is_empty() {
        return Err(CloudError::Protocol(
            "pairing code must not be empty or padded with whitespace".to_string(),
        ));
    }
    Ok(())
}

/// Sends `pairing.redeem` and interprets the cloud's reply.
///
/// The installation signs the challenge the cloud sent in `handshake.hello`
/// with its Ed25519 key, and transmits the matching public key, so a production
/// cloud can verify the peer cryptographically (ADR-0024).
pub async fn enroll(
    transport: &mut dyn Transport,
    code: &str,
    hostname: &str,
    agent_version: &str,
    expected_cloud_id: &str,
    key: &InstallationKey,
) -> Result<EnrollmentOutcome, CloudError> {
    validate_code(code)?;
    validate_identity_fields(hostname, agent_version)?;

    let hello = await_hello(transport, expected_cloud_id).await?;

    let redeem = PairingRedeemPayload {
        code: code.to_string(),
        protocol_version: PROTOCOL_VERSION.to_string(),
        agent_version: agent_version.to_string(),
        hostname: hostname.to_string(),
        public_key: key.public_key_b64(),
        challenge_signature: key.sign_challenge(&hello.challenge),
        capability_schema_version: None,
    };
    send_json(transport, MessageType::PairingRedeem, &redeem).await?;

    let reply = transport
        .recv()
        .await?
        .ok_or(TransportError::Closed)
        .map_err(CloudError::from)?;

    match reply.kind() {
        Some(MessageType::PairingGranted) => {
            let granted: PairingGrantedPayload = decode(&reply)?;
            Ok(EnrollmentOutcome::Granted(Box::new(granted)))
        }
        Some(MessageType::PairingDenied) => {
            let denied: PairingDeniedPayload = decode(&reply)?;
            Ok(EnrollmentOutcome::Denied {
                code: denied.code,
                remediation: denied.code.remediation(),
            })
        }
        Some(_) => Err(CloudError::Protocol(format!(
            "expected pairing.granted or pairing.denied, received '{}'",
            reply.message_type
        ))),
        None => Err(CloudError::Protocol(format!(
            "expected pairing.granted or pairing.denied, received unknown type '{}'",
            reply.message_type
        ))),
    }
}

async fn await_hello(
    transport: &mut dyn Transport,
    expected_cloud_id: &str,
) -> Result<HandshakeHelloPayload, CloudError> {
    let frame = transport
        .recv()
        .await?
        .ok_or(TransportError::Closed)
        .map_err(CloudError::from)?;

    if frame.kind() != Some(MessageType::HandshakeHello) {
        return Err(CloudError::Protocol(format!(
            "expected handshake.hello, received '{}'",
            frame.message_type
        )));
    }

    let hello: HandshakeHelloPayload = decode(&frame)?;
    crate::transport::websocket::WssTransport::verify_cloud_identity(
        expected_cloud_id,
        Some(hello.cloud_id.as_str()),
    )?;
    Ok(hello)
}

fn validate_identity_fields(hostname: &str, agent_version: &str) -> Result<(), CloudError> {
    if hostname.is_empty() || hostname.len() > MAX_HOSTNAME_LEN {
        return Err(CloudError::Protocol(format!(
            "hostname must be 1–{MAX_HOSTNAME_LEN} characters"
        )));
    }
    if agent_version.is_empty() || agent_version.len() > MAX_AGENT_VERSION_LEN {
        return Err(CloudError::Protocol(format!(
            "agent version must be 1–{MAX_AGENT_VERSION_LEN} characters"
        )));
    }
    Ok(())
}

fn decode<T: serde::de::DeserializeOwned>(frame: &Envelope) -> Result<T, CloudError> {
    serde_json::from_value(frame.payload.clone()).map_err(|e| {
        CloudError::Protocol(format!("malformed '{}' payload: {e}", frame.message_type))
    })
}

async fn send_json<T: serde::Serialize>(
    transport: &dyn Transport,
    ty: MessageType,
    payload: &T,
) -> Result<(), CloudError> {
    let payload = serde_json::to_value(payload)
        .map_err(|e| CloudError::Protocol(format!("cannot encode '{}': {e}", ty.as_str())))?;
    transport
        .send(&Envelope::new(ty, payload, None))
        .await
        .map_err(CloudError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::fake::FakeTransport;
    use chrono::{TimeZone, Utc};
    use serde_json::json;
    use uuid::Uuid;

    const CLOUD_ID: &str = "argus-cloud";

    fn key() -> InstallationKey {
        InstallationKey::from_seed_bytes(&[7u8; 32])
    }

    fn now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 21, 12, 0, 0).unwrap()
    }

    fn hello() -> Envelope {
        Envelope::new(
            MessageType::HandshakeHello,
            json!({
                "cloud_id": CLOUD_ID,
                "cloud_instance": "gateway-7",
                "server_time": now().to_rfc3339(),
                "supported_protocol_versions": ["1.0.0"],
                "challenge": "nonce",
                "heartbeat_interval_seconds": 20
            }),
            None,
        )
    }

    fn granted() -> Envelope {
        Envelope::new(
            MessageType::PairingGranted,
            json!({
                "instance_id": Uuid::new_v4().to_string(),
                "tenant_id": Uuid::new_v4().to_string(),
                "instance_name": "web-01",
                "session_token": "session-credential-value",
                "session_expires_at": (now() + chrono::Duration::hours(1)).to_rfc3339(),
                "negotiated_protocol_version": "1.0.0"
            }),
            None,
        )
    }

    fn denied(code: &str) -> Envelope {
        Envelope::new(MessageType::PairingDenied, json!({ "code": code }), None)
    }

    async fn enroll_with(frames: Vec<Envelope>) -> Result<EnrollmentOutcome, CloudError> {
        let mut transport = FakeTransport::with_inbound(frames);
        enroll(
            &mut transport,
            "ARGUS-7F3K-9Q2M-4XZ8",
            "web-01",
            "0.1.7",
            CLOUD_ID,
            &key(),
        )
        .await
    }

    #[test]
    fn code_validation_rejects_impossible_codes() {
        assert!(validate_code("ARGUS-7F3K-9Q2M-4XZ8").is_ok());
        assert!(validate_code("short").is_err(), "below minimum");
        assert!(validate_code(&"x".repeat(MAX_PAIRING_CODE_LEN + 1)).is_err());
        assert!(validate_code(" padded ").is_err());
    }

    #[tokio::test]
    async fn a_granted_reply_yields_the_identity() {
        let outcome = enroll_with(vec![hello(), granted()])
            .await
            .expect("enrolled");
        match outcome {
            EnrollmentOutcome::Granted(payload) => {
                assert_eq!(payload.instance_name, "web-01");
                assert!(!payload.session_token.is_empty());
                assert_eq!(payload.negotiated_protocol_version, "1.0.0");
            }
            other => panic!("expected Granted, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn the_redeem_frame_carries_the_signed_challenge_and_public_key() {
        let key = key();
        let mut transport = FakeTransport::with_inbound(vec![hello(), granted()]);
        enroll(
            &mut transport,
            "ARGUS-7F3K-9Q2M-4XZ8",
            "web-01",
            "0.1.7",
            CLOUD_ID,
            &key,
        )
        .await
        .unwrap();

        let sent = transport.sent_of_type(MessageType::PairingRedeem);
        assert_eq!(sent.len(), 1);
        let payload = &sent[0].payload;
        for field in [
            "code",
            "protocol_version",
            "agent_version",
            "hostname",
            "public_key",
            "challenge_signature",
        ] {
            assert!(payload.get(field).is_some(), "cloud requires `{field}`");
        }
        assert_eq!(
            payload["public_key"].as_str().unwrap(),
            key.public_key_b64(),
            "the frame must carry the real public key"
        );
        assert_eq!(
            payload["challenge_signature"].as_str().unwrap(),
            key.sign_challenge("nonce"),
            "the frame must sign the challenge from handshake.hello"
        );
    }

    #[tokio::test]
    async fn every_denial_code_is_surfaced_distinctly() {
        for (wire, expected) in [
            ("INVALID", PairingDenialCode::Invalid),
            ("USED", PairingDenialCode::Used),
            ("EXPIRED", PairingDenialCode::Expired),
            ("CANCELLED", PairingDenialCode::Cancelled),
            ("KEY_CHANGED", PairingDenialCode::KeyChanged),
            ("RATE_LIMITED", PairingDenialCode::RateLimited),
        ] {
            let outcome = enroll_with(vec![hello(), denied(wire)])
                .await
                .unwrap_or_else(|e| panic!("{wire}: {e}"));
            match outcome {
                EnrollmentOutcome::Denied { code, remediation } => {
                    assert_eq!(code, expected, "{wire}");
                    assert!(!remediation.is_empty(), "{wire}");
                }
                other => panic!("{wire}: expected Denied, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn enrollment_refuses_an_unexpected_cloud() {
        let mut hello = hello();
        hello.payload["cloud_id"] = json!("someone-else");
        let err = enroll_with(vec![hello, granted()])
            .await
            .expect_err("must refuse an unexpected peer");
        assert!(
            matches!(
                err,
                CloudError::Transport(TransportError::CloudIdentityMismatch { .. })
            ),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn enrollment_fails_when_the_cloud_closes_first() {
        let err = enroll_with(vec![]).await.expect_err("closed stream");
        assert!(
            matches!(err, CloudError::Transport(TransportError::Closed)),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn enrollment_fails_on_an_unexpected_reply() {
        let ping = Envelope::new(MessageType::Ping, json!({}), None);
        let err = enroll_with(vec![hello(), ping])
            .await
            .expect_err("ping is not a pairing reply");
        assert!(matches!(err, CloudError::Protocol(_)), "{err:?}");
    }

    #[tokio::test]
    async fn an_invalid_code_never_reaches_the_network() {
        let mut transport = FakeTransport::new();
        let err = enroll(&mut transport, "short", "web-01", "0.1.7", CLOUD_ID, &key())
            .await
            .expect_err("rejected locally");
        assert!(matches!(err, CloudError::Protocol(_)), "{err:?}");
        assert!(transport.sent().is_empty(), "no frame may be sent");
    }
}
