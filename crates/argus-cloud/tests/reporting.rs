//! Reporting contract tests (T050).
//!
//! Asserts the bounds and the attribution rule that FR-019 and FR-021 impose on
//! everything the installation reports: batches stay inside the contract's item
//! limits, telemetry stays inside its byte limit, and no report carries an
//! installation identifier — attribution comes from the authenticated session,
//! so an identifier in a payload would be an attribution bug.

use argus_cloud::mapping::telemetry::{EventCounts, TelemetrySource, telemetry_report};
use argus_cloud::mapping::{activity, event};
use argus_cloud::protocol::messages::{
    MAX_ACTIVITIES_PER_REPORT, MAX_EVENTS_PER_REPORT, MAX_TELEMETRY_BYTES,
};
use argus_domain::{
    Action, CapabilityId, DecisionOutcome, DomainEvent, EventType, Execution, ExecutionStatus,
    HealthState, ResourceId, RiskClass, Severity,
};
use chrono::{TimeZone, Utc};
use uuid::Uuid;

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 21, 12, 0, 0).unwrap()
}

fn domain_event() -> DomainEvent {
    DomainEvent::new(
        Uuid::new_v4(),
        EventType::new("argus.ready").unwrap(),
        now(),
        "argusd",
        "host:web-01",
        Severity::Info,
        None,
        None,
        serde_json::json!({ "version": "0.1.7" }),
    )
}

fn execution() -> Execution {
    Execution {
        action: Action {
            capability: CapabilityId::new("host.status.read").unwrap(),
            resource: Some(ResourceId::new("host", "web-01").unwrap()),
            arguments: serde_json::json!({}),
        },
        status: ExecutionStatus::Completed,
        evidence: serde_json::json!({ "exit": 0 }),
    }
}

fn telemetry_source() -> TelemetrySource {
    TelemetrySource {
        health_state: argus_cloud::protocol::messages::HealthStateWire::Healthy,
        uptime_seconds: 120,
        event_counts: EventCounts {
            info: 1,
            warning: 0,
            error: 0,
        },
        capability_count: 3,
        plugin_count: 1,
        connectivity: argus_domain::CloudConnectivityState::Connected,
        buffered_reports: 0,
        dropped_reports: 0,
    }
}

#[test]
fn event_batches_never_exceed_the_item_bound() {
    let events: Vec<DomainEvent> = (0..(MAX_EVENTS_PER_REPORT * 3 + 1))
        .map(|_| domain_event())
        .collect();

    let batches = event::batches(&events);

    assert_eq!(batches.len(), 4, "the tail gets its own batch");
    for batch in &batches {
        assert!(
            batch.events.len() <= MAX_EVENTS_PER_REPORT,
            "a batch of {} would be rejected by the cloud",
            batch.events.len()
        );
        assert!(!batch.events.is_empty(), "an empty batch is invalid");
    }
}

#[test]
fn no_event_is_lost_to_batching() {
    let events: Vec<DomainEvent> = (0..(MAX_EVENTS_PER_REPORT + 7))
        .map(|_| domain_event())
        .collect();
    let sent: usize = event::batches(&events)
        .iter()
        .map(|batch| batch.events.len())
        .sum();
    assert_eq!(sent, events.len());
}

#[test]
fn activity_batches_never_exceed_the_item_bound() {
    let records: Vec<_> = (0..(MAX_ACTIVITIES_PER_REPORT * 2 + 5))
        .map(|_| {
            activity::record(
                &execution(),
                RiskClass::Read,
                activity::ActivityOutcome::Succeeded,
                activity::ActivityContext::default(),
            )
        })
        .collect();

    let batches = activity::batches(records);

    assert_eq!(batches.len(), 3);
    for batch in &batches {
        assert!(batch.activities.len() <= MAX_ACTIVITIES_PER_REPORT);
        assert!(!batch.activities.is_empty());
    }
}

#[test]
fn telemetry_stays_within_the_byte_bound() {
    let payload = telemetry_report(&telemetry_source(), now()).expect("within bounds");
    let encoded = serde_json::to_vec(&payload).unwrap();

    assert!(
        encoded.len() <= MAX_TELEMETRY_BYTES,
        "{} bytes exceeds the {MAX_TELEMETRY_BYTES}-byte bound",
        encoded.len()
    );
}

#[test]
fn telemetry_reports_the_drop_count_rather_than_hiding_it() {
    let mut source = telemetry_source();
    source.dropped_reports = 9;
    source.buffered_reports = 3;

    let payload = telemetry_report(&source, now()).unwrap();

    assert_eq!(payload.metrics["dropped_reports"], 9);
    assert_eq!(payload.metrics["buffered_reports"], 3);
}

#[test]
fn no_report_payload_carries_an_installation_identifier() {
    // FR-021: attribution comes from the authenticated session. If a payload
    // carried an installation id, it could name another installation, which is
    // exactly the cross-attribution the requirement forbids.
    let telemetry =
        serde_json::to_value(telemetry_report(&telemetry_source(), now()).unwrap()).unwrap();

    let events = serde_json::to_value(event::batches(&[domain_event()]).remove(0)).unwrap();

    let activities = serde_json::to_value(
        activity::batches(vec![activity::record(
            &execution(),
            RiskClass::Read,
            activity::ActivityOutcome::Succeeded,
            activity::ActivityContext::default(),
        )])
        .remove(0),
    )
    .unwrap();

    for (label, payload) in [
        ("telemetry", telemetry),
        ("events", events),
        ("activities", activities),
    ] {
        let rendered = payload.to_string();
        for forbidden in [
            "installation_id",
            "instance_id",
            "tenant_id",
            "environment_id",
        ] {
            assert!(
                !rendered.contains(forbidden),
                "{label} must not carry `{forbidden}`: attribution is the session's job"
            );
        }
    }
}

#[test]
fn a_denied_operation_is_reported_as_denied_not_as_success() {
    // Guards the outcome projection that the cloud's activity view depends on.
    let outcome = activity::outcome_from(DecisionOutcome::Denied, Some(ExecutionStatus::Completed));
    assert_eq!(outcome, activity::ActivityOutcome::Denied);
}

#[test]
fn an_approval_required_operation_is_reported_as_pending_not_executed() {
    let outcome = activity::outcome_from(DecisionOutcome::RequireApproval, None);
    assert_eq!(outcome, activity::ActivityOutcome::PendingApproval);
}

#[test]
fn health_is_reported_in_the_cloud_vocabulary() {
    let report =
        argus_cloud::mapping::health::health_report(&argus_domain::HealthStatus::ready(now()));
    assert_eq!(
        serde_json::to_value(&report).unwrap()["state"],
        "healthy",
        "the cloud rejects local spellings like `ready`"
    );
    assert_ne!(
        argus_cloud::mapping::health::health_state(HealthState::Ready),
        argus_cloud::protocol::messages::HealthStateWire::Unknown
    );
}
