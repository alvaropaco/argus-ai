//! Reconnect behaviour tests (T042).
//!
//! Drives the reconnect policy and one connection cycle through the crate's
//! public API. A scripted factory stands in for the network, so a gateway
//! restart, a failed connection, and a revocation are all reproducible without
//! sockets.

use std::time::Duration;

use argus_cloud::client::identity::InstallationKey;
use argus_cloud::client::supervisor::{
    ConnectionOutcome, NextAction, ReconnectPolicy, SessionRequest, StopReason, connect_once,
};
use argus_cloud::protocol::messages::PairingGrantedPayload;
use argus_cloud::protocol::{Envelope, MessageType};
use argus_cloud::state::EnrolledIdentity;
use argus_cloud::transport::fake::FakeFactory;
use chrono::{TimeZone, Utc};
use serde_json::json;
use uuid::Uuid;

const CLOUD_ID: &str = "argus-cloud";
const SESSION_PROOF: &str = "credential-issued-at-enrollment";
const ENDPOINT: &str = "wss://cloud.example.com/agent";
const BASE: Duration = Duration::from_secs(1);
const MAX: Duration = Duration::from_secs(300);

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

/// A full successful session: welcome then ready.
fn a_good_connection() -> Vec<Envelope> {
    vec![hello(), ready()]
}

async fn one_cycle(
    factory: &FakeFactory,
    identity: &EnrolledIdentity,
) -> Result<(), ConnectionOutcome> {
    let key = key();
    connect_once(
        factory,
        SessionRequest {
            endpoint: ENDPOINT,
            expected_cloud_id: CLOUD_ID,
            identity,
            key: &key,
            session_proof: Some(SESSION_PROOF),
            hostname: "web-01",
            agent_version: "0.1.7",
            capability_schema_version: Some("0.1.0"),
        },
    )
    .await
    .map(|_| ())
}

fn policy() -> ReconnectPolicy {
    ReconnectPolicy::with_seed(BASE, MAX, 7)
}

#[tokio::test]
async fn a_gateway_restart_reconnects_and_establishes_a_new_session() {
    let factory = FakeFactory::new();
    factory.script_connection(a_good_connection());
    factory.script_connection(a_good_connection());
    let identity = identity();

    one_cycle(&factory, &identity).await.expect("first session");
    one_cycle(&factory, &identity)
        .await
        .expect("session after restart");

    assert_eq!(
        factory.connect_attempts(),
        2,
        "each session needs its own connection"
    );
}

#[tokio::test]
async fn reconnecting_uses_a_new_connection_rather_than_reusing_the_old_one() {
    let factory = FakeFactory::new();
    factory.script_connection(a_good_connection());
    factory.script_connection(a_good_connection());
    let identity = identity();

    one_cycle(&factory, &identity).await.expect("first");
    assert_eq!(factory.connect_attempts(), 1);

    one_cycle(&factory, &identity).await.expect("second");
    assert_eq!(
        factory.connect_attempts(),
        2,
        "a reconnect must not resume the dead connection"
    );
}

#[tokio::test]
async fn no_authenticate_frame_is_duplicated_across_a_reconnect() {
    let factory = FakeFactory::new();
    factory.script_connection(a_good_connection());
    factory.script_connection(a_good_connection());
    let identity = identity();

    one_cycle(&factory, &identity).await.expect("first");
    one_cycle(&factory, &identity).await.expect("second");

    assert_eq!(
        factory.count_sent(MessageType::HandshakeAuthenticate),
        2,
        "exactly one authenticate per connection, never a replay"
    );
    assert_eq!(factory.count_sent(MessageType::HandshakeHello), 0);
}

#[tokio::test]
async fn a_failed_connection_is_classified_and_retried_within_the_cap() {
    let factory = FakeFactory::new();
    factory.fail_next_connection("connection refused");
    let identity = identity();

    let outcome = one_cycle(&factory, &identity)
        .await
        .expect_err("connect fails");
    assert!(
        matches!(outcome, ConnectionOutcome::TransportFailed(_)),
        "{outcome:?}"
    );

    let mut policy = policy();
    match policy.after(outcome) {
        NextAction::Reconnect(delay) => assert!(delay <= MAX, "delay {delay:?} exceeded cap"),
        other => panic!("expected Reconnect, got {other:?}"),
    }
}

#[tokio::test]
async fn a_revoked_instance_ends_supervision_rather_than_looping() {
    let factory = FakeFactory::new();
    factory.script_connection(vec![
        hello(),
        Envelope::new(
            MessageType::HandshakeReject,
            json!({ "code": "REVOKED", "message": "instance revoked" }),
            None,
        ),
    ]);
    let identity = identity();

    let outcome = one_cycle(&factory, &identity).await.expect_err("revoked");

    let mut policy = policy();
    assert_eq!(policy.after(outcome), NextAction::Stop(StopReason::Revoked));
}

#[tokio::test]
async fn a_version_mismatch_stops_and_needs_a_human() {
    let factory = FakeFactory::new();
    factory.script_connection(vec![Envelope::new(
        MessageType::HandshakeHello,
        json!({
            "cloud_id": CLOUD_ID,
            "cloud_instance": "gateway-7",
            "server_time": now().to_rfc3339(),
            "supported_protocol_versions": ["9.0.0"],
            "challenge": "nonce",
            "heartbeat_interval_seconds": 20
        }),
        None,
    )]);
    let identity = identity();

    let outcome = one_cycle(&factory, &identity)
        .await
        .expect_err("no shared version");
    let mut policy = policy();
    assert_eq!(
        policy.after(outcome),
        NextAction::Stop(StopReason::UnsupportedVersion)
    );
}

#[tokio::test]
async fn a_suspension_reconnects_rather_than_giving_up() {
    let factory = FakeFactory::new();
    factory.script_connection(vec![
        hello(),
        Envelope::new(
            MessageType::HandshakeReject,
            json!({ "code": "SUSPENDED", "message": "instance suspended" }),
            None,
        ),
    ]);
    let identity = identity();

    let outcome = one_cycle(&factory, &identity).await.expect_err("suspended");
    let mut policy = policy();
    assert!(
        matches!(policy.after(outcome), NextAction::Reconnect(_)),
        "suspension must not strand a correctly-behaving installation"
    );
}

#[test]
fn a_successful_session_resets_the_schedule_so_one_flap_does_not_punish_forever() {
    let mut policy = policy();
    for _ in 0..8 {
        policy.after(ConnectionOutcome::TransportFailed("reset".into()));
    }
    assert_eq!(policy.attempt(), 8);

    policy.note_connected();
    assert_eq!(policy.attempt(), 0);
    assert!(matches!(
        policy.after(ConnectionOutcome::SessionEnded),
        NextAction::Reconnect(_)
    ));
}

#[test]
fn different_installations_do_not_converge_on_the_same_schedule() {
    // A shared schedule would turn a cloud restart into a thundering herd.
    let mut a = ReconnectPolicy::with_seed(BASE, MAX, 1001);
    let mut b = ReconnectPolicy::with_seed(BASE, MAX, 2002);

    for _ in 0..10 {
        a.after(ConnectionOutcome::TransportFailed("x".into()));
        b.after(ConnectionOutcome::TransportFailed("x".into()));
    }

    let a_delays: Vec<Duration> = (0..5)
        .map(|_| match a.after(ConnectionOutcome::SessionEnded) {
            NextAction::Reconnect(delay) => delay,
            other => panic!("{other:?}"),
        })
        .collect();
    let b_delays: Vec<Duration> = (0..5)
        .map(|_| match b.after(ConnectionOutcome::SessionEnded) {
            NextAction::Reconnect(delay) => delay,
            other => panic!("{other:?}"),
        })
        .collect();

    assert_ne!(a_delays, b_delays);
}
