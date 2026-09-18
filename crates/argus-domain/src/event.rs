//! Domain events: transport-independent operational events.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::error::DomainError;
use crate::validate::is_dotted_path;

/// A namespaced, dotted event type, e.g. `argus.ready`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EventType(String);

impl EventType {
    pub fn new(path: &str) -> Result<Self, DomainError> {
        if !is_dotted_path(path) {
            return Err(DomainError::InvalidEventType(path.to_string()));
        }
        Ok(Self(path.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Event severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

/// An immutable, transport-independent domain event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)] // payload is serde_json::Value
pub struct DomainEvent {
    id: Uuid,
    event_type: EventType,
    timestamp: DateTime<Utc>,
    source: String,
    subject: String,
    severity: Severity,
    correlation_id: Option<Uuid>,
    causation_id: Option<Uuid>,
    payload: Value,
}

impl DomainEvent {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: Uuid,
        event_type: EventType,
        timestamp: DateTime<Utc>,
        source: impl Into<String>,
        subject: impl Into<String>,
        severity: Severity,
        correlation_id: Option<Uuid>,
        causation_id: Option<Uuid>,
        payload: Value,
    ) -> Self {
        Self {
            id,
            event_type,
            timestamp,
            source: source.into(),
            subject: subject.into(),
            severity,
            correlation_id,
            causation_id,
            payload,
        }
    }

    pub fn id(&self) -> Uuid {
        self.id
    }

    pub fn event_type(&self) -> &EventType {
        &self.event_type
    }

    pub fn timestamp(&self) -> DateTime<Utc> {
        self.timestamp
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn subject(&self) -> &str {
        &self.subject
    }

    pub fn severity(&self) -> Severity {
        self.severity
    }

    pub fn correlation_id(&self) -> Option<Uuid> {
        self.correlation_id
    }

    pub fn causation_id(&self) -> Option<Uuid> {
        self.causation_id
    }

    pub fn payload(&self) -> &Value {
        &self.payload
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn ts() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 16, 12, 0, 0).unwrap()
    }

    #[test]
    fn event_type_valid_and_invalid() {
        assert!(EventType::new("argus.ready").is_ok());
        assert!(EventType::new("plugin.loaded").is_ok());
        assert!(EventType::new("argus").is_err());
        assert!(EventType::new("Argus.ready").is_err());
    }

    #[test]
    fn serde_round_trip_with_correlation() {
        let correlation = Uuid::new_v4();
        let event = DomainEvent::new(
            Uuid::new_v4(),
            EventType::new("argus.ready").unwrap(),
            ts(),
            "argusd",
            "argusd",
            Severity::Info,
            Some(correlation),
            None,
            serde_json::json!({ "version": "0.1.0" }),
        );
        let json = serde_json::to_string(&event).unwrap();
        let back: DomainEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, back);
        assert_eq!(back.correlation_id(), Some(correlation));
    }
}
