//! `tracing` and OpenTelemetry instrumentation.
//!
//! Structured logs, metrics, and traces. Telemetry export is optional; local
//! structured logs continue when no exporter is configured (ADR-015).

use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::WithExportConfig;
use serde::{Deserialize, Serialize};
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
