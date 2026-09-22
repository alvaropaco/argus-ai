//! Event projection onto the cloud's event vocabulary.
//!
//! The deduplication key is `(instance_id, source_event_id)`, so
//! `source_event_id` is the local `DomainEvent` UUID, unchanged across retries.
//! Using anything else would make a retry after reconnection produce a duplicate.
//!
//! FR-017 fixes the fields that must survive the projection: type, severity,
//! subject, correlation, causation, and time. The local `source` field is not in
//! that list and has no cloud counterpart, so it is deliberately not transmitted.

use argus_domain::{DomainEvent, Severity};

use crate::protocol::messages::{
    EventsReportPayload, MAX_EVENTS_PER_REPORT, ReportedEvent, SeverityWire,
};

/// Translates the local severity, which is already aligned with the cloud's.
pub fn severity(severity: Severity) -> SeverityWire {
    match severity {
        Severity::Info => SeverityWire::Info,
        Severity::Warning => SeverityWire::Warning,
        Severity::Error => SeverityWire::Error,
    }
}

/// Projects one local event onto the wire shape.
pub fn event(event: &DomainEvent) -> ReportedEvent {
    ReportedEvent {
        source_event_id: event.id().to_string(),
        event_type: event.event_type().as_str().to_string(),
        severity: severity(event.severity()),
        subject: Some(event.subject().to_string()).filter(|s| !s.is_empty()),
        correlation_id: event.correlation_id().map(|id| id.to_string()),
        causation_id: event.causation_id().map(|id| id.to_string()),
        reported_at: event.timestamp(),
        payload: match event.payload() {
            serde_json::Value::Object(map) if !map.is_empty() => Some(map.clone()),
            _ => None,
        },
    }
}

/// Splits events into report batches that respect the contract's item bound.
///
/// Splitting rather than truncating keeps FR-019 satisfied without losing
/// events; an empty input yields no batches, because the payload requires at
/// least one event.
pub fn batches(events: &[DomainEvent]) -> Vec<EventsReportPayload> {
    events
        .chunks(MAX_EVENTS_PER_REPORT)
        .map(|chunk| EventsReportPayload {
            events: chunk.iter().map(event).collect(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use argus_domain::EventType;
    use chrono::{TimeZone, Utc};
    use uuid::Uuid;

    fn ts() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 21, 12, 0, 0).unwrap()
    }

    fn domain_event(id: Uuid, severity: Severity) -> DomainEvent {
        DomainEvent::new(
            id,
            EventType::new("argus.ready").unwrap(),
            ts(),
            "argusd",
            "host:web-01",
            severity,
            Some(Uuid::nil()),
            None,
            serde_json::json!({"version": "0.1.7"}),
        )
    }

    #[test]
    fn source_event_id_is_the_local_event_uuid() {
        let id = Uuid::new_v4();
        let projected = event(&domain_event(id, Severity::Info));
        assert_eq!(projected.source_event_id, id.to_string());
    }

    #[test]
    fn the_dedup_key_is_stable_across_a_retry() {
        let source = domain_event(Uuid::new_v4(), Severity::Info);
        let first = event(&source);
        let retry = event(&source);
        assert_eq!(first.source_event_id, retry.source_event_id);
        assert_eq!(first, retry);
    }

    #[test]
    fn severity_is_aligned_with_the_cloud() {
        assert_eq!(severity(Severity::Info), SeverityWire::Info);
        assert_eq!(severity(Severity::Warning), SeverityWire::Warning);
        assert_eq!(severity(Severity::Error), SeverityWire::Error);
    }

    #[test]
    fn type_severity_subject_and_correlation_survive_projection() {
        let projected = event(&domain_event(Uuid::new_v4(), Severity::Warning));
        assert_eq!(projected.event_type, "argus.ready");
        assert_eq!(projected.severity, SeverityWire::Warning);
        assert_eq!(projected.subject.as_deref(), Some("host:web-01"));
        let nil = Uuid::nil().to_string();
        assert_eq!(projected.correlation_id.as_deref(), Some(nil.as_str()));
        assert_eq!(projected.reported_at, ts());
        assert!(projected.payload.is_some());
    }

    #[test]
    fn empty_payload_is_omitted_rather_than_sent_as_an_empty_object() {
        let bare = DomainEvent::new(
            Uuid::new_v4(),
            EventType::new("argus.started").unwrap(),
            ts(),
            "argusd",
            "argusd",
            Severity::Info,
            None,
            None,
            serde_json::json!({}),
        );
        assert!(event(&bare).payload.is_none());
    }

    #[test]
    fn batching_respects_the_item_bound() {
        let events: Vec<DomainEvent> = (0..(MAX_EVENTS_PER_REPORT * 2 + 7))
            .map(|_| domain_event(Uuid::new_v4(), Severity::Info))
            .collect();
        let batches = batches(&events);

        assert_eq!(batches.len(), 3);
        assert_eq!(batches[0].events.len(), MAX_EVENTS_PER_REPORT);
        assert_eq!(batches[1].events.len(), MAX_EVENTS_PER_REPORT);
        assert_eq!(batches[2].events.len(), 7);
        let total: usize = batches.iter().map(|b| b.events.len()).sum();
        assert_eq!(total, events.len(), "no event is lost to batching");
    }

    #[test]
    fn an_exactly_full_batch_produces_one_payload() {
        let events: Vec<DomainEvent> = (0..MAX_EVENTS_PER_REPORT)
            .map(|_| domain_event(Uuid::new_v4(), Severity::Info))
            .collect();
        assert_eq!(batches(&events).len(), 1);
    }

    #[test]
    fn an_empty_input_produces_no_batches() {
        assert!(batches(&[]).is_empty());
    }
}
