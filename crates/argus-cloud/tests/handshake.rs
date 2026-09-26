//! Handshake variant tests (T041).
//!
//! Covers the sequence in `contracts/agent-protocol-conformance.md` §2 through
//! the crate's public API: the welcome, the authenticate frame, the ready
//! reply, every rejection classification, protocol negotiation, and cloud
//! identity verification.

use argus_cloud::client::handshake::{HandshakeFailure, InstallationIdentity, authenticate};
use argus_cloud::client::identity::InstallationKey;
use argus_cloud::protocol::messages::PairingGrantedPayload;
use argus_cloud::protocol::{Envelope, MessageType};
use argus_cloud::state::EnrolledIdentity;
use argus_cloud::transport::fake::FakeTransport;
use chrono::{TimeZone, Utc};
use serde_json::json;
use uuid::Uuid;

const CLOUD_ID: &str = "argus-cloud";
const SESSION_PROOF: &str = "credential-issued-at-enrollment";

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 21, 12, 0, 0).unwrap()
}

fn key() -> InstallationKey {
    InstallationKey::from_seed_bytes(&[7u8; 32])
}

fn identity() -> EnrolledIdentity {
    EnrolledIdentity::from_granted(
        &PairingGrantedPayload {
            instance_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            instance_name: "web-01".into(),
            session_token: SESSION_PROOF.into(),
            session_expires_at: now() + chrono::Duration::hours(1),
            negotiated_protocol_version: "1.0.0".into(),
        },
        now(),
        key().public_key_b64(),
    )
}

fn hello(cloud_id: &str, versions: &[&str], heartbeat: u64) -> Envelope {
    Envelope::new(
        MessageType::HandshakeHello,
        json!({
            "cloud_id": cloud_id,
            "cloud_instance": "gateway-7",
            "server_time": now().to_rfc3339(),
            "supported_protocol_versions": versions,
            "challenge": "nonce",
            "heartbeat_interval_seconds": heartbeat
        }),
        None,
    )
}

fn ready() -> Envelope {
    Envelope::new(
        MessageType::HandshakeReady,
        json!({
            "negotiated_protocol_version": "1.0.0",
            "instance_id": Uuid::new_v4().to_string(),
            "tenant_id": Uuid::new_v4().to_string(),
            "config_pull_required": true
        }),
        None,
    )
}

fn reject(code: &str) -> Envelope {
    Envelope::new(
        MessageType::HandshakeReject,
        json!({ "code": code, "message": "refused" }),
        None,
    )
}

async fn attempt(
    frames: Vec<Envelope>,
) -> (
    Result<argus_cloud::client::handshake::AuthenticatedSession, HandshakeFailure>,
    FakeTransport,
) {
    let mut transport = FakeTransport::with_inbound(frames);
    let identity = identity();
    let key = key();
    let outcome = authenticate(
        &mut transport,
        CLOUD_ID,
        InstallationIdentity {
            enrolled: &identity,
            key: &key,
        },
        Some(SESSION_PROOF),
        "web-01",
        "0.1.7",
        Some("0.1.0"),
    )
    .await;
    (outcome, transport)
}

#[tokio::test]
async fn the_sequence_hello_authenticate_ready_yields_a_session() {
    let (outcome, transport) = attempt(vec![hello(CLOUD_ID, &["1.0.0"], 20), ready()]).await;

    let session = outcome.expect("authenticated");
    assert_eq!(session.negotiated_protocol_version, "1.0.0");
    assert_eq!(session.heartbeat_interval_seconds, 20);
    assert!(session.config_pull_required);

    let sent = transport.sent_of_type(MessageType::HandshakeAuthenticate);
    assert_eq!(sent.len(), 1, "exactly one authenticate frame");
}

#[tokio::test]
async fn the_authenticate_frame_carries_the_session_proof_and_identity() {
    let (_, transport) = attempt(vec![hello(CLOUD_ID, &["1.0.0"], 20), ready()]).await;
    let sent = transport.sent();
    let payload = &sent[0].payload;

    assert_eq!(payload["session_proof"], SESSION_PROOF);
    for field in [
        "instance_id",
        "protocol_version",
        "agent_version",
        "hostname",
        "challenge_signature",
    ] {
        assert!(payload.get(field).is_some(), "cloud requires `{field}`");
    }
}

#[tokio::test]
async fn the_negotiated_heartbeat_interval_is_adopted() {
    let (outcome, _) = attempt(vec![hello(CLOUD_ID, &["1.0.0"], 45), ready()]).await;
    assert_eq!(
        outcome.expect("authenticated").heartbeat_interval_seconds,
        45
    );
}

#[tokio::test]
async fn a_revocation_is_terminal() {
    let (outcome, _) = attempt(vec![hello(CLOUD_ID, &["1.0.0"], 20), reject("REVOKED")]).await;
    let failure = outcome.expect_err("rejected");

    match &failure {
        HandshakeFailure::Rejected { code, .. } => assert_eq!(code, "REVOKED"),
        other => panic!("expected Rejected, got {other:?}"),
    }
    assert!(failure.is_terminal());
    assert!(!failure.allows_retry());
}

#[tokio::test]
async fn a_suspension_stays_retryable() {
    let (outcome, _) = attempt(vec![hello(CLOUD_ID, &["1.0.0"], 20), reject("SUSPENDED")]).await;
    let failure = outcome.expect_err("rejected");
    assert!(
        !failure.is_terminal(),
        "suspension must never strand an installation"
    );
    assert!(failure.allows_retry());
}

#[tokio::test]
async fn rate_limiting_is_identifiable_and_retryable() {
    let (outcome, _) = attempt(vec![
        hello(CLOUD_ID, &["1.0.0"], 20),
        reject("RATE_LIMITED"),
    ])
    .await;
    let failure = outcome.expect_err("rejected");
    assert!(failure.is_rate_limited());
    assert!(failure.allows_retry());
    assert!(!failure.is_terminal());
}

#[tokio::test]
async fn an_unsupported_version_tells_the_operator_to_upgrade() {
    let (outcome, _) = attempt(vec![
        hello(CLOUD_ID, &["1.0.0"], 20),
        reject("UNSUPPORTED_VERSION"),
    ])
    .await;
    let remediation = outcome.expect_err("rejected").remediation();
    assert!(
        remediation.to_lowercase().contains("upgrade"),
        "{remediation}"
    );
}

#[tokio::test]
async fn no_shared_protocol_version_fails_before_authenticating() {
    let (outcome, transport) = attempt(vec![hello(CLOUD_ID, &["9.0.0"], 20), ready()]).await;

    assert!(matches!(
        outcome.expect_err("no shared version"),
        HandshakeFailure::UnsupportedVersion { .. }
    ));
    assert!(
        transport.sent().is_empty(),
        "must not authenticate when no version is shared"
    );
}

#[tokio::test]
async fn an_unexpected_cloud_is_refused() {
    let (outcome, transport) = attempt(vec![hello("someone-else", &["1.0.0"], 20), ready()]).await;

    match outcome.expect_err("wrong peer") {
        HandshakeFailure::CloudIdentityMismatch { expected, actual } => {
            assert_eq!(expected, CLOUD_ID);
            assert_eq!(actual, "someone-else");
        }
        other => panic!("expected CloudIdentityMismatch, got {other:?}"),
    }
    assert!(
        transport.sent().is_empty(),
        "must not send the credential to an unexpected peer"
    );
}

#[tokio::test]
async fn a_closed_stream_is_reported_as_the_cloud_closing() {
    let (outcome, _) = attempt(vec![]).await;
    assert_eq!(outcome.expect_err("closed"), HandshakeFailure::CloudClosed);
}

#[tokio::test]
async fn an_unexpected_message_during_the_handshake_is_reported() {
    let ping = Envelope::new(MessageType::Ping, json!({}), None);
    let (outcome, _) = attempt(vec![ping]).await;
    assert!(matches!(
        outcome.expect_err("unexpected"),
        HandshakeFailure::Unexpected { .. }
    ));
}

#[tokio::test]
async fn an_unknown_rejection_code_is_surfaced_not_guessed() {
    let (outcome, _) = attempt(vec![
        hello(CLOUD_ID, &["1.0.0"], 20),
        reject("SOMETHING_NEW"),
    ])
    .await;
    match outcome.expect_err("rejected") {
        HandshakeFailure::Rejected { code, .. } => assert_eq!(code, "SOMETHING_NEW"),
        other => panic!("expected Rejected, got {other:?}"),
    }
}
