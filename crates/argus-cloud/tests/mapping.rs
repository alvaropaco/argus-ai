//! Mapping contract tests (T024).
//!
//! These lock the vocabulary translation fixed by ADR-0016 and the serialisation
//! rule that the cloud's schemas impose: fields declared `.optional()` accept an
//! absent key but reject `null`, while the single `.nullable()` field requires an
//! explicit `null` and rejects absence. A `null` where the cloud wants an absent
//! key is a rejection, so this is a correctness contract, not a style choice.

use argus_cloud::mapping::{activity, capability, event, health};
use argus_cloud::protocol::messages::{
    ActivitiesReportPayload, CapabilityDescriptorWire, EventsReportPayload,
    MAX_ACTIVITIES_PER_REPORT, MAX_EVENTS_PER_REPORT, RiskClassWire,
};
use argus_domain::{
    Action, CapabilityDescriptor, CapabilityId, DecisionOutcome, DomainEvent, EventType, Execution,
    ExecutionStatus, HealthState, HealthStatus, ResourceId, Reversibility, RiskClass, Severity,
};
use chrono::{TimeZone, Utc};
use semver::Version;
use serde_json::json;
use uuid::Uuid;

fn ts() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 21, 12, 0, 0).unwrap()
}

fn descriptor(id: &str, risk: RiskClass, rev: Reversibility) -> CapabilityDescriptor {
    CapabilityDescriptor::new(
        CapabilityId::new(id).unwrap(),
        "argusd",
        "service.restart",
        risk,
        Version::new(0, 1, 0),
        json!({}),
        json!({"type": "object"}),
        rev,
    )
}

// --- Health ---

#[test]
fn health_vocabulary_maps_exactly_as_specified() {
    use argus_cloud::protocol::messages::HealthStateWire;
    assert_eq!(
        health::health_state(HealthState::Ready),
        HealthStateWire::Healthy
    );
    assert_eq!(
        health::health_state(HealthState::Degraded),
        HealthStateWire::Degraded
    );
    assert_eq!(
        health::health_state(HealthState::NotReady),
        HealthStateWire::Critical,
        "NotReady must not be reported as unknown"
    );
}

#[test]
fn degraded_health_reason_becomes_the_summary() {
    let report = health::health_report(&HealthStatus::degraded("lancedb unavailable", ts()));
    assert_eq!(report.summary, "lancedb unavailable");
    assert_eq!(serde_json::to_value(&report).unwrap()["state"], "degraded");
}

#[test]
fn health_report_omits_details_rather_than_sending_null() {
    let report = health::health_report(&HealthStatus::ready(ts()));
    let value = serde_json::to_value(&report).unwrap();
    assert!(
        value.get("details").is_none(),
        "cloud .optional() rejects null"
    );
}

// --- Risk class and reversibility ---

#[test]
fn risk_class_case_is_normalised_for_every_variant() {
    let pairs = [
        (RiskClass::Read, "Read"),
        (RiskClass::LowRisk, "LowRisk"),
        (RiskClass::Controlled, "Controlled"),
        (RiskClass::HighRisk, "HighRisk"),
        (RiskClass::Destructive, "Destructive"),
    ];
    for (local, expected) in pairs {
        assert_eq!(
            serde_json::to_value(capability::risk_class(local)).unwrap(),
            expected,
            "{local:?}"
        );
        assert_ne!(
            serde_json::to_value(local).unwrap(),
            expected,
            "{local:?} must not be sent in local casing"
        );
    }
}

#[test]
fn reversibility_none_is_irreversible_and_never_not_applicable() {
    let none = capability::reversibility(Reversibility::None);
    assert_eq!(
        none,
        argus_cloud::protocol::messages::ReversibilityWire::Irreversible
    );
    assert_ne!(
        none,
        argus_cloud::protocol::messages::ReversibilityWire::NotApplicable
    );
    assert_eq!(serde_json::to_value(none).unwrap(), "irreversible");
}

#[test]
fn reversibility_partial_collapses_to_reversible() {
    assert_eq!(
        serde_json::to_value(capability::reversibility(
            Reversibility::PartiallyReversible
        ))
        .unwrap(),
        "reversible"
    );
    assert_eq!(
        serde_json::to_value(capability::reversibility(Reversibility::Reversible)).unwrap(),
        "reversible"
    );
}

// --- Descriptor and publication ---

#[test]
fn descriptor_drops_version_and_omits_unset_optionals() {
    let wire = capability::descriptor(
        &descriptor(
            "host.service.restart",
            RiskClass::LowRisk,
            Reversibility::Reversible,
        ),
        true,
    );
    let value = serde_json::to_value(&wire).unwrap();

    assert!(
        value.get("version").is_none(),
        "version is publication-level"
    );
    assert_eq!(value["requires_approval"], true);
    assert_eq!(value["risk_class"], "LowRisk");

    // The regression guard: these must be absent, never null.
    for absent in ["description", "input_schema"] {
        assert!(
            value.get(absent).is_none(),
            "`{absent}` must be omitted, not serialised as null"
        );
    }
    assert!(
        value.get("output_schema").is_some(),
        "a declared schema is sent"
    );
}

#[test]
fn publication_refuses_an_oversized_surface_before_sending_it() {
    let surface: Vec<CapabilityDescriptor> = (0..1001)
        .map(|i| {
            descriptor(
                &format!("cap.item{i}"),
                RiskClass::Read,
                Reversibility::Reversible,
            )
        })
        .collect();
    assert!(capability::publication("0.1.0", &surface, |_| false).is_err());
}

#[test]
fn an_empty_surface_is_published_explicitly() {
    let payload = capability::publication("0.1.0", &[], |_| false).unwrap();
    let value = serde_json::to_value(&payload).unwrap();
    assert!(value["capabilities"].as_array().unwrap().is_empty());
}

// --- Events ---

#[test]
fn source_event_id_is_the_local_event_uuid_and_is_retry_stable() {
    let id = Uuid::new_v4();
    let source = DomainEvent::new(
        id,
        EventType::new("argus.ready").unwrap(),
        ts(),
        "argusd",
        "host:web-01",
        Severity::Info,
        Some(Uuid::nil()),
        None,
        json!({"version": "0.1.7"}),
    );
    assert_eq!(event::event(&source).source_event_id, id.to_string());
    assert_eq!(event::event(&source), event::event(&source), "retry-stable");
}

#[test]
fn event_batching_never_exceeds_the_item_bound_and_loses_nothing() {
    let events: Vec<DomainEvent> = (0..(MAX_EVENTS_PER_REPORT * 2 + 1))
        .map(|_| {
            DomainEvent::new(
                Uuid::new_v4(),
                EventType::new("argus.ready").unwrap(),
                ts(),
                "argusd",
                "argusd",
                Severity::Info,
                None,
                None,
                json!({}),
            )
        })
        .collect();
    let batches: Vec<EventsReportPayload> = event::batches(&events);
    assert_eq!(batches.len(), 3);
    assert!(
        batches
            .iter()
            .all(|b| b.events.len() <= MAX_EVENTS_PER_REPORT)
    );
    let total: usize = batches.iter().map(|b| b.events.len()).sum();
    assert_eq!(total, events.len());
}

#[test]
fn event_optional_fields_are_omitted_not_nulled() {
    let bare = DomainEvent::new(
        Uuid::new_v4(),
        EventType::new("argus.started").unwrap(),
        ts(),
        "argusd",
        "argusd",
        Severity::Info,
        None,
        None,
        json!({}),
    );
    let value = serde_json::to_value(event::event(&bare)).unwrap();
    for absent in ["causation_id", "payload"] {
        assert!(value.get(absent).is_none(), "`{absent}` must be omitted");
    }
    assert!(value.get("correlation_id").is_none());
}

// --- Activities ---

#[test]
fn activity_outcome_is_derived_never_guessed() {
    use argus_cloud::mapping::activity::ActivityOutcome as A;
    assert_eq!(
        activity::outcome_from(DecisionOutcome::Denied, Some(ExecutionStatus::Completed)),
        A::Denied,
        "a denial can never be reported as success"
    );
    assert_eq!(
        activity::outcome_from(DecisionOutcome::RequireApproval, None),
        A::PendingApproval
    );
    assert_eq!(
        activity::outcome_from(DecisionOutcome::Permitted, Some(ExecutionStatus::Completed)),
        A::Succeeded
    );
    assert_eq!(
        activity::outcome_from(DecisionOutcome::Permitted, Some(ExecutionStatus::Failed)),
        A::Failed
    );
    assert_eq!(
        activity::outcome_from(DecisionOutcome::Permitted, None),
        A::Failed,
        "no record of execution cannot be reported as success"
    );
}

#[test]
fn activity_record_carries_capability_resource_and_cloud_casing() {
    let execution = Execution {
        action: Action {
            capability: CapabilityId::new("host.service.restart").unwrap(),
            resource: Some(ResourceId::new("host", "web-01").unwrap()),
            arguments: json!({"unit": "nginx"}),
        },
        status: ExecutionStatus::Completed,
        evidence: json!({"exit": 0}),
    };
    let wire = activity::record(
        &execution,
        RiskClass::HighRisk,
        activity::ActivityOutcome::Succeeded,
        activity::ActivityContext {
            intent_id: Some("intent-1".into()),
            correlation_id: Some("corr-1".into()),
            started_at: Some(ts()),
            finished_at: Some(ts()),
            reason: None,
        },
    );
    let value = serde_json::to_value(&wire).unwrap();

    assert_eq!(value["capability_id"], "host.service.restart");
    assert_eq!(value["resource"], "host:web-01");
    assert_eq!(value["risk_class"], "HighRisk");
    assert_eq!(value["outcome"], "succeeded");
    assert!(
        value.get("reason").is_none(),
        "unset reason must be omitted"
    );
}

#[test]
fn activity_batching_respects_the_bound() {
    let execution = Execution {
        action: Action {
            capability: CapabilityId::new("host.status.read").unwrap(),
            resource: None,
            arguments: json!({}),
        },
        status: ExecutionStatus::Completed,
        evidence: json!({}),
    };
    let records: Vec<_> = (0..(MAX_ACTIVITIES_PER_REPORT + 1))
        .map(|_| {
            activity::record(
                &execution,
                RiskClass::Read,
                activity::ActivityOutcome::Succeeded,
                activity::ActivityContext::default(),
            )
        })
        .collect();
    let batches: Vec<ActivitiesReportPayload> = activity::batches(records);
    assert_eq!(batches.len(), 2);
    assert_eq!(batches[0].activities.len(), MAX_ACTIVITIES_PER_REPORT);
    assert_eq!(batches[1].activities.len(), 1);
}

// --- The .nullable() exception ---

#[test]
fn config_state_sends_an_explicit_null_because_the_cloud_requires_it() {
    use argus_cloud::protocol::messages::{ConfigStatePayload, ConfigurationStateEntry};
    let payload = ConfigStatePayload {
        configurations: vec![ConfigurationStateEntry {
            configuration_id: Uuid::nil(),
            applied_version_id: None,
        }],
    };
    let value = serde_json::to_value(&payload).unwrap();
    assert!(
        value["configurations"][0]["applied_version_id"].is_null(),
        "the cloud declares this .nullable(): absence is rejected, null is required"
    );
}

#[test]
fn descriptor_wire_type_has_no_version_field() {
    let wire = CapabilityDescriptorWire {
        id: "host.status.read".into(),
        provider: "argusd".into(),
        operation: "status.get".into(),
        risk_class: RiskClassWire::Read,
        reversibility: argus_cloud::protocol::messages::ReversibilityWire::NotApplicable,
        input_schema: None,
        output_schema: None,
        description: None,
        requires_approval: None,
    };
    let value = serde_json::to_value(&wire).unwrap();
    assert!(value.get("version").is_none());
}
