//! Capability publication tests (T059).
//!
//! Covers the publication contract at the crate's public API: descriptor
//! translation fidelity, an explicit empty publication, supersession by content
//! hash, and that publishing never mutates the local descriptors (FR-025,
//! FR-026).

use argus_cloud::mapping::capability::{publication, reversibility, risk_class};
use argus_cloud::protocol::messages::{
    MAX_CAPABILITIES_PER_PUBLICATION, ReversibilityWire, RiskClassWire,
};
use argus_domain::{
    CapabilityDescriptor, CapabilityId, CapabilityPublication, Reversibility, RiskClass,
};
use chrono::{TimeZone, Utc};
use semver::Version;

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 21, 12, 0, 0).unwrap()
}

fn descriptor(id: &str, risk: RiskClass, rev: Reversibility) -> CapabilityDescriptor {
    CapabilityDescriptor::new(
        CapabilityId::new(id).unwrap(),
        "argusd",
        "service.restart",
        risk,
        Version::new(0, 1, 0),
        serde_json::json!({}),
        serde_json::json!({"type": "object"}),
        rev,
    )
}

#[test]
fn every_risk_class_is_published_in_the_cloud_casing() {
    let cases = [
        (RiskClass::Read, RiskClassWire::Read),
        (RiskClass::LowRisk, RiskClassWire::LowRisk),
        (RiskClass::Controlled, RiskClassWire::Controlled),
        (RiskClass::HighRisk, RiskClassWire::HighRisk),
        (RiskClass::Destructive, RiskClassWire::Destructive),
    ];

    for (local, expected) in cases {
        assert_eq!(risk_class(local), expected, "{local:?}");
        let wire = serde_json::to_string(&risk_class(local)).unwrap();
        assert!(
            wire.contains(char::is_uppercase),
            "the cloud expects PascalCase, not the local snake_case: {wire}"
        );
    }
}

#[test]
fn irreversibility_is_never_published_as_not_applicable() {
    // `n/a` would read as "reversibility is irrelevant"; the local `none` means
    // the operation cannot be undone, which the operator needs to be warned about.
    assert_eq!(
        reversibility(Reversibility::None),
        ReversibilityWire::Irreversible
    );
    assert_ne!(
        reversibility(Reversibility::None),
        ReversibilityWire::NotApplicable
    );
}

#[test]
fn partial_reversibility_collapses_to_reversible() {
    assert_eq!(
        reversibility(Reversibility::PartiallyReversible),
        ReversibilityWire::Reversible
    );
    assert_eq!(
        reversibility(Reversibility::Reversible),
        ReversibilityWire::Reversible
    );
}

#[test]
fn a_published_descriptor_omits_the_local_version_field() {
    let surface = vec![descriptor(
        "host.status.read",
        RiskClass::Read,
        Reversibility::None,
    )];
    let payload = publication("0.1.0", &surface, |_| false).unwrap();
    let value = serde_json::to_value(&payload).unwrap();

    assert!(
        value["capabilities"][0].get("version").is_none(),
        "version is a publication-level concern, not a descriptor field"
    );
}

#[test]
fn an_empty_surface_is_published_explicitly_rather_than_omitted() {
    let payload = publication("0.1.0", &[], |_| false).unwrap();
    let value = serde_json::to_value(&payload).unwrap();

    assert!(
        value["capabilities"].is_array(),
        "the cloud must be able to tell 'nothing' from 'no answer'"
    );
    assert!(value["capabilities"].as_array().unwrap().is_empty());
}

#[test]
fn the_publication_carries_its_schema_version() {
    let payload = publication("2.3.1", &[], |_| false).unwrap();
    assert_eq!(payload.version, "2.3.1");
}

#[test]
fn an_oversized_surface_is_refused_before_it_is_sent() {
    let surface: Vec<CapabilityDescriptor> = (0..=MAX_CAPABILITIES_PER_PUBLICATION)
        .map(|i| {
            descriptor(
                &format!("cap.item{i}"),
                RiskClass::Read,
                Reversibility::None,
            )
        })
        .collect();

    assert!(
        publication("0.1.0", &surface, |_| false).is_err(),
        "the installation enforces the bound rather than letting the cloud reject it"
    );
}

#[test]
fn exactly_the_limit_is_accepted() {
    let surface: Vec<CapabilityDescriptor> = (0..MAX_CAPABILITIES_PER_PUBLICATION)
        .map(|i| {
            descriptor(
                &format!("cap.item{i}"),
                RiskClass::Read,
                Reversibility::None,
            )
        })
        .collect();

    let payload = publication("0.1.0", &surface, |_| false).unwrap();
    assert_eq!(payload.capabilities.len(), MAX_CAPABILITIES_PER_PUBLICATION);
}

#[test]
fn publishing_never_mutates_the_local_descriptors() {
    // FR-026: the cloud sees a projection; the local registry stays the source of
    // truth and must be untouched by publishing.
    let surface = vec![descriptor(
        "host.service.restart",
        RiskClass::LowRisk,
        Reversibility::Reversible,
    )];
    let before = surface.clone();

    let _ = publication("0.1.0", &surface, |_| false).unwrap();

    assert_eq!(surface, before, "publishing must be read-only");
    assert_eq!(surface[0].risk_class(), RiskClass::LowRisk);
}

#[test]
fn identical_capabilities_produce_the_same_content_hash() {
    let surface = vec![descriptor(
        "host.status.read",
        RiskClass::Read,
        Reversibility::None,
    )];
    let first = CapabilityPublication::new("0.1.0", surface.clone(), now());
    let second = CapabilityPublication::new("0.2.0", surface.clone(), now());

    assert!(
        first.is_noop_for(&surface) && second.is_noop_for(&surface),
        "an unchanged surface must be detectable regardless of the version label"
    );
}

#[test]
fn a_changed_surface_is_not_a_noop() {
    let original = vec![descriptor(
        "host.status.read",
        RiskClass::Read,
        Reversibility::None,
    )];
    let publication_record = CapabilityPublication::new("0.1.0", original.clone(), now());

    let grown = vec![
        descriptor("host.status.read", RiskClass::Read, Reversibility::None),
        descriptor(
            "host.service.restart",
            RiskClass::LowRisk,
            Reversibility::Reversible,
        ),
    ];

    assert!(
        !publication_record.is_noop_for(&grown),
        "a larger surface must produce a new publication"
    );
    assert!(!publication_record.is_noop_for(&[]));
}

#[test]
fn each_publication_has_its_own_identity_so_history_is_retainable() {
    let surface = vec![descriptor(
        "host.status.read",
        RiskClass::Read,
        Reversibility::None,
    )];
    let first = CapabilityPublication::new("0.1.0", surface.clone(), now());
    let second = CapabilityPublication::new("0.2.0", surface, now());

    assert_ne!(
        first.publication_id, second.publication_id,
        "superseding a publication must not overwrite the previous one"
    );
}
