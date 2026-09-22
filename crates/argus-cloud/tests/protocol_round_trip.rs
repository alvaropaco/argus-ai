//! Protocol round-trip and tolerance tests (T012).
//!
//! These assert the wire contract in
//! `specs/001-argus-cloud-sync/contracts/agent-protocol-conformance.md` §1–§3:
//! every message type survives a round trip, unknown fields are tolerated, and
//! every payload shape matches what the cloud's schemas expect.

use argus_cloud::protocol::envelope::{Direction, Envelope, MessageType};
use argus_cloud::protocol::messages::*;
use argus_cloud::protocol::version::{SUPPORTED_PROTOCOL_VERSIONS, negotiate};
use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

fn all_message_types() -> Vec<MessageType> {
    use MessageType::*;
    vec![
        HandshakeHello,
        HandshakeAuthenticate,
        HandshakeReady,
        HandshakeReject,
        PairingRedeem,
        PairingGranted,
        PairingDenied,
        TelemetryReport,
        HealthReport,
        EventsReport,
        ActivitiesReport,
        CapabilitiesPublish,
        TelemetryAck,
        HealthAck,
        EventsAck,
        ActivitiesAck,
        CapabilitiesAck,
        IngestAck,
        ConfigResult,
        CommandResult,
        ConfigState,
        ConfigApply,
        CommandInvoke,
        ConfigPullRequest,
        Ping,
        Pong,
        StreamThrottle,
        SessionRotate,
    ]
}

#[test]
fn every_message_type_survives_a_json_round_trip() {
    for ty in all_message_types() {
        let envelope = Envelope::new(ty, json!({}), None);
        let encoded = serde_json::to_string(&envelope).expect("encodes");
        let decoded: Envelope = serde_json::from_str(&encoded).expect("decodes");
        assert_eq!(decoded, envelope, "{ty:?}");
        assert_eq!(decoded.kind(), Some(ty), "{ty:?}");
    }
}

#[test]
fn envelope_uses_the_type_field_name_the_cloud_expects() {
    let envelope = Envelope::new(MessageType::Ping, json!({}), None);
    let value = serde_json::to_value(&envelope).unwrap();
    assert!(value.get("type").is_some(), "cloud reads `type`");
    assert!(value.get("message_type").is_none());
    assert_eq!(value["protocol_version"], "1.0.0");
}

#[test]
fn correlation_id_echoes_for_replies() {
    let correlation = Uuid::new_v4();
    let reply = Envelope::new(MessageType::Pong, json!({}), Some(correlation));
    assert_eq!(reply.correlation_id, Some(correlation));
}

#[test]
fn unknown_fields_do_not_break_decoding() {
    let wire = json!({
        "protocol_version": "1.0.0",
        "message_id": Uuid::new_v4().to_string(),
        "correlation_id": null,
        "sent_at": Utc::now().to_rfc3339(),
        "type": "handshake.hello",
        "payload": {},
        "future_major_field": {"deeply": {"nested": [1, 2, 3]}}
    });
    let decoded: Envelope = serde_json::from_value(wire).expect("additive evolution tolerated");
    assert_eq!(decoded.kind(), Some(MessageType::HandshakeHello));
}

#[test]
fn unknown_message_type_decodes_but_does_not_route() {
    let wire = json!({
        "protocol_version": "1.0.0",
        "message_id": Uuid::new_v4().to_string(),
        "sent_at": Utc::now().to_rfc3339(),
        "type": "some.future.message",
        "payload": {"anything": true}
    });
    let decoded: Envelope = serde_json::from_value(wire).expect("decodes");
    assert!(decoded.kind().is_none(), "unknown type must not route");
}

#[test]
fn directions_match_the_contract() {
    use MessageType::*;
    for ty in [
        HandshakeHello,
        HandshakeReady,
        HandshakeReject,
        PairingGranted,
        PairingDenied,
        TelemetryAck,
        HealthAck,
        EventsAck,
        ActivitiesAck,
        CapabilitiesAck,
        IngestAck,
        ConfigApply,
        CommandInvoke,
        ConfigPullRequest,
        Ping,
        Pong,
        StreamThrottle,
        SessionRotate,
    ] {
        assert_eq!(ty.direction(), Direction::CloudToInstallation, "{ty:?}");
    }
    for ty in [
        HandshakeAuthenticate,
        PairingRedeem,
        TelemetryReport,
        HealthReport,
        EventsReport,
        ActivitiesReport,
        CapabilitiesPublish,
        ConfigResult,
        CommandResult,
        ConfigState,
    ] {
        assert_eq!(ty.direction(), Direction::InstallationToCloud, "{ty:?}");
    }
}

#[test]
fn handshake_hello_payload_round_trips() {
    let payload = HandshakeHelloPayload {
        cloud_id: "argus-cloud".into(),
        cloud_instance: "gateway-7".into(),
        server_time: Utc::now(),
        supported_protocol_versions: vec!["1.0.0".into()],
        challenge: "nonce-value".into(),
        heartbeat_interval_seconds: 20,
    };
    let json = serde_json::to_string(&payload).unwrap();
    let back: HandshakeHelloPayload = serde_json::from_str(&json).unwrap();
    assert_eq!(back, payload);
}

#[test]
fn pairing_redeem_payload_matches_cloud_required_fields() {
    let payload = PairingRedeemPayload {
        code: "ARGUS-7F3K-9Q2M-4XZ8".into(),
        protocol_version: "1.0.0".into(),
        agent_version: "0.1.7".into(),
        hostname: "web-01".into(),
        public_key: "placeholder-public-key-non-decodable-000000".into(),
        challenge_signature: "placeholder-signature-000000000000".into(),
        capability_schema_version: Some("0.1.0".into()),
    };
    let value = serde_json::to_value(&payload).unwrap();
    for field in [
        "code",
        "protocol_version",
        "agent_version",
        "hostname",
        "public_key",
        "challenge_signature",
    ] {
        assert!(value.get(field).is_some(), "cloud requires `{field}`");
    }
    assert!(payload.code.len() >= MIN_PAIRING_CODE_LEN);
    assert!(payload.code.len() <= MAX_PAIRING_CODE_LEN);
    assert!(payload.public_key.len() >= MIN_PUBLIC_KEY_LEN);
    assert!(payload.public_key.len() <= MAX_PUBLIC_KEY_LEN);
    assert!(payload.challenge_signature.len() >= MIN_CHALLENGE_SIGNATURE_LEN);
    assert!(payload.challenge_signature.len() <= MAX_CHALLENGE_SIGNATURE_LEN);
    assert!(payload.agent_version.len() <= MAX_AGENT_VERSION_LEN);
    assert!(payload.hostname.len() <= MAX_HOSTNAME_LEN);
}

#[test]
fn handshake_authenticate_carries_the_session_proof() {
    let payload = HandshakeAuthenticatePayload {
        instance_id: Uuid::new_v4(),
        protocol_version: "1.0.0".into(),
        agent_version: "0.1.7".into(),
        hostname: "web-01".into(),
        challenge_signature: "placeholder-signature-000000000000".into(),
        session_proof: "credential-value".into(),
        capability_schema_version: None,
    };
    let value = serde_json::to_value(&payload).unwrap();
    assert!(value.get("session_proof").is_some());
    assert!(value.get("session_token").is_none());
}

#[test]
fn health_report_uses_cloud_vocabulary() {
    let payload = HealthReportPayload {
        state: HealthStateWire::Healthy,
        summary: "all good".into(),
        details: None,
        reported_at: Utc::now(),
    };
    let value = serde_json::to_value(&payload).unwrap();
    assert_eq!(value["state"], "healthy");
    assert!(payload.summary.len() <= MAX_HEALTH_SUMMARY_LEN);
}

#[test]
fn capability_descriptor_publishes_pascal_risk_class() {
    let payload = CapabilitiesPublishPayload {
        version: "0.1.0".into(),
        capabilities: vec![CapabilityDescriptorWire {
            id: "host.service.restart".into(),
            provider: "argusd".into(),
            operation: "service.restart".into(),
            risk_class: RiskClassWire::LowRisk,
            reversibility: ReversibilityWire::Reversible,
            input_schema: None,
            output_schema: None,
            description: None,
            requires_approval: Some(false),
        }],
    };
    let value = serde_json::to_value(&payload).unwrap();
    assert_eq!(value["capabilities"][0]["risk_class"], "LowRisk");
    assert_eq!(value["capabilities"][0]["reversibility"], "reversible");
    assert!(payload.capabilities.len() <= MAX_CAPABILITIES_PER_PUBLICATION);
}

#[test]
fn events_report_is_within_the_batch_bound() {
    let events = (0..MAX_EVENTS_PER_REPORT)
        .map(|i| ReportedEvent {
            source_event_id: format!("event-{i}"),
            event_type: "argus.ready".into(),
            severity: SeverityWire::Info,
            subject: Some("argusd".into()),
            correlation_id: None,
            causation_id: None,
            reported_at: Utc::now(),
            payload: None,
        })
        .collect::<Vec<_>>();
    let payload = EventsReportPayload { events };
    assert_eq!(payload.events.len(), MAX_EVENTS_PER_REPORT);
    assert!(payload.events.len() >= MIN_EVENTS_PER_REPORT);
    serde_json::to_string(&payload).expect("a full batch still encodes");
}

#[test]
fn activities_report_is_within_the_batch_bound() {
    let activities = (0..MAX_ACTIVITIES_PER_REPORT)
        .map(|_| ActivityRecordWire {
            intent_id: None,
            capability_id: "host.service.restart".into(),
            risk_class: RiskClassWire::LowRisk,
            resource: Some("nginx.service".into()),
            outcome: ActivityOutcomeWire::Succeeded,
            reason: None,
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
            correlation_id: None,
        })
        .collect::<Vec<_>>();
    let payload = ActivitiesReportPayload { activities };
    assert_eq!(payload.activities.len(), MAX_ACTIVITIES_PER_REPORT);
    assert!(payload.activities.len() >= MIN_ACTIVITIES_PER_REPORT);
}

#[test]
fn command_result_only_uses_the_three_terminal_statuses() {
    for status in [
        CommandResultStatus::Acknowledged,
        CommandResultStatus::Failed,
        CommandResultStatus::Refused,
    ] {
        let value = serde_json::to_value(status).unwrap();
        assert!(["acknowledged", "failed", "refused"].contains(&value.as_str().unwrap()));
    }
    // `pending` is deliberately not representable: the cloud has no such state.
    assert!(serde_json::from_str::<CommandResultStatus>("\"pending\"").is_err());
}

#[test]
fn config_result_only_uses_the_three_outcomes() {
    for status in [
        ConfigResultStatus::Applied,
        ConfigResultStatus::Rejected,
        ConfigResultStatus::Failed,
    ] {
        serde_json::to_value(status).expect("encodes");
    }
    assert!(serde_json::from_str::<ConfigResultStatus>("\"partial\"").is_err());
}

#[test]
fn telemetry_bound_is_the_documented_value() {
    assert_eq!(MAX_TELEMETRY_BYTES, 256 * 1024);
}

#[test]
fn version_negotiation_uses_the_supported_set() {
    assert_eq!(SUPPORTED_PROTOCOL_VERSIONS, &["1.0.0"]);
    assert!(negotiate(&["1.0.0".to_string()]).is_some());
    assert!(negotiate(&["9.9.9".to_string()]).is_none());
}
