//! Daemon configuration.

use std::path::{Path, PathBuf};

use argus_cloud::config::{CloudConfig, CloudConfigError};
use argus_domain::{CapabilityId, EffectiveSource};
use argus_observability::LogFormat;
use serde::{Deserialize, Serialize};

/// Resolved configuration for a single `argusd` instance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// The AI decision-provider settings (spec 005 FR-002). Absent means
    /// no provider: the brain runs observe-only.
    #[serde(default)]
    pub model: argus_ai_core::model::ModelProviderConfig,
    /// The brain loop settings (spec 005 FR-003/004).
    #[serde(default)]
    pub brain: BrainConfig,
    /// The optional Kubernetes cluster (ADR-0037): absent means no cluster,
    /// and the k8s capabilities degrade honestly.
    #[serde(default)]
    pub kubernetes: KubernetesConfig,
    /// The action ledger's local + upload settings (spec 007). Defaults keep
    /// the ledger always on (local) and upload on with bounded retention.
    #[serde(default)]
    pub ledger: LedgerConfig,
    /// The graduated-autonomy settings (spec 008 FR-006). Absent means the
    /// defaults: the assimilation machine works below whatever `brain.autonomy`
    /// ceiling exists — it never raises one.
    #[serde(default)]
    pub autonomy: AutonomyConfig,
    /// The situation thresholds (spec 011 FR-001). Absent means the defaults:
    /// 90%, which ordinary operation stays below, so the default
    /// configuration observes exactly as today.
    #[serde(default)]
    pub situations: SituationsConfig,
    /// The pressure-action settings (spec 012 FR-001/FR-002). Absent means
    /// the defaults: the standard drop_caches sysctl and a 7-day journal
    /// retention.
    #[serde(default)]
    pub capabilities: CapabilitiesConfig,
    /// The external MCP servers (spec 014 FR-001). Absent or empty means no
    /// runtime, no capabilities — byte-identical behavior (ADR-0044 §5).
    #[serde(default)]
    pub mcp: McpConfig,
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
            model: argus_ai_core::model::ModelProviderConfig::default(),
            brain: BrainConfig::default(),
            kubernetes: KubernetesConfig::default(),
            ledger: LedgerConfig::default(),
            autonomy: AutonomyConfig::default(),
            situations: SituationsConfig::default(),
            capabilities: CapabilitiesConfig::default(),
            mcp: McpConfig::default(),
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
    #[serde(default)]
    pub model: Option<argus_ai_core::model::ModelProviderConfig>,
    #[serde(default)]
    pub brain: Option<BrainConfig>,
    #[serde(default)]
    pub kubernetes: Option<KubernetesConfig>,
    #[serde(default)]
    pub ledger: Option<LedgerConfig>,
    #[serde(default)]
    pub autonomy: Option<AutonomyConfig>,
    #[serde(default)]
    pub situations: Option<SituationsConfig>,
    #[serde(default)]
    pub capabilities: Option<CapabilitiesConfig>,
    #[serde(default)]
    pub mcp: Option<McpConfig>,
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

    #[error("invalid mcp configuration: {0}")]
    Mcp(String),
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

/// The optional Kubernetes cluster settings (`[kubernetes]` in argus.toml).
/// Plain strings on purpose: the daemon holds no kube-rs types unless the
/// `kubernetes` feature is compiled in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct KubernetesConfig {
    /// Path to a kubeconfig (e.g. `/etc/rancher/k3s/k3s.yaml`); absent with
    /// `enabled = true` falls back to the ambient kubeconfig rules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kubeconfig: Option<String>,
    /// Optional context name from that kubeconfig.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// Explicitly off; `true` is only meaningful with the `kubernetes`
    /// build feature and a reachable cluster.
    #[serde(default)]
    pub enabled: bool,
}

/// Brain-loop settings (`[brain]` in argus.toml).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BrainConfig {
    /// The autonomy level the loop runs at — default L0: the brain observes
    /// and explains; execution requires the operator to raise this, and even
    /// then every plan crosses policy/escalation/approvals as usual.
    pub autonomy: argus_domain::AutonomyMode,
    /// Seconds between brain ticks.
    pub interval_seconds: u64,
    /// The confidence gate for acting on a decision (`diagnose_once`).
    pub confidence_threshold: f64,
    /// Directory of runbook TOML files; absent means no runbooks load.
    pub runbooks_dir: Option<String>,
}

impl Default for BrainConfig {
    fn default() -> Self {
        Self {
            autonomy: argus_domain::AutonomyMode::L0Observe,
            interval_seconds: 60,
            confidence_threshold: 0.3,
            runbooks_dir: None,
        }
    }
}

impl<'de> Deserialize<'de> for BrainConfig {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct BrainWire {
            #[serde(default)]
            autonomy: Option<argus_domain::AutonomyMode>,
            #[serde(default)]
            interval_seconds: Option<u64>,
            #[serde(default)]
            confidence_threshold: Option<f64>,
            #[serde(default)]
            runbooks_dir: Option<String>,
        }
        let wire = BrainWire::deserialize(deserializer)?;
        Ok(Self {
            autonomy: wire.autonomy.unwrap_or_default(),
            interval_seconds: wire.interval_seconds.unwrap_or(60).max(5),
            confidence_threshold: wire.confidence_threshold.unwrap_or(0.3).clamp(0.0, 1.0),
            runbooks_dir: wire.runbooks_dir,
        })
    }
}

/// Action-ledger settings (`[ledger]` in argus.toml, spec 007).
///
/// The ledger itself is always local — only the *upload* is optional, so an
/// operator can keep the audit trail while silencing the stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LedgerConfig {
    /// Stream redacted ledger events to the cloud when enrolled. The local
    /// ledger is unaffected by this switch (spec 007 NFR).
    pub upload_enabled: bool,
    /// How many days of local ledger history to retain (bounded growth).
    pub retention_days: i64,
}

impl Default for LedgerConfig {
    fn default() -> Self {
        Self {
            upload_enabled: true,
            retention_days: 30,
        }
    }
}

impl<'de> Deserialize<'de> for LedgerConfig {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct LedgerWire {
            #[serde(default)]
            upload_enabled: Option<bool>,
            #[serde(default)]
            retention_days: Option<i64>,
        }
        let wire = LedgerWire::deserialize(deserializer)?;
        Ok(Self {
            upload_enabled: wire.upload_enabled.unwrap_or(true),
            retention_days: wire.retention_days.unwrap_or(30).clamp(1, 3650),
        })
    }
}

/// Graduated-autonomy settings (`[autonomy]` in argus.toml, spec 008 FR-006).
///
/// Absent section → defaults; every knob is clamped, so a typo'd extreme
/// degrades to the nearest valid bound rather than disabling the gates. The
/// ceiling semantics and defaults of `brain.autonomy` are unchanged — this
/// machine works below whatever ceiling exists, it never raises one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AutonomyConfig {
    /// Completed clean cycles the shadow window requires before the phase
    /// turns `earned` (default 10, clamped 1–1000).
    pub shadow_min_cycles: u32,
    /// Clean cycles required at each rung before the next promotion
    /// (default 20, clamped 1–10 000).
    pub rung_clean_cycles: u32,
    /// The blast-radius budget: anchored windows keyed by risk class and
    /// scope (spec 008 FR-004), each key clamped ≥1.
    pub budgets: argus_policy::BudgetLimits,
}

impl Default for AutonomyConfig {
    fn default() -> Self {
        Self {
            shadow_min_cycles: 10,
            rung_clean_cycles: 20,
            budgets: argus_policy::BudgetLimits::default(),
        }
    }
}

impl<'de> Deserialize<'de> for AutonomyConfig {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct AutonomyWire {
            #[serde(default)]
            shadow_min_cycles: Option<u32>,
            #[serde(default)]
            rung_clean_cycles: Option<u32>,
            #[serde(default)]
            budgets: Option<argus_policy::BudgetLimits>,
        }
        let wire = AutonomyWire::deserialize(deserializer)?;
        Ok(Self {
            shadow_min_cycles: wire.shadow_min_cycles.unwrap_or(10).clamp(1, 1000),
            rung_clean_cycles: wire.rung_clean_cycles.unwrap_or(20).clamp(1, 10_000),
            budgets: wire.budgets.unwrap_or_default(),
        })
    }
}

/// Situation-threshold settings (`[situations]` in argus.toml, spec 011
/// FR-001).
///
/// Absent section → defaults; every knob is clamped, so a typo'd extreme
/// degrades to the nearest valid bound rather than disabling the checks (the
/// `AutonomyConfig` pattern). The defaults sit at 90% — above ordinary
/// operation — so the default configuration collects no pressure situations
/// and observes exactly as today (AC-006).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SituationsConfig {
    /// The memory-pressure crossing: `memory.used_percent` at/above this
    /// collects a `memory-pressure` situation (default 90, clamped 50–99).
    pub memory_used_percent: f64,
    /// The disk-pressure crossing: root-filesystem usage percent at/above
    /// this collects a `disk-pressure` situation (default 90, clamped 50–99).
    pub disk_used_percent: f64,
}

impl Default for SituationsConfig {
    fn default() -> Self {
        Self {
            memory_used_percent: 90.0,
            disk_used_percent: 90.0,
        }
    }
}

impl<'de> Deserialize<'de> for SituationsConfig {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct SituationsWire {
            #[serde(default)]
            memory_used_percent: Option<f64>,
            #[serde(default)]
            disk_used_percent: Option<f64>,
        }
        let wire = SituationsWire::deserialize(deserializer)?;
        Ok(Self {
            memory_used_percent: wire.memory_used_percent.unwrap_or(90.0).clamp(50.0, 99.0),
            disk_used_percent: wire.disk_used_percent.unwrap_or(90.0).clamp(50.0, 99.0),
        })
    }
}

/// The standard drop_caches sysctl — the default write path
/// `host.memory.reclaim` puts `1` to (spec 012 FR-001).
pub const DEFAULT_DROP_CACHES_PATH: &str = "/proc/sys/vm/drop_caches";

/// Pressure-action settings (`[capabilities]` in argus.toml, spec 012
/// FR-001/FR-002).
///
/// Absent section → defaults; the retention bound is clamped, so a typo'd
/// extreme degrades to the nearest valid bound rather than vacuuming the
/// journal to nothing (the `AutonomyConfig` pattern). The write path is
/// config-injectable for tests — tests point it at a temp file; production
/// writes the standard sysctl and needs the unit's `ReadWritePaths` carve-out
/// for `/proc/sys/vm` (ADR-0021).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CapabilitiesConfig {
    /// The drop_caches sysctl `host.memory.reclaim` writes `1` to — page
    /// cache only (default `/proc/sys/vm/drop_caches`).
    pub drop_caches_path: String,
    /// The `host.journal.vacuum` retention bound in days: the call vacuums
    /// everything older than `now − keep_days` (default 7, clamped 1–365).
    pub journal_keep_days: u32,
}

impl Default for CapabilitiesConfig {
    fn default() -> Self {
        Self {
            drop_caches_path: DEFAULT_DROP_CACHES_PATH.to_string(),
            journal_keep_days: 7,
        }
    }
}

impl<'de> Deserialize<'de> for CapabilitiesConfig {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct CapabilitiesWire {
            #[serde(default)]
            drop_caches_path: Option<String>,
            #[serde(default)]
            journal_keep_days: Option<u32>,
        }
        let wire = CapabilitiesWire::deserialize(deserializer)?;
        Ok(Self {
            drop_caches_path: wire
                .drop_caches_path
                .unwrap_or_else(|| DEFAULT_DROP_CACHES_PATH.to_string()),
            journal_keep_days: wire.journal_keep_days.unwrap_or(7).clamp(1, 365),
        })
    }
}

/// External MCP server settings (`[mcp]` in argus.toml, spec 014 FR-001).
///
/// Deny-by-default and config-driven: no server, no tool, no call exists
/// unless the operator declared it here. An absent or empty `[mcp]` section
/// means no runtime and no capabilities — byte-identical behavior
/// (ADR-0044 §5).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct McpConfig {
    /// The configured servers. An unlisted or `enabled = false` server
    /// contributes nothing.
    pub servers: Vec<McpServerConfig>,
}

impl<'de> Deserialize<'de> for McpConfig {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct McpWire {
            #[serde(default)]
            servers: Vec<McpServerConfig>,
        }
        Ok(Self {
            servers: McpWire::deserialize(deserializer)?.servers,
        })
    }
}

impl McpConfig {
    /// The configured server that is actually enabled.
    pub fn enabled_servers(&self) -> impl Iterator<Item = &McpServerConfig> {
        self.servers.iter().filter(|server| server.enabled)
    }

    /// Merges a cloud-delivered servers document over the file-declared one
    /// (spec 015 FR-005, ADR-0045 §4): a delivered server replaces the file's
    /// same-named one — managed supersedes file — while file-only servers keep
    /// working and delivered-only servers are appended in their document
    /// order. Neither side is validated here; the caller validates the merged
    /// result exactly as the loader validates the file.
    pub fn merged_over(&self, delivered: &McpConfig) -> McpConfig {
        let mut merged: Vec<McpServerConfig> = self
            .servers
            .iter()
            .map(|file_server| {
                delivered
                    .servers
                    .iter()
                    .find(|delivered_server| delivered_server.name == file_server.name)
                    .cloned()
                    .unwrap_or_else(|| file_server.clone())
            })
            .collect();
        for delivered_server in &delivered.servers {
            if !merged
                .iter()
                .any(|server| server.name == delivered_server.name)
            {
                merged.push(delivered_server.clone());
            }
        }
        McpConfig { servers: merged }
    }

    /// Fails closed on a declaration the runtime could not honor: duplicate
    /// or malformed names, a transport missing its endpoint, or an override
    /// naming an unrepresentable tool. Called at load, so a typo fails the
    /// startup loudly instead of silently contributing nothing.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let mut seen = std::collections::HashSet::new();
        for server in &self.servers {
            if !is_capability_segment(&server.name) {
                return Err(ConfigError::Mcp(format!(
                    "server name '{}' must be a lowercase capability-id segment without dots",
                    server.name
                )));
            }
            if !seen.insert(server.name.clone()) {
                return Err(ConfigError::Mcp(format!(
                    "duplicate mcp server name '{}'",
                    server.name
                )));
            }
            match &server.transport {
                McpTransportConfig::Stdio { command, .. } => {
                    if command.trim().is_empty() {
                        return Err(ConfigError::Mcp(format!(
                            "server '{}' declares an empty stdio command",
                            server.name
                        )));
                    }
                }
                McpTransportConfig::Http { url } => {
                    if !url.starts_with("http://") && !url.starts_with("https://") {
                        return Err(ConfigError::Mcp(format!(
                            "server '{}' http url must start with http:// or https://",
                            server.name
                        )));
                    }
                }
            }
            if let Some(tools) = &server.tools {
                for tool in tools {
                    if !is_capability_segment(tool) {
                        return Err(ConfigError::Mcp(format!(
                            "server '{}' allowlists '{}', which is not a valid tool segment",
                            server.name, tool
                        )));
                    }
                }
            }
            for tool in &server.overrides {
                if !is_capability_segment(&tool.name) {
                    return Err(ConfigError::Mcp(format!(
                        "server '{}' overrides '{}', which is not a valid tool segment",
                        server.name, tool.name
                    )));
                }
            }
        }
        Ok(())
    }
}

/// Whether `name` can serve as ONE segment of `mcp.<server>.<tool>`: the
/// domain's segment grammar, with dots refused — a dotted name would make the
/// namespacing ambiguous and break its no-collision-by-construction property
/// (ADR-0044 §2).
fn is_capability_segment(name: &str) -> bool {
    !name.contains('.') && CapabilityId::new(&format!("mcp.{name}.probe")).is_ok()
}

/// One configured MCP server (`[[mcp.servers]]`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct McpServerConfig {
    /// The namespace segment: its tools register as `mcp.<name>.<tool>`.
    pub name: String,
    pub transport: McpTransportConfig,
    /// Explicitly off; `true` connects the server at startup. The default is
    /// the safe one (spec 014 FR-001).
    #[serde(default)]
    pub enabled: bool,
    /// The per-call and connect timeout in seconds (default 30, clamped
    /// 5–300).
    pub timeout_seconds: u32,
    /// The tool allowlist (`tools = [...]`); absent means every tool the
    /// server advertises (spec 014 FR-001).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    /// Per-tool risk overrides (`[[mcp.servers.overrides]]`) — the operator
    /// writing law, ADR-0044 §1.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<McpToolConfig>,
}

impl<'de> Deserialize<'de> for McpServerConfig {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct McpServerWire {
            name: String,
            transport: McpTransportConfig,
            #[serde(default)]
            enabled: Option<bool>,
            #[serde(default)]
            timeout_seconds: Option<u32>,
            #[serde(default)]
            tools: Option<Vec<String>>,
            #[serde(default)]
            overrides: Vec<McpToolConfig>,
        }
        let wire = McpServerWire::deserialize(deserializer)?;
        Ok(Self {
            name: wire.name,
            transport: wire.transport,
            enabled: wire.enabled.unwrap_or(false),
            timeout_seconds: wire.timeout_seconds.unwrap_or(30).clamp(5, 300),
            tools: wire.tools,
            overrides: wire.overrides,
        })
    }
}

/// How the daemon reaches one MCP server (spec 014 FR-001).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpTransportConfig {
    /// A subprocess the runtime owns — the transport, not a shell.
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
    /// A streamable-HTTP endpoint.
    Http { url: String },
}

/// One per-tool trust override (`[[mcp.servers.overrides]]`): the ONLY mechanism
/// that lowers an external tool's authority — the server's self-reports never
/// reach the runtime (ADR-0044 §1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpToolConfig {
    /// The MCP tool name exactly as the server advertises it.
    pub name: String,
    /// The risk class the operator assigns; `risk = "controlled"` lets the
    /// autonomy matrix govern the tool at L4+ within budget.
    pub risk: argus_domain::RiskClass,
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
    config.mcp.validate()?;

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
    if let Some(value) = file.model {
        config.model = value;
    }
    if let Some(value) = file.brain {
        config.brain = value;
    }
    if let Some(value) = file.kubernetes {
        config.kubernetes = value;
    }
    if let Some(value) = file.ledger {
        config.ledger = value;
    }
    if let Some(value) = file.autonomy {
        config.autonomy = value;
    }
    if let Some(value) = file.situations {
        config.situations = value;
    }
    if let Some(value) = file.capabilities {
        config.capabilities = value;
    }
    if let Some(value) = file.mcp {
        config.mcp = value;
    }
}

/// Provider and model settings, the configuration kind this release can make
/// effective.
pub const KIND_PROVIDER: &str = "provider";
/// Brain-loop settings (spec 006): the managed lever for the running brain.
pub const KIND_BRAIN: &str = "brain";
/// Runbook distribution (spec 009): the content is the runbook's TOML
/// document. Applied through the runbook loader as a Candidate with cloud
/// provenance — delivery is data, never promotion (ADR-0041).
pub const KIND_RUNBOOK: &str = "runbook";
/// MCP server topology (spec 015, ADR-0045 §3): the content is the servers
/// document — the same grammar as the `[mcp]` file section. Applied live
/// through the MCP wiring's reconcile: declaring a server is configuration,
/// never execution.
pub const KIND_MCP: &str = "mcp";
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
        KIND_PROVIDER | KIND_BRAIN | KIND_RUNBOOK | KIND_MCP => ConfigDisposition::Apply,
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
    fn the_runbook_kind_is_applied_not_deferred() {
        // Spec 009 FR-001: the runbook kind is applied in this release — its
        // refusal reason would be a lie.
        assert_eq!(disposition(KIND_RUNBOOK), ConfigDisposition::Apply);
        assert_eq!(disposition(KIND_RUNBOOK).refusal_reason(), None);
    }

    #[test]
    fn the_mcp_kind_is_applied_not_deferred() {
        // Spec 015 FR-001: the mcp kind is applied in this release through the
        // wiring's live reconcile — a "coming soon" refusal would be a lie.
        assert_eq!(disposition(KIND_MCP), ConfigDisposition::Apply);
        assert_eq!(disposition(KIND_MCP).refusal_reason(), None);
    }

    #[test]
    fn a_delivered_mcp_document_supersedes_the_file_on_name_conflict_only() {
        // Spec 015 FR-005 / ADR-0045 §4: cloud wins per server NAME; file-only
        // servers keep working; delivered-only servers are appended.
        let file_server = |name: &str, command: &str| McpServerConfig {
            name: name.to_string(),
            transport: crate::config::McpTransportConfig::Stdio {
                command: command.to_string(),
                args: Vec::new(),
            },
            enabled: true,
            timeout_seconds: 30,
            tools: None,
            overrides: Vec::new(),
        };
        let file = McpConfig {
            servers: vec![
                file_server("shared", "/bin/file-version"),
                file_server("file-only", "/bin/file-only"),
            ],
        };
        let mut cloud_shared = file_server("shared", "/bin/cloud-version");
        cloud_shared.timeout_seconds = 42;
        let delivered = McpConfig {
            servers: vec![cloud_shared, file_server("cloud-only", "/bin/cloud-only")],
        };

        let merged = file.merged_over(&delivered);
        let names: Vec<&str> = merged.servers.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["shared", "file-only", "cloud-only"]);
        let shared = merged.servers.iter().find(|s| s.name == "shared").unwrap();
        assert!(
            matches!(&shared.transport, McpTransportConfig::Stdio { command, .. } if command == "/bin/cloud-version"),
            "the delivered server replaces the file's same-named one"
        );
        assert_eq!(shared.timeout_seconds, 42);
        assert!(merged.validate().is_ok(), "the merged set is coherent");
    }

    #[test]
    fn a_delivered_mcp_document_alone_reseeds_exactly_itself() {
        // An empty file section means the delivered document is the whole
        // topology (spec 015: the dashboard can introduce servers to a host
        // whose argus.toml declares none).
        let file = McpConfig::default();
        let delivered: McpConfig = toml::from_str(
            r#"
            [[servers]]
            name = "cloud-only"
            enabled = true
            transport = { type = "stdio", command = "/bin/cloud-only" }
            "#,
        )
        .expect("a delivered servers document parses with the ordinary rules");
        let merged = file.merged_over(&delivered);
        assert_eq!(merged.servers.len(), 1);
        assert_eq!(merged.servers[0].name, "cloud-only");
        assert!(merged.validate().is_ok());
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
    fn load_without_an_explicit_path_is_environment_consistent() {
        // Hermetic: the default search may find a real config on the machine
        // running the tests (e.g. a deployed /root/argus.toml). The
        // invariant is that a None-search either yields pure defaults
        // (nothing found) or a parsed config with a recorded source path.
        let loaded = load(None).expect("absence is not an error");
        match loaded.source_path {
            None => assert_eq!(loaded.config, DaemonConfig::default()),
            Some(path) => assert!(!path.as_os_str().is_empty(), "source path is real"),
        }
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

    #[test]
    fn the_ledger_section_loads_with_defaults_and_clamps() {
        let dir = temp_dir();
        let path = write_config(
            &dir,
            "argus.toml",
            "[ledger]\nupload_enabled = false\nretention_days = 7\n",
        );

        let loaded = load(Some(&path)).expect("valid");
        assert!(
            !loaded.config.ledger.upload_enabled,
            "upload can be silenced"
        );
        assert_eq!(loaded.config.ledger.retention_days, 7);

        // Absent section: defaults (upload on, 30 days) — the ledger itself is
        // always local (spec 007 NFR).
        let path = write_config(&dir, "argus.toml", "environment_name = \"x\"\n");
        let loaded = load(Some(&path)).expect("valid");
        assert_eq!(loaded.config.ledger, LedgerConfig::default());

        let path = write_config(&dir, "argus.toml", "[ledger]\nretention_days = 0\n");
        let loaded = load(Some(&path)).expect("valid");
        assert_eq!(
            loaded.config.ledger.retention_days, 1,
            "clamped to the floor"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_autonomy_section_loads_with_defaults_and_clamps() {
        let dir = temp_dir();

        // Absent section: the defaults — the machine works below whatever
        // ceiling exists, it never raises one (spec 008 FR-006).
        let path = write_config(&dir, "argus.toml", "environment_name = \"x\"\n");
        let loaded = load(Some(&path)).expect("valid");
        assert_eq!(loaded.config.autonomy, AutonomyConfig::default());

        // Explicit values ride through; the budget keys clamp ≥1.
        let path = write_config(
            &dir,
            "argus.toml",
            "[autonomy]\nshadow_min_cycles = 3\nrung_clean_cycles = 5\n\n[autonomy.budgets]\nlow_risk_per_hour = 2\ncontrolled_per_day = 0\n",
        );
        let loaded = load(Some(&path)).expect("valid");
        assert_eq!(loaded.config.autonomy.shadow_min_cycles, 3);
        assert_eq!(loaded.config.autonomy.rung_clean_cycles, 5);
        assert_eq!(loaded.config.autonomy.budgets.low_risk_per_hour, 2);
        assert_eq!(
            loaded.config.autonomy.budgets.controlled_per_day, 1,
            "a zero budget is clamped to the floor — exhaustion pauses, never denies"
        );

        // Extremes clamp to the documented bounds.
        let path = write_config(
            &dir,
            "argus.toml",
            "[autonomy]\nshadow_min_cycles = 100_000\nrung_clean_cycles = 0\n",
        );
        let loaded = load(Some(&path)).expect("valid");
        assert_eq!(loaded.config.autonomy.shadow_min_cycles, 1000);
        assert_eq!(loaded.config.autonomy.rung_clean_cycles, 1);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_situations_section_loads_with_defaults_and_clamps() {
        let dir = temp_dir();

        // Absent section: the defaults (90/90) — the default configuration
        // observes exactly as today (spec 011 FR-001, AC-006).
        let path = write_config(&dir, "argus.toml", "environment_name = \"x\"\n");
        let loaded = load(Some(&path)).expect("valid");
        assert_eq!(loaded.config.situations, SituationsConfig::default());

        // Explicit values ride through.
        let path = write_config(
            &dir,
            "argus.toml",
            "[situations]\nmemory_used_percent = 75\ndisk_used_percent = 80.5\n",
        );
        let loaded = load(Some(&path)).expect("valid");
        assert_eq!(loaded.config.situations.memory_used_percent, 75.0);
        assert_eq!(loaded.config.situations.disk_used_percent, 80.5);

        // Extremes clamp to the documented bounds (50–99): a typo'd extreme
        // degrades to the nearest valid bound, never disables the checks.
        let path = write_config(
            &dir,
            "argus.toml",
            "[situations]\nmemory_used_percent = 1\ndisk_used_percent = 150\n",
        );
        let loaded = load(Some(&path)).expect("valid");
        assert_eq!(loaded.config.situations.memory_used_percent, 50.0);
        assert_eq!(loaded.config.situations.disk_used_percent, 99.0);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_capabilities_section_loads_with_defaults_and_clamps() {
        let dir = temp_dir();

        // Absent section: the defaults — the standard sysctl, 7 days of
        // journal (spec 012 FR-001/FR-002).
        let path = write_config(&dir, "argus.toml", "environment_name = \"x\"\n");
        let loaded = load(Some(&path)).expect("valid");
        assert_eq!(loaded.config.capabilities, CapabilitiesConfig::default());
        assert_eq!(
            loaded.config.capabilities.drop_caches_path,
            "/proc/sys/vm/drop_caches"
        );

        // Explicit values ride through.
        let path = write_config(
            &dir,
            "argus.toml",
            "[capabilities]\ndrop_caches_path = \"/tmp/argus-test-drop\"\njournal_keep_days = 30\n",
        );
        let loaded = load(Some(&path)).expect("valid");
        assert_eq!(
            loaded.config.capabilities.drop_caches_path,
            "/tmp/argus-test-drop"
        );
        assert_eq!(loaded.config.capabilities.journal_keep_days, 30);

        // Extremes clamp to the documented bounds (1–365): never vacuum the
        // journal to nothing, never keep it forever.
        let path = write_config(
            &dir,
            "argus.toml",
            "[capabilities]\njournal_keep_days = 0\n",
        );
        let loaded = load(Some(&path)).expect("valid");
        assert_eq!(
            loaded.config.capabilities.journal_keep_days, 1,
            "clamped to the floor"
        );
        let path = write_config(
            &dir,
            "argus.toml",
            "[capabilities]\njournal_keep_days = 10_000\n",
        );
        let loaded = load(Some(&path)).expect("valid");
        assert_eq!(
            loaded.config.capabilities.journal_keep_days, 365,
            "clamped to the ceiling"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    // --- Spec 014: the [mcp] section ---

    #[test]
    fn the_mcp_section_loads_servers_with_defaults_and_clamps() {
        let dir = temp_dir();

        // Absent section: no servers — no runtime, no capabilities
        // (ADR-0044 §5, the byte-identical default).
        let path = write_config(&dir, "argus.toml", "environment_name = \"x\"\n");
        let loaded = load(Some(&path)).expect("valid");
        assert_eq!(loaded.config.mcp, McpConfig::default());
        assert_eq!(loaded.config.mcp.servers.len(), 0);

        // A stdio server with a per-tool override; disabled is the default.
        let path = write_config(
            &dir,
            "argus.toml",
            r#"
[[mcp.servers]]
name = "metrics"
transport = { type = "stdio", command = "/usr/bin/prometheus-mcp", args = ["--flag"] }

[[mcp.servers.overrides]]
name = "query"
risk = "controlled"

[[mcp.servers]]
name = "search"
enabled = true
timeout_seconds = 120
transport = { type = "http", url = "https://mcp.internal/search" }
tools = ["query"]
"#,
        );
        let loaded = load(Some(&path)).expect("valid");
        assert_eq!(loaded.config.mcp.servers.len(), 2);

        let metrics = &loaded.config.mcp.servers[0];
        assert_eq!(metrics.name, "metrics");
        assert!(!metrics.enabled, "disabled is the default");
        assert_eq!(metrics.timeout_seconds, 30);
        assert_eq!(
            metrics.transport,
            McpTransportConfig::Stdio {
                command: "/usr/bin/prometheus-mcp".to_string(),
                args: vec!["--flag".to_string()],
            }
        );
        assert_eq!(metrics.overrides.len(), 1);
        assert_eq!(metrics.overrides[0].name, "query");
        assert_eq!(
            metrics.overrides[0].risk,
            argus_domain::RiskClass::Controlled
        );

        let search = &loaded.config.mcp.servers[1];
        assert!(search.enabled);
        assert_eq!(search.timeout_seconds, 120);
        assert_eq!(
            search.transport,
            McpTransportConfig::Http {
                url: "https://mcp.internal/search".to_string()
            }
        );
        assert_eq!(
            search.tools.as_deref(),
            Some(["query".to_string()].as_slice())
        );

        // The timeout clamps to the documented bounds (5–300).
        let path = write_config(
            &dir,
            "argus.toml",
            "[[mcp.servers]]\nname = \"x\"\ntimeout_seconds = 1\ntransport = { type = \"stdio\", command = \"x\" }\n",
        );
        let loaded = load(Some(&path)).expect("valid");
        assert_eq!(loaded.config.mcp.servers[0].timeout_seconds, 5);
        let path = write_config(
            &dir,
            "argus.toml",
            "[[mcp.servers]]\nname = \"x\"\ntimeout_seconds = 10_000\ntransport = { type = \"stdio\", command = \"x\" }\n",
        );
        let loaded = load(Some(&path)).expect("valid");
        assert_eq!(loaded.config.mcp.servers[0].timeout_seconds, 300);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_mcp_section_fails_closed_on_unrepresentable_declarations() {
        let dir = temp_dir();

        // A dotted name would break the no-collision namespacing.
        let path = write_config(
            &dir,
            "argus.toml",
            "[[mcp.servers]]\nname = \"my.server\"\ntransport = { type = \"stdio\", command = \"x\" }\n",
        );
        let err = load(Some(&path)).expect_err("a dotted server name is invalid");
        assert!(matches!(err, ConfigError::Mcp(_)), "{err:?}");

        // Duplicate names.
        let path = write_config(
            &dir,
            "argus.toml",
            "[[mcp.servers]]\nname = \"dup\"\ntransport = { type = \"stdio\", command = \"x\" }\n\n[[mcp.servers]]\nname = \"dup\"\ntransport = { type = \"stdio\", command = \"y\" }\n",
        );
        let err = load(Some(&path)).expect_err("duplicate names are invalid");
        assert!(matches!(err, ConfigError::Mcp(_)), "{err:?}");

        // An http url that is not an url.
        let path = write_config(
            &dir,
            "argus.toml",
            "[[mcp.servers]]\nname = \"srv\"\ntransport = { type = \"http\", url = \"not-an-url\" }\n",
        );
        let err = load(Some(&path)).expect_err("a non-url is invalid");
        assert!(matches!(err, ConfigError::Mcp(_)), "{err:?}");

        // An empty stdio command.
        let path = write_config(
            &dir,
            "argus.toml",
            "[[mcp.servers]]\nname = \"srv\"\ntransport = { type = \"stdio\", command = \" \" }\n",
        );
        let err = load(Some(&path)).expect_err("an empty command is invalid");
        assert!(matches!(err, ConfigError::Mcp(_)), "{err:?}");

        // An override naming an unrepresentable tool.
        let path = write_config(
            &dir,
            "argus.toml",
            "[[mcp.servers]]\nname = \"srv\"\ntransport = { type = \"stdio\", command = \"x\" }\n\n[[mcp.servers.overrides]]\nname = \"bad.name\"\nrisk = \"controlled\"\n",
        );
        let err = load(Some(&path)).expect_err("a dotted tool override is invalid");
        assert!(matches!(err, ConfigError::Mcp(_)), "{err:?}");

        // An unknown risk class.
        let path = write_config(
            &dir,
            "argus.toml",
            "[[mcp.servers]]\nname = \"srv\"\ntransport = { type = \"stdio\", command = \"x\" }\n\n[[mcp.servers.overrides]]\nname = \"tool\"\nrisk = \"trusted\"\n",
        );
        let err = load(Some(&path)).expect_err("an unknown risk class is invalid");
        assert!(
            matches!(err, ConfigError::Parse { .. } | ConfigError::Mcp(_)),
            "{err:?}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn enabled_servers_exposes_only_the_enabled_half() {
        let config = McpConfig {
            servers: vec![
                McpServerConfig {
                    name: "off".to_string(),
                    transport: McpTransportConfig::Stdio {
                        command: "x".to_string(),
                        args: vec![],
                    },
                    enabled: false,
                    timeout_seconds: 30,
                    tools: None,
                    overrides: vec![],
                },
                McpServerConfig {
                    name: "on".to_string(),
                    transport: McpTransportConfig::Http {
                        url: "https://x".to_string(),
                    },
                    enabled: true,
                    timeout_seconds: 30,
                    tools: None,
                    overrides: vec![],
                },
            ],
        };
        let enabled: Vec<&str> = config.enabled_servers().map(|s| s.name.as_str()).collect();
        assert_eq!(enabled, vec!["on"]);
    }
}
