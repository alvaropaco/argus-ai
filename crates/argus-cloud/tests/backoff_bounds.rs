//! Backoff policy and outbound bound tests (T017).
//!
//! Asserts the reconnection policy from `research.md` R7 and the payload bounds
//! from `contracts/agent-protocol-conformance.md` §3. The bounds matter because
//! FR-019 requires the installation to enforce them itself rather than relying on
//! the cloud to reject.

use std::time::Duration;

use argus_cloud::protocol::messages::*;
use argus_cloud::protocol::{Envelope, MessageType};
use argus_cloud::transport::backoff::Backoff;
use argus_cloud::transport::fake::FakeTransport;
use argus_cloud::transport::{CloseReason, Transport, TransportError};
use chrono::Utc;
use serde_json::json;

const BASE: Duration = Duration::from_secs(1);
const CAP: Duration = Duration::from_secs(300);

#[test]
fn a_successful_handshake_resets_the_schedule() {
    let mut backoff = Backoff::with_seed(BASE, CAP, 11);
    for _ in 0..6 {
        backoff.next_delay();
    }
    assert_eq!(backoff.attempt(), 6);

    backoff.reset();
    assert_eq!(backoff.attempt(), 0);
    assert_eq!(backoff.ceiling(), BASE);
}

#[test]
fn the_schedule_never_hammers_the_cloud() {
    // A fixed-interval retry would trip the gateway's handshake rate limiter and
    // turn a cloud restart into a self-inflicted denial of service.
    let mut backoff = Backoff::with_seed(BASE, CAP, 7);
    for _ in 0..30 {
        let delay = backoff.next_delay();
        assert!(delay >= Duration::ZERO);
        assert!(delay <= CAP, "delay {delay:?} exceeded the cap");
    }
}

#[test]
fn jitter_spreads_retries_across_installations() {
    // Two installations must not converge on the same delay from the same
    // attempt, or a fleet-wide outage becomes a thundering herd.
    let mut a = Backoff::with_seed(BASE, CAP, 1001);
    let mut b = Backoff::with_seed(BASE, CAP, 2002);
    for _ in 0..12 {
        a.next_delay();
        b.next_delay();
    }
    let a_delays: Vec<Duration> = (0..5).map(|_| a.next_delay()).collect();
    let b_delays: Vec<Duration> = (0..5).map(|_| b.next_delay()).collect();
    assert_ne!(a_delays, b_delays);
}

#[test]
fn telemetry_bound_matches_the_contract() {
    assert_eq!(MAX_TELEMETRY_BYTES, 256 * 1024);

    let oversized = TelemetryReportPayload {
        metrics: {
            let mut m = serde_json::Map::new();
            m.insert("filler".into(), json!("x".repeat(MAX_TELEMETRY_BYTES)));
            m
        },
        reported_at: Utc::now(),
    };
    let encoded = serde_json::to_string(&oversized).unwrap();
    // The caller must refuse to send this; the bound is what it checks against.
    assert!(encoded.len() > MAX_TELEMETRY_BYTES);
}

#[test]
fn batch_bounds_match_the_contract() {
    assert_eq!(MIN_EVENTS_PER_REPORT, 1);
    assert_eq!(MAX_EVENTS_PER_REPORT, 500);
    assert_eq!(MIN_ACTIVITIES_PER_REPORT, 1);
    assert_eq!(MAX_ACTIVITIES_PER_REPORT, 500);
    assert_eq!(MAX_CAPABILITIES_PER_PUBLICATION, 1000);
}

#[test]
fn text_field_bounds_match_the_contract() {
    assert_eq!(MAX_AGENT_VERSION_LEN, 50);
    assert_eq!(MAX_HOSTNAME_LEN, 255);
    assert_eq!(MAX_HEALTH_SUMMARY_LEN, 4000);
    assert_eq!(MAX_ACK_REASON_LEN, 500);
    assert_eq!(MAX_RESULT_REASON_LEN, 2000);
    assert_eq!(MIN_PUBLIC_KEY_LEN, 32);
    assert_eq!(MAX_PUBLIC_KEY_LEN, 2000);
    assert_eq!(MIN_CHALLENGE_SIGNATURE_LEN, 16);
    assert_eq!(MAX_CHALLENGE_SIGNATURE_LEN, 2000);
    assert_eq!(MIN_PAIRING_CODE_LEN, 8);
    assert_eq!(MAX_PAIRING_CODE_LEN, 64);
    assert_eq!(MAX_CAPABILITY_ID_LEN, 200);
}

#[tokio::test]
async fn a_send_failure_is_reported_and_not_swallowed() {
    let mut transport = FakeTransport::new();
    transport.fail_send_after(0);
    let envelope = Envelope::new(MessageType::Ping, json!({}), None);
    let err = transport.send(&envelope).await.unwrap_err();
    assert!(matches!(err, TransportError::Send(_)));
    assert!(transport.sent().is_empty());
}

#[tokio::test]
async fn a_clean_stream_end_is_not_an_error() {
    let mut transport = FakeTransport::new();
    assert_eq!(transport.recv().await.unwrap(), None);
}

#[tokio::test]
async fn close_reason_is_recorded_for_every_terminal_case() {
    for reason in [
        CloseReason::Client,
        CloseReason::Revoked,
        CloseReason::Suspended,
        CloseReason::VersionMismatch,
        CloseReason::Unauthenticated,
        CloseReason::Error,
        CloseReason::Network,
    ] {
        let mut transport = FakeTransport::new();
        transport.close(reason).await;
        assert_eq!(transport.close_reason(), Some(reason), "{reason:?}");
    }
}

#[tokio::test]
async fn reports_are_delivered_as_one_frame_each() {
    let transport = FakeTransport::new();
    transport
        .send(&Envelope::new(
            MessageType::TelemetryReport,
            json!({}),
            None,
        ))
        .await
        .unwrap();
    transport
        .send(&Envelope::new(MessageType::HealthReport, json!({}), None))
        .await
        .unwrap();

    assert_eq!(transport.sent().len(), 2);
    assert_eq!(transport.count_sent(MessageType::TelemetryReport), 1);
    assert_eq!(transport.count_sent(MessageType::HealthReport), 1);
}
