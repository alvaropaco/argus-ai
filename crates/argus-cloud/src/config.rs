//! Cloud settings as a plain value type.
//!
//! This module defines *what* the cloud settings are. Parsing `argus.toml`,
//! validating unknown keys, and applying precedence (`defaults < file < cloud <
//! flag`) live in `argus-daemon`, which owns configuration loading. Secrets are
//! never represented here — they live in the secret store
//! (`specs/001-argus-cloud-sync/contracts/cloud-config.md`).

use serde::{Deserialize, Serialize};

/// Non-secret cloud settings.
///
/// Unknown keys are rejected rather than ignored, so a typo cannot silently
/// disable a safety setting such as `allow_privileged_execution`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloudConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default = "default_expected_cloud_id")]
    pub expected_cloud_id: String,
    #[serde(default = "default_true")]
    pub allow_privileged_execution: bool,
    #[serde(default = "default_telemetry_interval")]
    pub telemetry_interval_seconds: u64,
    #[serde(default = "default_buffer_max")]
    pub report_buffer_max_records: usize,
    #[serde(default = "default_timeout")]
    pub request_timeout_seconds: u64,
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent_privileged: u32,
    #[serde(default = "default_reconnect_base")]
    pub reconnect_base_seconds: u64,
    #[serde(default = "default_reconnect_max")]
    pub reconnect_max_seconds: u64,
}

fn default_expected_cloud_id() -> String {
    "argus-cloud".to_string()
}

fn default_true() -> bool {
    true
}

fn default_telemetry_interval() -> u64 {
    60
}

fn default_buffer_max() -> usize {
    5000
}

fn default_timeout() -> u64 {
    120
}

fn default_max_concurrent() -> u32 {
    2
}

fn default_reconnect_base() -> u64 {
    1
}

fn default_reconnect_max() -> u64 {
    300
}

impl Default for CloudConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: None,
            expected_cloud_id: default_expected_cloud_id(),
            allow_privileged_execution: default_true(),
            telemetry_interval_seconds: default_telemetry_interval(),
            report_buffer_max_records: default_buffer_max(),
            request_timeout_seconds: default_timeout(),
            max_concurrent_privileged: default_max_concurrent(),
            reconnect_base_seconds: default_reconnect_base(),
            reconnect_max_seconds: default_reconnect_max(),
        }
    }
}

/// Why a cloud configuration was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CloudConfigError {
    #[error("cloud is enabled but no endpoint is configured")]
    MissingEndpoint,

    #[error("{field} must be at least {min}, got {actual}")]
    BelowMinimum {
        field: &'static str,
        min: u64,
        actual: u64,
    },

    #[error("reconnect_max_seconds ({max}) must be at least reconnect_base_seconds ({base})")]
    ReconnectRangeInverted { base: u64, max: u64 },

    #[error("the endpoint must be a secure WebSocket URL, got '{0}'")]
    InsecureEndpoint(String),
}

impl CloudConfig {
    /// Whether the installation should attempt to connect at all.
    pub fn is_active(&self) -> bool {
        self.enabled && self.endpoint.is_some()
    }

    pub fn permits_privileged_execution(&self) -> bool {
        self.allow_privileged_execution
    }

    /// Validates the settings, rejecting anything that would produce a
    /// misleadingly permissive or non-functional configuration.
    pub fn validate(&self) -> Result<(), CloudConfigError> {
        if self.enabled {
            let Some(endpoint) = self.endpoint.as_deref() else {
                return Err(CloudConfigError::MissingEndpoint);
            };
            if !endpoint.starts_with("wss://") && !endpoint.starts_with("ws://") {
                return Err(CloudConfigError::InsecureEndpoint(endpoint.to_string()));
            }
        }

        check_min(
            "telemetry_interval_seconds",
            self.telemetry_interval_seconds,
            10,
        )?;
        check_min(
            "report_buffer_max_records",
            self.report_buffer_max_records as u64,
            100,
        )?;
        check_min("request_timeout_seconds", self.request_timeout_seconds, 1)?;
        check_min(
            "max_concurrent_privileged",
            u64::from(self.max_concurrent_privileged),
            1,
        )?;
        check_min("reconnect_base_seconds", self.reconnect_base_seconds, 1)?;

        if self.reconnect_max_seconds < self.reconnect_base_seconds {
            return Err(CloudConfigError::ReconnectRangeInverted {
                base: self.reconnect_base_seconds,
                max: self.reconnect_max_seconds,
            });
        }

        Ok(())
    }
}

fn check_min(field: &'static str, actual: u64, min: u64) -> Result<(), CloudConfigError> {
    if actual < min {
        return Err(CloudConfigError::BelowMinimum { field, min, actual });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_leave_the_cloud_disabled() {
        let config = CloudConfig::default();
        assert!(!config.enabled);
        assert!(!config.is_active());
        assert_eq!(config.endpoint, None);
        config.validate().expect("an inactive config is valid");
    }

    #[test]
    fn defaults_match_the_documented_values() {
        let config = CloudConfig::default();
        assert_eq!(config.expected_cloud_id, "argus-cloud");
        assert!(config.allow_privileged_execution);
        assert_eq!(config.telemetry_interval_seconds, 60);
        assert_eq!(config.report_buffer_max_records, 5000);
        assert_eq!(config.request_timeout_seconds, 120);
        assert_eq!(config.max_concurrent_privileged, 2);
        assert_eq!(config.reconnect_base_seconds, 1);
        assert_eq!(config.reconnect_max_seconds, 300);
    }

    #[test]
    fn an_empty_table_yields_the_defaults() {
        let config: CloudConfig = toml::from_str("").expect("empty table is valid");
        assert_eq!(config, CloudConfig::default());
    }

    #[test]
    fn unknown_keys_are_rejected_so_a_typo_cannot_disable_a_safety_setting() {
        let result: Result<CloudConfig, _> = toml::from_str("allow_privleged_execution = false\n");
        assert!(
            result.is_err(),
            "a misspelled safety setting must not be ignored"
        );
    }

    #[test]
    fn enabling_without_an_endpoint_is_rejected() {
        let config = CloudConfig {
            enabled: true,
            endpoint: None,
            ..CloudConfig::default()
        };
        assert_eq!(config.validate(), Err(CloudConfigError::MissingEndpoint));
    }

    #[test]
    fn a_valid_endpoint_makes_the_config_active() {
        let config = CloudConfig {
            enabled: true,
            endpoint: Some("wss://cloud.example.com/agent".into()),
            ..CloudConfig::default()
        };
        assert!(config.is_active());
        config.validate().expect("valid");
    }

    #[test]
    fn a_non_websocket_endpoint_is_rejected() {
        let config = CloudConfig {
            enabled: true,
            endpoint: Some("https://cloud.example.com/agent".into()),
            ..CloudConfig::default()
        };
        assert!(matches!(
            config.validate(),
            Err(CloudConfigError::InsecureEndpoint(_))
        ));
    }

    #[test]
    fn every_lower_bound_is_enforced() {
        let cases: Vec<(CloudConfig, &str)> = vec![
            (
                CloudConfig {
                    telemetry_interval_seconds: 9,
                    ..CloudConfig::default()
                },
                "telemetry_interval_seconds",
            ),
            (
                CloudConfig {
                    report_buffer_max_records: 99,
                    ..CloudConfig::default()
                },
                "report_buffer_max_records",
            ),
            (
                CloudConfig {
                    request_timeout_seconds: 0,
                    ..CloudConfig::default()
                },
                "request_timeout_seconds",
            ),
            (
                CloudConfig {
                    max_concurrent_privileged: 0,
                    ..CloudConfig::default()
                },
                "max_concurrent_privileged",
            ),
        ];
        for (config, field) in cases {
            match config.validate() {
                Err(CloudConfigError::BelowMinimum { field: f, .. }) => {
                    assert_eq!(f, field, "wrong field reported")
                }
                other => panic!("{field}: expected BelowMinimum, got {other:?}"),
            }
        }
    }

    #[test]
    fn an_inverted_reconnect_range_is_rejected() {
        let config = CloudConfig {
            reconnect_base_seconds: 60,
            reconnect_max_seconds: 30,
            ..CloudConfig::default()
        };
        assert!(matches!(
            config.validate(),
            Err(CloudConfigError::ReconnectRangeInverted { base: 60, max: 30 })
        ));
    }

    #[test]
    fn the_kill_switch_is_a_plain_setting() {
        let config = CloudConfig {
            allow_privileged_execution: false,
            ..CloudConfig::default()
        };
        assert!(!config.permits_privileged_execution());
    }

    #[test]
    fn a_full_table_parses_from_toml() {
        let config: CloudConfig = toml::from_str(
            r#"
enabled = true
endpoint = "wss://cloud.example.com/agent"
expected_cloud_id = "argus-cloud"
allow_privileged_execution = false
telemetry_interval_seconds = 30
report_buffer_max_records = 1000
request_timeout_seconds = 60
max_concurrent_privileged = 1
reconnect_base_seconds = 2
reconnect_max_seconds = 600
"#,
        )
        .expect("parses");
        assert!(config.is_active());
        assert!(!config.permits_privileged_execution());
        assert_eq!(config.reconnect_max_seconds, 600);
        config.validate().expect("valid");
    }
}
