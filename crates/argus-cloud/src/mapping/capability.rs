//! Capability descriptor and publication translation.
//!
//! Two drifts are handled here and nowhere else: the risk class casing (the
//! local enum serialises `low_risk`, the cloud requires `LowRisk`) and the
//! reversibility vocabulary (the local `none` maps to `irreversible`, never to
//! the cloud's `n/a`, because `n/a` would hide a real irreversibility risk).
//!
//! `version` is dropped on publish: the cloud versions the schema at publication
//! level rather than per descriptor. `requires_approval` is derived from the
//! authorization model, never from the descriptor.

use argus_domain::{CapabilityDescriptor, CapabilityId, Reversibility, RiskClass};

use crate::error::MappingError;
use crate::protocol::messages::{
    CapabilitiesPublishPayload, CapabilityDescriptorWire, MAX_CAPABILITIES_PER_PUBLICATION,
    ReversibilityWire, RiskClassWire,
};

/// Normalises the local risk class onto the cloud's casing.
pub fn risk_class(risk: RiskClass) -> RiskClassWire {
    match risk {
        RiskClass::Read => RiskClassWire::Read,
        RiskClass::LowRisk => RiskClassWire::LowRisk,
        RiskClass::Controlled => RiskClassWire::Controlled,
        RiskClass::HighRisk => RiskClassWire::HighRisk,
        RiskClass::Destructive => RiskClassWire::Destructive,
    }
}

/// Translates reversibility.
///
/// `None` means the operation cannot be undone, so it maps to `irreversible`.
/// `PartiallyReversible` maps to `reversible` because the cloud has no partial
/// value and the operation can, to some degree, be undone.
pub fn reversibility(value: Reversibility) -> ReversibilityWire {
    match value {
        Reversibility::None => ReversibilityWire::Irreversible,
        Reversibility::Reversible | Reversibility::PartiallyReversible => {
            ReversibilityWire::Reversible
        }
    }
}

fn schema(value: &serde_json::Value) -> Option<serde_json::Map<String, serde_json::Value>> {
    match value {
        serde_json::Value::Object(map) if !map.is_empty() => Some(map.clone()),
        _ => None,
    }
}

/// Projects one local descriptor onto the cloud's shape.
pub fn descriptor(
    descriptor: &CapabilityDescriptor,
    requires_approval: bool,
) -> CapabilityDescriptorWire {
    CapabilityDescriptorWire {
        id: descriptor.id().as_str().to_string(),
        provider: descriptor.provider().to_string(),
        operation: descriptor.operation().to_string(),
        risk_class: risk_class(descriptor.risk_class()),
        reversibility: reversibility(descriptor.reversibility()),
        input_schema: schema(descriptor.input_schema()),
        output_schema: schema(descriptor.output_schema()),
        description: None,
        requires_approval: Some(requires_approval),
    }
}

/// Builds a publishable capability surface.
///
/// Refuses a surface larger than the contract allows rather than sending one the
/// cloud would reject.
pub fn publication<F>(
    version: &str,
    descriptors: &[CapabilityDescriptor],
    requires_approval: F,
) -> Result<CapabilitiesPublishPayload, MappingError>
where
    F: Fn(&CapabilityId) -> bool,
{
    if descriptors.len() > MAX_CAPABILITIES_PER_PUBLICATION {
        return Err(MappingError::bound(
            "capabilities",
            descriptors.len(),
            MAX_CAPABILITIES_PER_PUBLICATION,
        ));
    }

    Ok(CapabilitiesPublishPayload {
        version: version.to_string(),
        capabilities: descriptors
            .iter()
            .map(|d| descriptor(d, requires_approval(d.id())))
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use semver::Version;

    fn descriptor_for(id: &str, risk: RiskClass, rev: Reversibility) -> CapabilityDescriptor {
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
    fn risk_class_case_is_normalised_to_the_cloud_spelling() {
        let wire = serde_json::to_value(risk_class(RiskClass::LowRisk)).unwrap();
        assert_eq!(wire, "LowRisk");
        let local = serde_json::to_value(RiskClass::LowRisk).unwrap();
        assert_eq!(local, "low_risk");
        assert_ne!(wire, local, "the drift this function exists to fix");
    }

    #[test]
    fn every_risk_class_maps_to_its_pascal_case_twin() {
        for (local, expected) in [
            (RiskClass::Read, "Read"),
            (RiskClass::LowRisk, "LowRisk"),
            (RiskClass::Controlled, "Controlled"),
            (RiskClass::HighRisk, "HighRisk"),
            (RiskClass::Destructive, "Destructive"),
        ] {
            assert_eq!(serde_json::to_value(risk_class(local)).unwrap(), expected);
        }
    }

    #[test]
    fn none_reversibility_maps_to_irreversible_not_not_applicable() {
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
    fn partial_reversibility_maps_to_reversible() {
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
    fn descriptor_drops_version_and_derives_requires_approval() {
        let local = descriptor_for(
            "host.service.restart",
            RiskClass::LowRisk,
            Reversibility::Reversible,
        );
        let wire = descriptor(&local, true);
        let value = serde_json::to_value(&wire).unwrap();

        assert!(
            value.get("version").is_none(),
            "version is publication-level"
        );
        assert_eq!(value["requires_approval"], true);
        assert_eq!(value["risk_class"], "LowRisk");
        assert!(value.get("description").is_none(), "not invented locally");
    }

    #[test]
    fn empty_local_schemas_are_omitted_rather_than_sent_as_empty_objects() {
        let local = descriptor_for("argus.health.read", RiskClass::Read, Reversibility::None);
        let wire = descriptor(&local, false);
        assert!(wire.input_schema.is_none());
        assert!(wire.output_schema.is_some());
    }

    #[test]
    fn publication_refuses_more_capabilities_than_the_contract_allows() {
        let descriptors: Vec<CapabilityDescriptor> = (0..=MAX_CAPABILITIES_PER_PUBLICATION)
            .map(|i| {
                descriptor_for(
                    &format!("cap.item{i}"),
                    RiskClass::Read,
                    Reversibility::Reversible,
                )
            })
            .collect();
        let result = publication("0.1.0", &descriptors, |_| false);
        assert!(matches!(result, Err(MappingError::BoundExceeded { .. })));
    }

    #[test]
    fn publication_accepts_exactly_the_limit() {
        let descriptors: Vec<CapabilityDescriptor> = (0..MAX_CAPABILITIES_PER_PUBLICATION)
            .map(|i| {
                descriptor_for(
                    &format!("cap.item{i}"),
                    RiskClass::Read,
                    Reversibility::Reversible,
                )
            })
            .collect();
        let payload = publication("0.1.0", &descriptors, |_| false).expect("at the limit");
        assert_eq!(payload.capabilities.len(), MAX_CAPABILITIES_PER_PUBLICATION);
        assert_eq!(payload.version, "0.1.0");
    }

    #[test]
    fn an_empty_surface_still_publishes() {
        let payload = publication("0.1.0", &[], |_| false).expect("empty is valid");
        assert!(payload.capabilities.is_empty());
        let value = serde_json::to_value(&payload).unwrap();
        assert!(
            value["capabilities"].is_array(),
            "absence is explicit, not omitted"
        );
    }
}
