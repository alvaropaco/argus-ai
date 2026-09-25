//! Secret-handling tests for enrollment (T032).
//!
//! The pairing code and the session credential are secrets. The cloud contract
//! states pairing codes are never logged, and FR-051 requires the credential to
//! stay in the secret store. These tests assert the two ways a secret could leak
//! from this crate: through a debug format, and through a frame that should not
//! carry it.

use argus_cloud::client::identity::InstallationKey;
use argus_cloud::client::pairing::{EnrollmentOutcome, enroll};
use argus_cloud::protocol::messages::{
    HandshakeAuthenticatePayload, PairingGrantedPayload, PairingRedeemPayload, SessionRotatePayload,
};
use argus_cloud::protocol::{Envelope, MessageType};
use argus_cloud::transport::fake::FakeTransport;
use chrono::{TimeZone, Utc};
use serde_json::json;
use uuid::Uuid;

const CODE: &str = "ARGUS-SECRET-CODE-0001";
const SESSION_TOKEN: &str = "super-secret-session-credential";
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
            "session_token": SESSION_TOKEN,
            "session_expires_at": (now() + chrono::Duration::hours(1)).to_rfc3339(),
            "negotiated_protocol_version": "1.0.0"
        }),
        None,
    )
}

#[test]
fn the_pairing_code_is_never_rendered_by_debug() {
    let key = key();
    let payload = PairingRedeemPayload {
        code: CODE.to_string(),
        protocol_version: "1.0.0".into(),
        agent_version: "0.1.7".into(),
        hostname: "web-01".into(),
        public_key: key.public_key_b64(),
        challenge_signature: key.sign_challenge("nonce"),
        capability_schema_version: None,
    };
    let rendered = format!("{payload:?}");
    assert!(!rendered.contains(CODE), "pairing code leaked: {rendered}");
    assert!(rendered.contains("<redacted>"), "redaction marker missing");
    assert!(
        rendered.contains(&key.public_key_b64()),
        "the public key is public by definition (ADR-0024) and stays visible: {rendered}"
    );
}

#[test]
fn the_session_credential_is_never_rendered_by_debug() {
    let payload = PairingGrantedPayload {
        instance_id: Uuid::new_v4(),
        tenant_id: Uuid::new_v4(),
        instance_name: "web-01".into(),
        session_token: SESSION_TOKEN.into(),
        session_expires_at: now(),
        negotiated_protocol_version: "1.0.0".into(),
    };
    let rendered = format!("{payload:?}");
    assert!(
        !rendered.contains(SESSION_TOKEN),
        "session credential leaked: {rendered}"
    );
    assert!(rendered.contains("<redacted>"));
}

#[test]
fn the_session_proof_is_never_rendered_by_debug() {
    let payload = HandshakeAuthenticatePayload {
        instance_id: Uuid::new_v4(),
        protocol_version: "1.0.0".into(),
        agent_version: "0.1.7".into(),
        hostname: "web-01".into(),
        challenge_signature: key().sign_challenge("nonce"),
        session_proof: SESSION_TOKEN.into(),
        capability_schema_version: None,
    };
    let rendered = format!("{payload:?}");
    assert!(!rendered.contains(SESSION_TOKEN), "session proof leaked");
}

#[test]
fn the_rotation_token_is_never_rendered_by_debug() {
    let payload = SessionRotatePayload {
        rotation_token: "super-secret-rotation-token".into(),
        expires_at: now(),
    };
    let rendered = format!("{payload:?}");
    assert!(!rendered.contains("super-secret-rotation-token"));
    assert!(rendered.contains("<redacted>"));
}

#[tokio::test]
async fn a_successful_enrollment_outcome_does_not_render_the_credential() {
    let mut transport = FakeTransport::with_inbound(vec![hello(), granted()]);
    let outcome = enroll(&mut transport, CODE, "web-01", "0.1.7", CLOUD_ID, &key())
        .await
        .expect("enrolled");

    let rendered = format!("{outcome:?}");
    assert!(
        !rendered.contains(SESSION_TOKEN),
        "the outcome must not leak the credential when logged: {rendered}"
    );
    assert!(matches!(outcome, EnrollmentOutcome::Granted(_)));
}

#[tokio::test]
async fn the_code_appears_only_in_the_redeem_frame() {
    let mut transport = FakeTransport::with_inbound(vec![hello(), granted()]);
    enroll(&mut transport, CODE, "web-01", "0.1.7", CLOUD_ID, &key())
        .await
        .expect("enrolled");

    let sent = transport.sent();
    let mut frames_containing_the_code = Vec::new();
    for frame in &sent {
        if serde_json::to_string(&frame.payload)
            .unwrap_or_default()
            .contains(CODE)
        {
            frames_containing_the_code.push(frame.kind());
        }
    }

    assert_eq!(
        frames_containing_the_code,
        vec![Some(MessageType::PairingRedeem)],
        "the code belongs only in pairing.redeem"
    );
}

#[tokio::test]
async fn the_code_is_not_echoed_back_in_a_denial() {
    let denied = Envelope::new(MessageType::PairingDenied, json!({ "code": "USED" }), None);
    let mut transport = FakeTransport::with_inbound(vec![hello(), denied]);
    let outcome = enroll(&mut transport, CODE, "web-01", "0.1.7", CLOUD_ID, &key())
        .await
        .expect("a denial is not an error");

    let rendered = format!("{outcome:?}");
    assert!(
        !rendered.contains(CODE),
        "denial leaked the code: {rendered}"
    );
}

#[test]
fn the_protocol_error_type_does_not_carry_a_payload() {
    // Errors are the other place a secret could travel: keep them payload-free.
    let err = argus_cloud::error::CloudError::Protocol("expected pairing.granted".into());
    let rendered = format!("{err:?}");
    assert!(!rendered.contains(SESSION_TOKEN));
}
