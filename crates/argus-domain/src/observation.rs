//! Observations: immutable evidence collected from the environment.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::ResourceId;
use crate::error::DomainError;

/// A typed observed value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(clippy::derive_partial_eq_without_eq)] // contains f64
pub enum ObservedValue {
    Text(String),
    Number(f64),
    Bool(bool),
}

/// Provenance metadata describing how an observation was collected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    source: String,
    method: String,
    collected_at: DateTime<Utc>,
}

impl Provenance {
    pub fn new(
        source: impl Into<String>,
        method: impl Into<String>,
        collected_at: DateTime<Utc>,
    ) -> Self {
        Self {
            source: source.into(),
            method: method.into(),
            collected_at,
        }
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn method(&self) -> &str {
        &self.method
    }

    pub fn collected_at(&self) -> DateTime<Utc> {
        self.collected_at
    }
}

/// An immutable piece of evidence collected from the environment.
///
/// Observations must never be overwritten by inferred state (Principle 4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)] // confidence is f32
pub struct Observation {
    id: Uuid,
    source: String,
    subject: ResourceId,
    attribute: String,
    value: ObservedValue,
    confidence: f32,
    provenance: Provenance,
    observed_at: DateTime<Utc>,
}

impl Observation {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: Uuid,
        source: impl Into<String>,
        subject: ResourceId,
        attribute: impl Into<String>,
        value: ObservedValue,
        confidence: f32,
        provenance: Provenance,
        observed_at: DateTime<Utc>,
    ) -> Result<Self, DomainError> {
        if !(0.0..=1.0).contains(&confidence) {
            return Err(DomainError::ConfidenceOutOfRange(confidence));
        }
        Ok(Self {
            id,
            source: source.into(),
            subject,
            attribute: attribute.into(),
            value,
            confidence,
            provenance,
            observed_at,
        })
    }

    pub fn id(&self) -> Uuid {
        self.id
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn subject(&self) -> &ResourceId {
        &self.subject
    }

    pub fn attribute(&self) -> &str {
        &self.attribute
    }

    pub fn value(&self) -> &ObservedValue {
        &self.value
    }

    pub fn confidence(&self) -> f32 {
        self.confidence
    }

    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }

    pub fn observed_at(&self) -> DateTime<Utc> {
        self.observed_at
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn ts() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 16, 12, 0, 0).unwrap()
    }

    fn observation(confidence: f32) -> Result<Observation, DomainError> {
        let subject = ResourceId::new("host", "abc123").unwrap();
        Observation::new(
            Uuid::new_v4(),
            "argusd",
            subject,
            "memory.pressure",
            ObservedValue::Number(12.5),
            confidence,
            Provenance::new("procfs", "read_pressure", ts()),
            ts(),
        )
    }

    #[test]
    fn confidence_bounds_are_accepted() {
        assert!(observation(0.0).is_ok());
        assert!(observation(0.5).is_ok());
        assert!(observation(1.0).is_ok());
    }

    #[test]
    fn confidence_out_of_range_is_rejected() {
        assert_eq!(
            observation(-0.1),
            Err(DomainError::ConfidenceOutOfRange(-0.1))
        );
        assert_eq!(
            observation(1.1),
            Err(DomainError::ConfidenceOutOfRange(1.1))
        );
    }

    #[test]
    fn serde_round_trip() {
        let obs = observation(0.99).unwrap();
        let json = serde_json::to_string(&obs).unwrap();
        let back: Observation = serde_json::from_str(&json).unwrap();
        assert_eq!(obs, back);
    }
}
