//! Daemon configuration.

use std::path::{Path, PathBuf};

use argus_cloud::config::{CloudConfig, CloudConfigError};
use argus_domain::EffectiveSource;
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
    #[serde(default)]
    pub cloud: CloudConfig,
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
            cloud: CloudConfig::default(),
        }
    }
}

/// Paths searched for `argus.toml` when the operator does not name one.
pub const DEFAULT_CONFIG_PATHS: [&str; 2] = ["argus.toml", "/etc/argus/argus.toml"];

/// The on-disk shape of `argus.toml`.
///
/// Unknown *top-level* keys are tolerated because the setup wizard also writes
/// sections this daemon does not read, such as `[model]`. Unknown keys *inside*
/// `[cloud]` are rejected by `CloudConfig`, so a typo cannot silently disable a
/// safety setting.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ConfigFile {
    #[serde(default)]
    pub socket_path: Option<String>,
    #[serde(default)]
    pub state_path: Option<String>,
    #[serde(default)]
    pub environment_name: Option<String>,
    #[serde(default)]
    pub authorized_uids: Option<Vec<u32>>,
    #[serde(default)]
    pub otel_endpoint: Option<String>,
    #[serde(default)]
    pub cloud: Option<CloudConfig>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read config file '{path}': {source}")]
    Read {
        path: String,
        source: std::io::Error,
    },

    #[error("cannot parse config file '{path}': {source}")]
    Parse {
        path: String,
        source: toml::de::Error,
    },

    #[error("invalid cloud configuration: {0}")]
    Cloud(#[from] CloudConfigError),
}

#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub config: DaemonConfig,
    pub source_path: Option<PathBuf>,
    pub warnings: Vec<String>,
}

/// Chooses the configuration file to use.
///
/// An explicit path always wins, even if it does not exist, so a typo in
/// `--config` fails loudly instead of silently falling back.
pub fn resolve_config_path(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = explicit {
        return Some(path.to_path_buf());
    }
    DEFAULT_CONFIG_PATHS
        .iter()
        .map(PathBuf::from)
        .find(|candidate| candidate.is_file())
}

/// Precedence is `defaults < file < flags`; flags are applied afterwards by the
/// caller, which is why only the first two are handled here.
///
/// A file the operator named explicitly must be readable and valid, because a
/// typo there is an operator error worth failing on. A file found by discovery
/// that is broken produces a warning and the defaults are used, so a broken
/// optional file cannot take a working daemon down.
pub fn load(explicit: Option<&Path>) -> Result<LoadedConfig, ConfigError> {
    let Some(path) = resolve_config_path(explicit) else {
        return Ok(LoadedConfig {
            config: DaemonConfig::default(),
            source_path: None,
            warnings: Vec::new(),
        });
    };
    load_from(&path, explicit.is_some())
}

fn load_from(path: &Path, explicit: bool) -> Result<LoadedConfig, ConfigError> {
    let mut warnings = Vec::new();
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(source) => {
            if explicit {
                return Err(ConfigError::Read {
                    path: path.display().to_string(),
                    source,
                });
            }
            warnings.push(format!(
                "ignoring unreadable config file '{}': {source}",
                path.display()
            ));
            return Ok(LoadedConfig {
                config: DaemonConfig::default(),
                source_path: None,
                warnings,
            });
        }
    };

    let file: ConfigFile = match toml::from_str(&text) {
        Ok(file) => file,
        Err(source) => {
            let message = format!("cannot parse config file '{}': {source}", path.display());
            if explicit {
                return Err(ConfigError::Parse {
                    path: path.display().to_string(),
                    source,
                });
            }
            warnings.push(format!("{message}; using defaults"));
            return Ok(LoadedConfig {
                config: DaemonConfig::default(),
                source_path: None,
                warnings,
            });
        }
    };

    let mut config = DaemonConfig::default();
    apply(&mut config, file);
    config.cloud.validate()?;

    Ok(LoadedConfig {
        config,
        source_path: Some(path.to_path_buf()),
        warnings,
    })
}

fn apply(config: &mut DaemonConfig, file: ConfigFile) {
    if let Some(value) = file.socket_path {
        config.socket_path = value;
    }
    if let Some(value) = file.state_path {
        config.state_path = value;
    }
    if let Some(value) = file.environment_name {
        config.environment_name = value;
    }
    if let Some(value) = file.authorized_uids {
        config.authorized_uids = Some(value);
    }
    if let Some(value) = file.otel_endpoint {
        config.otel_endpoint = Some(value);
    }
    if let Some(value) = file.cloud {
        config.cloud = value;
    }
}

/// Provider and model settings, the configuration kind this release can make
/// effective.
pub const KIND_PROVIDER: &str = "provider";
/// Agent-team configuration: recognised, but nothing local consumes it yet.
pub const KIND_AGENT_TEAM: &str = "agent_team";
/// Orchestration configuration: same deferral as the agent team.
pub const KIND_ORCHESTRATION: &str = "orchestration";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigDisposition {
    /// Supported and made effective in this release.
    Apply,
    /// Recognised, but no local runtime consumes it yet. Refused with a reason
    /// rather than accepted and quietly ignored, so the cloud's deployment view
    /// tells the truth.
    Deferred,
    Unknown,
}

impl ConfigDisposition {
    /// Why a kind will not be applied, or `None` when it will be.
    ///
    /// The two refusal reasons differ on purpose: "deferred" tells the operator
    /// the feature is coming, "unknown" tells them the cloud sent something this
    /// build does not understand.
    pub fn refusal_reason(self) -> Option<&'static str> {
        match self {
            Self::Apply => None,
            Self::Deferred => Some(
                "this configuration kind is recognised but has no local runtime in this release, \
                 so it was not applied",
            ),
            Self::Unknown => Some("this configuration kind is not recognised by this installation"),
        }
    }
}

pub fn disposition(kind: &str) -> ConfigDisposition {
    match kind {
        KIND_PROVIDER => ConfigDisposition::Apply,
        KIND_AGENT_TEAM | KIND_ORCHESTRATION => ConfigDisposition::Deferred,
        _ => ConfigDisposition::Unknown,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved<T> {
    pub value: T,
    pub source: EffectiveSource,
}

/// Applies the documented precedence: `defaults < file < cloud < flag`.
///
/// A step only wins when it actually supplies a value, so a file that omits a
/// setting does not erase the default and an absent cloud setting does not erase
/// a file value. The winning source is recorded, because an operator asking "why
/// is this provider set to X?" deserves a better answer than a shrug.
pub fn resolve<T: Clone>(
    default: T,
    file: Option<T>,
    cloud: Option<T>,
    flag: Option<T>,
) -> Resolved<T> {
    let mut resolved = Resolved {
        value: default,
        source: EffectiveSource::Default,
    };

    for (candidate, source) in [
        (file, EffectiveSource::File),
        (cloud, EffectiveSource::Cloud),
        (flag, EffectiveSource::Flag),
    ] {
        if let Some(value) = candidate {
            resolved = Resolved { value, source };
        }
    }

    resolved
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn the_provider_kind_is_applied() {
        assert_eq!(disposition(KIND_PROVIDER), ConfigDisposition::Apply);
        assert_eq!(disposition(KIND_PROVIDER).refusal_reason(), None);
    }

    #[test]
    fn the_agent_team_kind_is_deferred_with_a_specific_reason() {
        // The Q1b decision: refused because nothing consumes it, not silently
        // accepted and reported as applied.
        for kind in [KIND_AGENT_TEAM, KIND_ORCHESTRATION] {
            let disposition = disposition(kind);
            assert_eq!(disposition, ConfigDisposition::Deferred, "{kind}");
            let reason = disposition.refusal_reason().expect("must explain itself");
            assert!(
                reason.contains("no local runtime"),
                "the reason must say why, not just that it failed: {reason}"
            );
        }
    }

    #[test]
    fn an_unrecognised_kind_is_refused_as_unknown_not_as_deferred() {
        assert_eq!(disposition("something_new"), ConfigDisposition::Unknown);
        let reason = disposition("something_new").refusal_reason().unwrap();
        assert!(reason.contains("not recognised"), "{reason}");
        assert_ne!(
            disposition("something_new").refusal_reason(),
            disposition(KIND_AGENT_TEAM).refusal_reason(),
            "a build that does not know a kind must not claim it is merely deferred"
        );
    }

    #[test]
    fn defaults_are_used_when_nothing_else_supplies_a_value() {
        let resolved = resolve("default", None, None, None);
        assert_eq!(resolved.value, "default");
        assert_eq!(resolved.source, EffectiveSource::Default);
    }

    #[test]
    fn the_file_overrides_the_default() {
        let resolved = resolve("default", Some("file"), None, None);
        assert_eq!(resolved.value, "file");
        assert_eq!(resolved.source, EffectiveSource::File);
    }

    #[test]
    fn cloud_configuration_overrides_the_file() {
        // FR-033 requires cloud configuration to take effect; a local file that
        // outranked it would make the reported `applied` status a lie.
        let resolved = resolve("default", Some("file"), Some("cloud"), None);
        assert_eq!(resolved.value, "cloud");
        assert_eq!(resolved.source, EffectiveSource::Cloud);
    }

    #[test]
    fn a_flag_overrides_the_cloud() {
        // The deliberate escape hatch: an operator must retain a local remedy
        // when a cloud-delivered configuration is wrong.
        let resolved = resolve("default", Some("file"), Some("cloud"), Some("flag"));
        assert_eq!(resolved.value, "flag");
        assert_eq!(resolved.source, EffectiveSource::Flag);
    }

    #[test]
    fn a_missing_higher_source_does_not_erase_a_lower_one() {
        let resolved = resolve("default", Some("file"), None, Some("flag"));
        assert_eq!(resolved.value, "flag");

        let only_cloud = resolve("default", None, Some("cloud"), None);
        assert_eq!(
            only_cloud.value, "cloud",
            "absent file must not clear the cloud"
        );
    }

    #[test]
    fn provenance_is_recorded_for_every_combination() {
        let cases = [
            (None, None, None, EffectiveSource::Default),
            (Some(1), None, None, EffectiveSource::File),
            (Some(1), Some(2), None, EffectiveSource::Cloud),
            (Some(1), Some(2), Some(3), EffectiveSource::Flag),
        ];
        for (file, cloud, flag, expected) in cases {
            let resolved = resolve(0, file, cloud, flag);
            assert_eq!(resolved.source, expected, "{file:?} {cloud:?} {flag:?}");
        }
    }

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("argus-cfg-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn write_config(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        let mut file = std::fs::File::create(&path).expect("create");
        file.write_all(body.as_bytes()).expect("write");
        path
    }

    #[test]
    fn no_file_anywhere_yields_defaults_without_error() {
        let loaded = load(None).expect("absence is not an error");
        assert_eq!(loaded.config, DaemonConfig::default());
        assert!(loaded.source_path.is_none() || loaded.source_path.is_some());
        assert!(!loaded.config.cloud.enabled);
    }

    #[test]
    fn a_file_overrides_the_defaults() {
        let dir = temp_dir();
        let path = write_config(
            &dir,
            "argus.toml",
            r#"
socket_path = "/tmp/custom.sock"
environment_name = "staging"

[cloud]
enabled = true
endpoint = "wss://cloud.example.com/agent"
telemetry_interval_seconds = 30
"#,
        );

        let loaded = load(Some(&path)).expect("valid");
        assert_eq!(loaded.config.socket_path, "/tmp/custom.sock");
        assert_eq!(loaded.config.environment_name, "staging");
        assert!(loaded.config.cloud.enabled);
        assert_eq!(loaded.config.cloud.telemetry_interval_seconds, 30);
        assert_eq!(loaded.source_path.as_deref(), Some(path.as_path()));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unspecified_fields_keep_their_defaults() {
        let dir = temp_dir();
        let path = write_config(&dir, "argus.toml", "environment_name = \"partial\"\n");

        let loaded = load(Some(&path)).expect("valid");
        assert_eq!(loaded.config.environment_name, "partial");
        assert_eq!(
            loaded.config.socket_path,
            DaemonConfig::default().socket_path
        );
        assert_eq!(loaded.config.cloud, CloudConfig::default());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unknown_keys_under_cloud_are_rejected() {
        let dir = temp_dir();
        let path = write_config(
            &dir,
            "argus.toml",
            "[cloud]\nallow_privleged_execution = false\n",
        );

        let err = load(Some(&path)).expect_err("a misspelled safety setting must fail");
        assert!(matches!(err, ConfigError::Parse { .. }), "{err:?}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unknown_top_level_section_is_tolerated() {
        let dir = temp_dir();
        let path = write_config(
            &dir,
            "argus.toml",
            "environment_name = \"keep\"\n\n[model]\nprovider = \"openai\"\n",
        );

        let loaded =
            load(Some(&path)).expect("the wizard's [model] section must not break startup");
        assert_eq!(loaded.config.environment_name, "keep");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_explicit_malformed_file_is_an_error() {
        let dir = temp_dir();
        let path = write_config(&dir, "broken.toml", "this is not = = toml\n");

        let err = load(Some(&path)).expect_err("an explicit file must fail loudly");
        assert!(matches!(err, ConfigError::Parse { .. }), "{err:?}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_explicit_missing_file_is_an_error() {
        let dir = temp_dir();
        let missing = dir.join("does-not-exist.toml");

        let err = load(Some(&missing)).expect_err("an explicit path must exist");
        assert!(matches!(err, ConfigError::Read { .. }), "{err:?}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_discovered_malformed_file_warns_and_uses_defaults() {
        let dir = temp_dir();
        let path = write_config(&dir, "argus.toml", "this is not = = toml\n");

        let loaded = load_from(&path, false).expect("discovery must not fail on a broken file");
        assert_eq!(loaded.config, DaemonConfig::default());
        assert!(loaded.source_path.is_none());
        assert!(
            loaded.warnings.iter().any(|w| w.contains("cannot parse")),
            "the operator must be warned: {:?}",
            loaded.warnings
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_discovered_unreadable_file_warns_and_uses_defaults() {
        let dir = temp_dir();
        let missing = dir.join("absent.toml");

        let loaded = load_from(&missing, false).expect("discovery must not fail on a missing file");
        assert_eq!(loaded.config, DaemonConfig::default());
        assert!(
            loaded.warnings.iter().any(|w| w.contains("unreadable")),
            "the operator must be warned: {:?}",
            loaded.warnings
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn enabling_the_cloud_without_an_endpoint_fails_validation() {
        let dir = temp_dir();
        let path = write_config(&dir, "argus.toml", "[cloud]\nenabled = true\n");

        let err = load(Some(&path)).expect_err("enabled without endpoint is invalid");
        assert!(
            matches!(err, ConfigError::Cloud(CloudConfigError::MissingEndpoint)),
            "{err:?}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_kill_switch_loads_from_the_file() {
        let dir = temp_dir();
        let path = write_config(
            &dir,
            "argus.toml",
            "[cloud]\nenabled = false\nallow_privileged_execution = false\n",
        );

        let loaded = load(Some(&path)).expect("valid");
        assert!(!loaded.config.cloud.permits_privileged_execution());

        std::fs::remove_dir_all(&dir).ok();
    }
}
