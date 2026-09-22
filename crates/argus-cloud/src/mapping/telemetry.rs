//! Telemetry projection.
//!
//! Only signals the runtime actually produces are reported. The bootstrap does
//! not collect host CPU, memory, or load, so inventing them here would ship a
//! metric that silently reports a placeholder — worse than reporting nothing
//! (research R9).
//!
//! The payload is bounded by the contract's byte limit, and the caller is told
//! when a payload would exceed it rather than letting the cloud reject it
//! (FR-019).

use argus_domain::CloudConnectivityState;
use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

use crate::error::MappingError;
use crate::protocol::messages::{HealthStateWire, MAX_TELEMETRY_BYTES, TelemetryReportPayload};

/// Errors and warnings seen since the last report.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EventCounts {
    pub info: u64,
    pub warning: u64,
    pub error: u64,
}

/// The signals the installation can actually vouch for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelemetrySource {
    pub health_state: HealthStateWire,
    pub uptime_seconds: u64,
    pub event_counts: EventCounts,
    pub capability_count: usize,
    pub plugin_count: usize,
    pub connectivity: CloudConnectivityState,
    /// Reports still waiting to be delivered, and how many were lost.
    pub buffered_reports: usize,
    pub dropped_reports: u64,
}

impl Default for TelemetrySource {
    fn default() -> Self {
        Self {
            health_state: HealthStateWire::Unknown,
            uptime_seconds: 0,
            event_counts: EventCounts::default(),
            capability_count: 0,
            plugin_count: 0,
            connectivity: CloudConnectivityState::NotConfigured,
            buffered_reports: 0,
            dropped_reports: 0,
        }
    }
}

/// Projects the source into a bounded telemetry payload.
///
/// Fails rather than sending an oversized payload the cloud would reject.
pub fn telemetry_report(
    source: &TelemetrySource,
    reported_at: DateTime<Utc>,
) -> Result<TelemetryReportPayload, MappingError> {
    let mut metrics = Map::new();
    metrics.insert(
        "health".into(),
        Value::String(health_label(source.health_state).into()),
    );
    metrics.insert("uptime_seconds".into(), Value::from(source.uptime_seconds));
    metrics.insert("events_info".into(), Value::from(source.event_counts.info));
    metrics.insert(
        "events_warning".into(),
        Value::from(source.event_counts.warning),
    );
    metrics.insert(
        "events_error".into(),
        Value::from(source.event_counts.error),
    );
    metrics.insert("capabilities".into(), Value::from(source.capability_count));
    metrics.insert("plugins".into(), Value::from(source.plugin_count));
    metrics.insert(
        "connectivity".into(),
        Value::String(connectivity_label(source.connectivity).into()),
    );
    metrics.insert(
        "buffered_reports".into(),
        Value::from(source.buffered_reports),
    );
    metrics.insert(
        "dropped_reports".into(),
        Value::from(source.dropped_reports),
    );

    let payload = TelemetryReportPayload {
        metrics,
        reported_at,
    };

    let encoded = serde_json::to_vec(&payload)
        .map_err(|error| MappingError::encoding("telemetry", error.to_string()))?;

    if encoded.len() > MAX_TELEMETRY_BYTES {
        return Err(MappingError::bound(
            "telemetry",
            encoded.len(),
            MAX_TELEMETRY_BYTES,
        ));
    }

    Ok(payload)
}

fn health_label(state: HealthStateWire) -> &'static str {
    match state {
        HealthStateWire::Healthy => "healthy",
        HealthStateWire::Degraded => "degraded",
        HealthStateWire::Critical => "critical",
        HealthStateWire::Unknown => "unknown",
    }
}

fn connectivity_label(state: CloudConnectivityState) -> &'static str {
    match state {
        CloudConnectivityState::NotConfigured => "not_configured",
        CloudConnectivityState::Connecting => "connecting",
        CloudConnectivityState::Connected => "connected",
        CloudConnectivityState::Degraded => "degraded",
        CloudConnectivityState::Disconnected => "disconnected",
        CloudConnectivityState::Suspended => "suspended",
        CloudConnectivityState::Revoked => "revoked",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 21, 12, 0, 0).unwrap()
    }

    fn source() -> TelemetrySource {
        TelemetrySource {
            health_state: HealthStateWire::Healthy,
            uptime_seconds: 3600,
            event_counts: EventCounts {
                info: 12,
                warning: 3,
                error: 1,
            },
            capability_count: 7,
            plugin_count: 2,
            connectivity: CloudConnectivityState::Connected,
            buffered_reports: 5,
            dropped_reports: 0,
        }
    }

    #[test]
    fn the_report_carries_the_signals_the_runtime_has() {
        let payload = telemetry_report(&source(), at()).expect("within bounds");
        let metrics = &payload.metrics;

        assert_eq!(metrics["health"], "healthy");
        assert_eq!(metrics["uptime_seconds"], 3600);
        assert_eq!(metrics["capabilities"], 7);
        assert_eq!(metrics["plugins"], 2);
        assert_eq!(metrics["connectivity"], "connected");
        assert_eq!(payload.reported_at, at());
    }

    #[test]
    fn event_counts_are_reported_per_severity() {
        let payload = telemetry_report(&source(), at()).unwrap();
        assert_eq!(payload.metrics["events_info"], 12);
        assert_eq!(payload.metrics["events_warning"], 3);
        assert_eq!(payload.metrics["events_error"], 1);
    }

    #[test]
    fn the_drop_count_is_surfaced_so_loss_is_not_silent() {
        let mut source = source();
        source.dropped_reports = 44;
        let payload = telemetry_report(&source, at()).unwrap();
        assert_eq!(payload.metrics["dropped_reports"], 44);
    }

    #[test]
    fn no_invented_host_metrics_are_reported() {
        // The bootstrap collects no host CPU, memory, or load; reporting a
        // placeholder would be worse than reporting nothing.
        let payload = telemetry_report(&source(), at()).unwrap();
        for invented in ["cpu", "memory", "load", "disk", "host_status"] {
            assert!(
                payload.metrics.get(invented).is_none(),
                "`{invented}` is not collected by this runtime"
            );
        }
    }

    #[test]
    fn the_payload_stays_within_the_contract_bound() {
        let payload = telemetry_report(&source(), at()).unwrap();
        let encoded = serde_json::to_vec(&payload).unwrap();
        assert!(
            encoded.len() <= MAX_TELEMETRY_BYTES,
            "a bounded payload must fit: {} bytes",
            encoded.len()
        );
    }

    #[test]
    fn every_connectivity_state_has_a_label() {
        for state in [
            CloudConnectivityState::NotConfigured,
            CloudConnectivityState::Connecting,
            CloudConnectivityState::Connected,
            CloudConnectivityState::Degraded,
            CloudConnectivityState::Disconnected,
            CloudConnectivityState::Suspended,
            CloudConnectivityState::Revoked,
        ] {
            let mut source = source();
            source.connectivity = state;
            let payload = telemetry_report(&source, at()).unwrap();
            let label = payload.metrics["connectivity"].as_str().unwrap();
            assert!(!label.is_empty(), "{state:?}");
        }
    }

    #[test]
    fn the_labels_are_the_serialised_domain_spellings() {
        // The label must match what the connectivity state serialises to, or the
        // cloud and the report would disagree about the same condition.
        for state in [
            CloudConnectivityState::Connected,
            CloudConnectivityState::Revoked,
            CloudConnectivityState::NotConfigured,
        ] {
            let serialised = serde_json::to_value(state).unwrap();
            assert_eq!(connectivity_label(state), serialised.as_str().unwrap());
        }
    }

    #[test]
    fn a_default_source_still_produces_a_valid_report() {
        // A fresh installation reports honestly as unknown/not-configured rather
        // than fabricating health.
        let payload = telemetry_report(&TelemetrySource::default(), at()).unwrap();
        assert_eq!(payload.metrics["health"], "unknown");
        assert_eq!(payload.metrics["connectivity"], "not_configured");
    }
}
