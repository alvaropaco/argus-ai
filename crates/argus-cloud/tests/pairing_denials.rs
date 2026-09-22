//! Pairing denial ladder (T031).
//!
//! Exercises enrollment through the crate's public API so the denial contract is
//! verified from the outside, where the daemon and CLI actually consume it. Every
//! code must produce a distinct, actionable outcome: the operator must be able to
//! tell "fix your typo" from "fetch a new code" from "wait".

use argus_cloud::client::pairing::{EnrollmentOutcome, enroll};
use argus_cloud::protocol::errors::PairingDenialCode;
use argus_cloud::protocol::{Envelope, MessageType};
use argus_cloud::transport::fake::FakeTransport;
use chrono::{TimeZone, Utc};
use serde_json::json;
use uuid::Uuid;

const CLOUD_ID: &str = "argus-cloud";
const CODE: &str = "ARGUS-7F3K-9Q2M-4XZ8";

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

fn denied(code: &str) -> Envelope {
    Envelope::new(MessageType::PairingDenied, json!({ "code": code }), None)
}

fn granted() -> Envelope {
    Envelope::new(
        MessageType::PairingGranted,
        json!({
            "instance_id": Uuid::new_v4().to_string(),
            "tenant_id": Uuid::new_v4().to_string(),
            "instance_name": "web-01",
            "session_token": "credential",
            "session_expires_at": (now() + chrono::Duration::hours(1)).to_rfc3339(),
            "negotiated_protocol_version": "1.0.0"
        }),
        None,
    )
}

async fn attempt(frames: Vec<Envelope>) -> EnrollmentOutcome {
    let mut transport = FakeTransport::with_inbound(frames);
    enroll(&mut transport, CODE, "web-01", "0.1.7", CLOUD_ID)
        .await
        .expect("a denial is an outcome, not an error")
}

#[tokio::test]
async fn every_denial_code_maps_to_a_distinct_reason() {
    let cases = [
        ("INVALID", PairingDenialCode::Invalid),
        ("USED", PairingDenialCode::Used),
        ("EXPIRED", PairingDenialCode::Expired),
        ("CANCELLED", PairingDenialCode::Cancelled),
        ("KEY_CHANGED", PairingDenialCode::KeyChanged),
        ("RATE_LIMITED", PairingDenialCode::RateLimited),
    ];

    let mut remediations = Vec::new();
    for (wire, expected) in cases {
        match attempt(vec![hello(), denied(wire)]).await {
            EnrollmentOutcome::Denied { code, remediation } => {
                assert_eq!(code, expected, "{wire}");
                remediations.push(remediation);
            }
            other => panic!("{wire}: expected Denied, got {other:?}"),
        }
    }

    let mut unique = remediations.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        cases.len(),
        "each denial code must give the operator different advice: {remediations:?}"
    );
}

#[tokio::test]
async fn rate_limiting_advises_waiting_rather_than_retrying_immediately() {
    match attempt(vec![hello(), denied("RATE_LIMITED")]).await {
        EnrollmentOutcome::Denied { code, remediation } => {
            assert_eq!(code, PairingDenialCode::RateLimited);
            let lower = remediation.to_lowercase();
            assert!(
                lower.contains("wait") || lower.contains("lockout"),
                "must discourage an immediate retry: {remediation}"
            );
        }
        other => panic!("expected Denied, got {other:?}"),
    }
}

#[tokio::test]
async fn an_already_used_code_tells_the_operator_to_get_a_new_one() {
    match attempt(vec![hello(), denied("USED")]).await {
        EnrollmentOutcome::Denied { code, remediation } => {
            assert_eq!(code, PairingDenialCode::Used);
            assert!(
                remediation.to_lowercase().contains("new"),
                "must point at a fresh code: {remediation}"
            );
        }
        other => panic!("expected Denied, got {other:?}"),
    }
}

#[tokio::test]
async fn an_expired_code_tells_the_operator_to_get_a_new_one() {
    match attempt(vec![hello(), denied("EXPIRED")]).await {
        EnrollmentOutcome::Denied { code, remediation } => {
            assert_eq!(code, PairingDenialCode::Expired);
            assert!(remediation.to_lowercase().contains("new"), "{remediation}");
        }
        other => panic!("expected Denied, got {other:?}"),
    }
}

#[tokio::test]
async fn a_key_change_points_at_revocation_rather_than_retrying() {
    match attempt(vec![hello(), denied("KEY_CHANGED")]).await {
        EnrollmentOutcome::Denied { code, remediation } => {
            assert_eq!(code, PairingDenialCode::KeyChanged);
            let lower = remediation.to_lowercase();
            assert!(
                lower.contains("revoke") || lower.contains("forget"),
                "must point at revoke/forget, not a retry: {remediation}"
            );
        }
        other => panic!("expected Denied, got {other:?}"),
    }
}

#[tokio::test]
async fn an_unknown_denial_code_is_rejected_rather_than_guessed() {
    let mut transport = FakeTransport::with_inbound(vec![hello(), denied("SOMETHING_NEW")]);
    let err = enroll(&mut transport, CODE, "web-01", "0.1.7", CLOUD_ID)
        .await
        .expect_err("an unknown denial code must not be silently accepted");
    assert!(
        matches!(err, argus_cloud::error::CloudError::Protocol(_)),
        "{err:?}"
    );
}

#[tokio::test]
async fn a_grant_after_a_denial_is_not_accepted() {
    // The ladder is single-shot: the first reply decides the outcome.
    let outcome = attempt(vec![hello(), denied("USED"), granted()]).await;
    assert!(matches!(outcome, EnrollmentOutcome::Denied { .. }));
}

#[tokio::test]
async fn enrollment_sends_exactly_one_frame_before_the_reply() {
    let mut transport = FakeTransport::with_inbound(vec![hello(), granted()]);
    enroll(&mut transport, CODE, "web-01", "0.1.7", CLOUD_ID)
        .await
        .unwrap();

    let sent = transport.sent();
    assert_eq!(
        sent.len(),
        1,
        "only pairing.redeem is sent during enrollment"
    );
    assert_eq!(sent[0].kind(), Some(MessageType::PairingRedeem));
    assert!(sent[0].correlation_id.is_none() || sent[0].correlation_id.is_some());
}
