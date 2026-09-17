//! Versioned capability contracts.

use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::CapabilityId;

/// Risk classification of a capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskClass {
    Read,
    LowRisk,
    Controlled,
    HighRisk,
    Destructive,
}

/// Whether an action can be undone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reversibility {
    None,
    Reversible,
    PartiallyReversible,
}

/// The declarative, versioned description of a capability.
///
/// The descriptor is metadata, not an authorization grant: the policy engine
/// remains authoritative (see ADR-016).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)] // schemas are serde_json::Value
pub struct CapabilityDescriptor {
    id: CapabilityId,
    provider: String,
    operation: String,
    risk_class: RiskClass,
    version: Version,
    input_schema: Value,
    output_schema: Value,
    reversibility: Reversibility,
}

impl CapabilityDescriptor {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: CapabilityId,
        provider: impl Into<String>,
        operation: impl Into<String>,
        risk_class: RiskClass,
        version: Version,
        input_schema: Value,
        output_schema: Value,
        reversibility: Reversibility,
    ) -> Self {
        Self {
            id,
            provider: provider.into(),
            operation: operation.into(),
            risk_class,
            version,
            input_schema,
            output_schema,
            reversibility,
        }
    }

    pub fn id(&self) -> &CapabilityId {
        &self.id
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }

    pub fn operation(&self) -> &str {
        &self.operation
    }

    pub fn risk_class(&self) -> RiskClass {
        self.risk_class
    }

    pub fn version(&self) -> &Version {
        &self.version
    }

    pub fn input_schema(&self) -> &Value {
        &self.input_schema
    }

    pub fn output_schema(&self) -> &Value {
        &self.output_schema
    }

    pub fn reversibility(&self) -> Reversibility {
        self.reversibility
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn risk_class_serde_is_snake_case() {
        assert_eq!(
            serde_json::to_string(&RiskClass::LowRisk).unwrap(),
            "\"low_risk\""
        );
    }

    #[test]
    fn descriptor_serde_round_trip() {
        let id = CapabilityId::new("argus.health.read").unwrap();
        let desc = CapabilityDescriptor::new(
            id.clone(),
            "argusd",
            "health.get",
            RiskClass::Read,
            Version::new(0, 1, 0),
            serde_json::json!({ "type": "object", "additionalProperties": false }),
            serde_json::json!({ "type": "object" }),
            Reversibility::None,
        );
        let json = serde_json::to_string(&desc).unwrap();
        let back: CapabilityDescriptor = serde_json::from_str(&json).unwrap();
        assert_eq!(desc, back);
        assert_eq!(back.id(), &id);
        assert_eq!(back.version(), &Version::new(0, 1, 0));
    }
}
