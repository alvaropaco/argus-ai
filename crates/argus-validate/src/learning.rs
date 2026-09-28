//! The separate, read-only learning pass (ADR-0031 §5).

use argus_domain::{DomainEvent, Observation};
use serde::Serialize;

/// The published validation event types (`contracts/events.md`).
///
/// Duplicated as literals so this crate stays free of the async event-bus
/// dependency; the strings are the transport-independent contract.
const VALIDATION_PASSED: &str = "validation.passed";
const VALIDATION_FAILED: &str = "validation.failed";

/// A deterministic summary of the persisted evidence the learning pass read.
///
/// v1 delivers the separation boundary only: a report, never a mutation
/// (outcome-based skill re-scoring remains deferred, SPEC.md:127).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct LearningReport {
    /// The number of persisted observations considered.
    pub observation_count: usize,
    /// Validation events observed as passed.
    pub validation_passed: usize,
    /// Validation events observed as failed.
    pub validation_failed: usize,
}

/// Runs the read-only learning pass over persisted evidence.
///
/// The pass receives data — never a plan handle — so it cannot mutate an
/// in-flight or already-authorized plan. It is deterministic: the same inputs
/// always produce the same report.
pub fn run(observations: &[Observation], audit_events: &[DomainEvent]) -> LearningReport {
    LearningReport {
        observation_count: observations.len(),
        validation_passed: audit_events
            .iter()
            .filter(|event| event.event_type().as_str() == VALIDATION_PASSED)
            .count(),
        validation_failed: audit_events
            .iter()
            .filter(|event| event.event_type().as_str() == VALIDATION_FAILED)
            .count(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use argus_domain::{EventType, ObservedValue, Provenance, ResourceId, Severity};
    use chrono::Utc;

    fn observation() -> Observation {
        let subject = ResourceId::new("host", "abc").unwrap();
        Observation::new(
            uuid::Uuid::new_v4(),
            "argusd",
            subject,
            "service.active",
            ObservedValue::Bool(true),
            1.0,
            Provenance::new("systemd", "is_active", Utc::now()),
            Utc::now(),
        )
        .unwrap()
    }

    fn event(event_type: &str) -> DomainEvent {
        DomainEvent::new(
            uuid::Uuid::new_v4(),
            EventType::new(event_type).unwrap(),
            Utc::now(),
            "argusd",
            "argusd",
            Severity::Info,
            None,
            None,
            serde_json::json!({}),
        )
    }

    #[test]
    fn run_counts_validation_events_deterministically() {
        let observations = vec![observation(), observation()];
        let events = vec![
            event("validation.passed"),
            event("validation.failed"),
            event("plan.proposed"),
        ];

        let report = run(&observations, &events);
        assert_eq!(report.observation_count, 2);
        assert_eq!(report.validation_passed, 1);
        assert_eq!(report.validation_failed, 1);

        // Deterministic: the same inputs always produce the same report.
        assert_eq!(report, run(&observations, &events));
    }

    #[test]
    fn run_reports_zero_on_empty_evidence() {
        assert_eq!(run(&[], &[]), LearningReport::default());
    }
}
