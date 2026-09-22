//! `tracing` and OpenTelemetry instrumentation.
//!
//! Structured logs, metrics, and traces. Telemetry export is optional; local
//! structured logs continue when no exporter is configured (ADR-015).

use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::WithExportConfig;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;

/// Log output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogFormat {
    Text,
    Json,
}

/// Initializes the global `tracing` subscriber, optionally attaching an
/// OpenTelemetry OTLP exporter.
///
/// Uses `RUST_LOG` (or `argus=info` by default) for filtering. When
/// `otel_endpoint` is `Some`, traces are additionally exported over OTLP/HTTP.
/// Returns an error if a subscriber has already been initialized.
pub fn init(format: LogFormat, otel_endpoint: Option<&str>) -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("argus=info"));

    match otel_endpoint {
        Some(endpoint) => {
            let tracer = init_tracer(endpoint)?;
            let telemetry = tracing_opentelemetry::layer().with_tracer(tracer);
            // tracing-opentelemetry requires the default (non-JSON) fmt fields,
            // so text formatting is used whenever traces are exported.
            tracing::subscriber::set_global_default(
                tracing_subscriber::registry()
                    .with(filter)
                    .with(tracing_subscriber::fmt::layer())
                    .with(telemetry),
            )?;
        }
        None => match format {
            LogFormat::Text => tracing::subscriber::set_global_default(
                tracing_subscriber::registry()
                    .with(filter)
                    .with(tracing_subscriber::fmt::layer()),
            )?,
            LogFormat::Json => tracing::subscriber::set_global_default(
                tracing_subscriber::registry()
                    .with(filter)
                    .with(tracing_subscriber::fmt::layer().json()),
            )?,
        },
    }

    Ok(())
}

fn init_tracer(endpoint: &str) -> anyhow::Result<opentelemetry_sdk::trace::SdkTracer> {
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint(endpoint)
        .build()?;

    let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_simple_exporter(exporter)
        .build();

    let tracer = provider.tracer("argus");
    opentelemetry::global::set_tracer_provider(provider);
    Ok(tracer)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricSeverity {
    Info,
    Warning,
    Error,
}

/// A cheap point-in-time reading of the runtime's own counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MetricsSnapshot {
    pub uptime_seconds: u64,
    pub info: u64,
    pub warning: u64,
    pub error: u64,
}

/// The bounded, low-cardinality counters the runtime contributes to telemetry.
///
/// Deliberately not a general metrics registry. Cloud telemetry reports are
/// bounded and low-cardinality, so this tracks fixed counters rather than
/// arbitrary labels: a label-based registry would let a caller blow the reported
/// payload past its size bound (research R9).
pub struct RuntimeMetrics {
    started_at: Instant,
    info: AtomicU64,
    warning: AtomicU64,
    error: AtomicU64,
}

impl Default for RuntimeMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeMetrics {
    pub fn new() -> Self {
        Self {
            started_at: Instant::now(),
            info: AtomicU64::new(0),
            warning: AtomicU64::new(0),
            error: AtomicU64::new(0),
        }
    }

    pub fn uptime_seconds(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }

    /// Cheap enough to call on every event and infallible, so instrumentation
    /// can never become a failure path.
    pub fn record(&self, severity: MetricSeverity) {
        let counter = match severity {
            MetricSeverity::Info => &self.info,
            MetricSeverity::Warning => &self.warning,
            MetricSeverity::Error => &self.error,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            uptime_seconds: self.uptime_seconds(),
            info: self.info.load(Ordering::Relaxed),
            warning: self.warning.load(Ordering::Relaxed),
            error: self.error.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_registry_reports_no_activity() {
        let snapshot = RuntimeMetrics::new().snapshot();
        assert_eq!(snapshot.info, 0);
        assert_eq!(snapshot.warning, 0);
        assert_eq!(snapshot.error, 0);
        assert_eq!(snapshot.uptime_seconds, 0);
    }

    #[test]
    fn each_severity_has_its_own_counter() {
        let metrics = RuntimeMetrics::new();
        metrics.record(MetricSeverity::Info);
        metrics.record(MetricSeverity::Info);
        metrics.record(MetricSeverity::Warning);
        metrics.record(MetricSeverity::Error);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.info, 2);
        assert_eq!(snapshot.warning, 1);
        assert_eq!(snapshot.error, 1);
    }

    #[test]
    fn recording_never_panics_and_is_cheap_to_repeat() {
        let metrics = RuntimeMetrics::new();
        for _ in 0..1000 {
            metrics.record(MetricSeverity::Info);
        }
        assert_eq!(metrics.snapshot().info, 1000);
    }

    #[test]
    fn a_snapshot_is_a_value_not_a_live_view() {
        let metrics = RuntimeMetrics::new();
        metrics.record(MetricSeverity::Error);
        let first = metrics.snapshot();

        metrics.record(MetricSeverity::Error);
        let second = metrics.snapshot();

        assert_eq!(first.error, 1, "an earlier reading must not change");
        assert_eq!(second.error, 2);
    }

    #[test]
    fn uptime_is_monotonic() {
        let metrics = RuntimeMetrics::new();
        let first = metrics.uptime_seconds();
        let second = metrics.uptime_seconds();
        assert!(second >= first);
    }
}
