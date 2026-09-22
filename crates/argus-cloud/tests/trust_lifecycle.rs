//! Trust lifecycle tests (T091).
//!
//! Asserts how a lifecycle outcome changes a connection's future: revocation
//! ends supervision for good, suspension keeps it recoverable, a resumed
//! installation does not carry the backoff it accumulated while suspended, and a
//! session that ends for any reason loses no queued work.

use std::time::Duration;

use argus_cloud::buffer::{ReportKind, ReportQueue};
use argus_cloud::client::handshake::HandshakeFailure;
use argus_cloud::client::reporting::{assemble_batches, requeue_batches};
use argus_cloud::client::supervisor::{ConnectionOutcome, NextAction, ReconnectPolicy, StopReason};
use argus_cloud::protocol::{Envelope, MessageType};
use chrono::{TimeZone, Utc};
use serde_json::json;

const BASE: Duration = Duration::from_secs(1);
const MAX: Duration = Duration::from_secs(300);

fn policy() -> ReconnectPolicy {
    ReconnectPolicy::with_seed(BASE, MAX, 11)
}

fn rejected(code: &str) -> ConnectionOutcome {
    ConnectionOutcome::HandshakeFailed(HandshakeFailure::Rejected {
        code: code.into(),
        message: String::new(),
    })
}

fn at() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 21, 12, 0, 0).unwrap()
}

#[test]
fn a_revoked_installation_stops_reconnecting_for_good() {
    // Revocation is the only terminal lifecycle outcome; continuing to retry
    // would keep a compromised host knocking.
    let mut policy = policy();
    assert_eq!(
        policy.after(rejected("REVOKED")),
        NextAction::Stop(StopReason::Revoked)
    );
    assert!(
        matches!(policy.after(rejected("REVOKED")), NextAction::Stop(_)),
        "a second attempt must not become retryable"
    );
}

#[test]
fn a_suspended_installation_keeps_trying() {
    // Suspension is temporary by definition. Treating it as terminal would strand
    // an installation that is behaving correctly.
    let action = policy().after(rejected("SUSPENDED"));
    assert!(
        matches!(action, NextAction::Reconnect(_)),
        "suspension must stay recoverable: {action:?}"
    );
}

#[test]
fn a_resumed_installation_does_not_carry_its_old_backoff() {
    // While suspended the installation backs off; on resume it should reconnect
    // promptly rather than waiting out a delay accumulated for a different state.
    let mut policy = policy();
    for _ in 0..6 {
        policy.after(rejected("SUSPENDED"));
    }
    assert_eq!(policy.attempt(), 6);

    policy.note_connected();
    assert_eq!(
        policy.attempt(),
        0,
        "a successful session resets the schedule"
    );
}

#[test]
fn a_version_mismatch_needs_an_upgrade_rather_than_a_retry() {
    let action = policy().after(ConnectionOutcome::HandshakeFailed(
        HandshakeFailure::UnsupportedVersion {
            offered: vec!["9.0.0".into()],
        },
    ));
    assert_eq!(action, NextAction::Stop(StopReason::UnsupportedVersion));
}

#[test]
fn a_credential_rotation_is_a_recognised_control_message() {
    // A rotation the installation does not recognise would leave it holding a
    // credential the cloud has already retired.
    let rotate = Envelope::new(
        MessageType::SessionRotate,
        json!({ "rotation_token": "token", "expires_at": at().to_rfc3339() }),
        None,
    );
    assert_eq!(rotate.kind(), Some(MessageType::SessionRotate));
    assert!(
        !argus_cloud::client::reporting::is_report_ack(MessageType::SessionRotate),
        "a rotation is control, not an acknowledgement"
    );
}

#[test]
fn a_session_ending_loses_no_queued_work() {
    // Whatever ends a session — a suspension, a rotation, a dropped socket — the
    // reports in flight must come back rather than disappear.
    let mut queue = ReportQueue::new(8);
    queue.enqueue(ReportKind::Telemetry, json!({ "tag": "in-flight" }), at());

    let in_flight = assemble_batches(&mut queue, at());
    assert!(queue.is_empty(), "drained for sending");

    requeue_batches(&mut queue, in_flight);

    let recovered = queue.drain(8, at());
    assert_eq!(recovered.len(), 1, "the report came back");
    assert_eq!(recovered[0].payload, json!({ "tag": "in-flight" }));
}

#[test]
fn repeated_lifecycle_rejections_never_hammer_the_cloud() {
    let mut policy = policy();
    for _ in 0..20 {
        match policy.after(rejected("SUSPENDED")) {
            NextAction::Reconnect(delay) => assert!(delay <= MAX, "delay {delay:?} exceeded cap"),
            other => panic!("expected Reconnect, got {other:?}"),
        }
    }
}
