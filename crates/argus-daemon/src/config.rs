//! Daemon configuration.

use argus_observability::LogFormat;
use serde::{Deserialize, Serialize};

/// Resolved configuration for a single `argusd` instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonConfig {
    /// Path to the Unix-domain socket.
    pub socket_path: String,
    /// Path to the SQLite state database.
    pub state_path: String,
    /// Human-readable environment name.
    pub environment_name: String,
    /// Optional allowlist of peer UIDs; `None` allows any connected peer
    /// (read-only bootstrap operations only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorized_uids: Option<Vec<u32>>,
    /// Optional OpenTelemetry OTLP endpoint; `None` disables trace export.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub otel_endpoint: Option<String>,
    /// Log output format.
    pub log_format: LogFormat,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            socket_path: "/run/argus/argusd.sock".to_string(),
            state_path: "/var/lib/argus/argus.db".to_string(),
            environment_name: "default".to_string(),
            authorized_uids: None,
            otel_endpoint: None,
            log_format: LogFormat::Text,
        }
    }
}
