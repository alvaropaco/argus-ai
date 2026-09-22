//! Health vocabulary translation.
//!
//! The local vocabulary is `Ready | Degraded | NotReady`; the cloud's is
//! `healthy | degraded | critical | unknown`. Both are shipped contracts —
//! the local one is persisted and exposed over IPC, the cloud one is validated
//! by the cloud's schema — so the translation is permanent and explicit
//! (ADR-0016 §4).

use argus_domain::{HealthState, HealthStatus};

use crate::protocol::messages::{HealthReportPayload, HealthStateWire};

/// Translates a local health state into the cloud's vocabulary.
///
/// `NotReady` maps to `critical`, not `unknown`: the runtime cannot serve, and
/// `unknown` would understate a genuine failure. `unknown` is never produced
/// here — it is reserved for an instance whose health has never been observed.
pub fn health_state(state: HealthState) -> HealthStateWire {
    match state {
        HealthState::Ready => HealthStateWire::Healthy,
        HealthState::Degraded => HealthStateWire::Degraded,
        HealthState::NotReady => HealthStateWire::Critical,
    }
}

/// Builds a health report, carrying the local reason as the human-readable
/// summary so the cloud shows why an environment is degraded.
pub fn health_report(health: &HealthStatus) -> HealthReportPayload {
    HealthReportPayload {
        state: health_state(health.state()),
        summary: health
            .reason()
            .map(str::to_owned)
            .unwrap_or_else(|| human_summary(health.state()).to_string()),
        details: None,
        reported_at: health.checked_at(),
    }
}

fn human_summary(state: HealthState) -> &'static str {
    match state {
        HealthState::Ready => "the runtime is ready",
        HealthState::Degraded => "the runtime is degraded",
        HealthState::NotReady => "the runtime is not ready",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn ts() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 21, 12, 0, 0).unwrap()
    }

    #[test]
    fn ready_maps_to_healthy() {
        assert_eq!(health_state(HealthState::Ready), HealthStateWire::Healthy);
    }

    #[test]
    fn degraded_maps_to_degraded() {
        assert_eq!(
            health_state(HealthState::Degraded),
            HealthStateWire::Degraded
        );
    }

    #[test]
    fn not_ready_maps_to_critical_not_unknown() {
        assert_eq!(
            health_state(HealthState::NotReady),
            HealthStateWire::Critical
        );
        assert_ne!(
            health_state(HealthState::NotReady),
            HealthStateWire::Unknown
        );
    }

    #[test]
    fn unknown_is_never_produced_from_a_observed_state() {
        for state in [
            HealthState::Ready,
            HealthState::Degraded,
            HealthState::NotReady,
        ] {
            assert_ne!(health_state(state), HealthStateWire::Unknown, "{state:?}");
        }
    }

    #[test]
    fn degraded_reason_becomes_the_summary() {
        let health = HealthStatus::degraded("state store unavailable", ts());
        let report = health_report(&health);
        assert_eq!(report.state, HealthStateWire::Degraded);
        assert_eq!(report.summary, "state store unavailable");
        assert_eq!(report.reported_at, ts());
    }

    #[test]
    fn ready_without_a_reason_still_has_a_summary() {
        let report = health_report(&HealthStatus::ready(ts()));
        assert_eq!(report.state, HealthStateWire::Healthy);
        assert!(!report.summary.is_empty());
    }

    #[test]
    fn report_serialises_with_the_cloud_vocabulary() {
        let report = health_report(&HealthStatus::ready(ts()));
        let value = serde_json::to_value(&report).unwrap();
        assert_eq!(value["state"], "healthy");
    }
}
