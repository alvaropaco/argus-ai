//! Plugin manifest schema (ADR-016) and TOML parsing.

use std::collections::BTreeMap;

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::CapabilityId;
use crate::error::DomainError;
use crate::validate::is_kind;

/// The category of a plugin/extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginType {
    Mcp,
    Adapter,
    Discovery,
    Provider,
    Policy,
    Telemetry,
    Runbook,
    Agent,
}

/// Compatibility constraints declared by a plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Compatibility {
    pub argus_min: Version,
}

/// A plugin's declarative manifest.
///
/// The manifest is descriptive, not an authorization grant: the policy engine
/// remains authoritative (ADR-016).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    pub version: Version,
    pub api_version: String,
    #[serde(rename = "type")]
    pub plugin_type: PluginType,
    #[serde(default)]
    pub capabilities: Vec<CapabilityId>,
    #[serde(default)]
    pub permissions: BTreeMap<String, bool>,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_check: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compatibility: Option<Compatibility>,
}

impl PluginManifest {
    /// Parses and validates a manifest from its TOML representation.
    pub fn from_toml(input: &str) -> Result<Self, DomainError> {
        let manifest: Self =
            toml::from_str(input).map_err(|e| DomainError::InvalidPluginManifest(e.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Validates the manifest's invariants.
    pub fn validate(&self) -> Result<(), DomainError> {
        if !is_kind(&self.name) {
            return Err(DomainError::InvalidPluginManifest(format!(
                "invalid plugin name '{}'",
                self.name
            )));
        }
        for capability in &self.capabilities {
            CapabilityId::new(capability.as_str())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"
name = "docker"
version = "1.0.0"
api_version = "1"
type = "mcp"
capabilities = ["container.list", "container.restart"]
health_check = "docker.health"

[permissions]
docker_socket = true
network = false
filesystem = false

[compatibility]
argus_min = "0.1.0"
"#;

    #[test]
    fn parses_valid_manifest() {
        let manifest = PluginManifest::from_toml(VALID).unwrap();
        assert_eq!(manifest.name, "docker");
        assert_eq!(manifest.version, Version::new(1, 0, 0));
        assert_eq!(manifest.plugin_type, PluginType::Mcp);
        assert_eq!(manifest.capabilities.len(), 2);
        assert!(manifest.permissions["docker_socket"]);
        assert_eq!(
            manifest.compatibility.as_ref().unwrap().argus_min,
            Version::new(0, 1, 0)
        );
    }

    #[test]
    fn rejects_invalid_name() {
        let bad = VALID.replace("docker", "Docker_Invalid");
        assert!(PluginManifest::from_toml(&bad).is_err());
    }

    #[test]
    fn rejects_invalid_capability() {
        let bad = VALID.replace("container.list", "Not.A.Capability");
        assert!(PluginManifest::from_toml(&bad).is_err());
    }

    #[test]
    fn rejects_malformed_toml() {
        assert!(PluginManifest::from_toml("not valid toml [").is_err());
    }

    #[test]
    fn defaults_are_empty() {
        let minimal = r#"
name = "x"
version = "1.0.0"
api_version = "1"
type = "mcp"
"#;
        let manifest = PluginManifest::from_toml(minimal).unwrap();
        assert!(manifest.capabilities.is_empty());
        assert!(manifest.permissions.is_empty());
        assert!(manifest.dependencies.is_empty());
        assert_eq!(manifest.health_check, None);
    }
}
