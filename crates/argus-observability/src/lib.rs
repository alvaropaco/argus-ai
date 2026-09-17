//! `tracing` and OpenTelemetry instrumentation.
//!
//! Structured logs, metrics, and traces. Telemetry export is optional; local
//! structured logs continue when no exporter is configured.

use serde::{Deserialize, Serialize};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Log output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogFormat {
    Text,
    Json,
}

/// Initializes the global `tracing` subscriber.
///
/// Uses `RUST_LOG` (or `argus=info` by default) for filtering. Returns an
/// error if a subscriber has already been initialized (e.g. called twice).
pub fn init(format: LogFormat) -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("argus=info"));

    let registry = tracing_subscriber::registry().with(filter);

    match format {
        LogFormat::Text => registry.with(tracing_subscriber::fmt::layer()).try_init()?,
        LogFormat::Json => registry
            .with(tracing_subscriber::fmt::layer().json())
            .try_init()?,
    }

    Ok(())
}
