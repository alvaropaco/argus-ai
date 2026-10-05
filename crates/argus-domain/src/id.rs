//! Strongly-typed domain identifiers.

use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::DomainError;
use crate::validate::{is_dotted_path, is_kind};

/// The top-level operational boundary identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EnvironmentId(Uuid);

impl EnvironmentId {
    /// Generates a new random environment id.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Wraps an existing UUID (useful for fixtures and persistence round-trips).
    pub fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }

    /// Returns the underlying UUID.
    pub fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for EnvironmentId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for EnvironmentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A stable identifier for any resource in the correlation graph, of the form
/// `{kind}:{identifier}` (for example `host:<machine_id>`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ResourceId(String);

impl ResourceId {
    /// Constructs a resource id, validating the `kind` and `identifier` parts.
    pub fn new(kind: &str, identifier: &str) -> Result<Self, DomainError> {
        let valid = is_kind(kind)
            && !identifier.is_empty()
            && !identifier.chars().any(char::is_control)
            && !identifier.chars().any(char::is_whitespace);
        if !valid {
            return Err(DomainError::InvalidResourceId(format!(
                "{kind}:{identifier}"
            )));
        }
        Ok(Self(format!("{kind}:{identifier}")))
    }

    /// The resource kind (the part before `:`).
    pub fn kind(&self) -> &str {
        self.0.split_once(':').map(|(k, _)| k).unwrap_or(&self.0)
    }

    /// The resource identifier (the part after `:`).
    pub fn identifier(&self) -> &str {
        self.0.split_once(':').map(|(_, id)| id).unwrap_or(&self.0)
    }

    /// The full string representation.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ResourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A typed, policy-checkable operation identifier, e.g. `host.status.read`.
///
/// Deserialization is validated: a serialized string cannot construct a
/// `CapabilityId` by bypassing the dotted-path grammar (ADR-0027 §3).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct CapabilityId(String);

impl CapabilityId {
    /// Bootstrap capability: read-only host status.
    pub const HOST_STATUS_READ: &'static str = "host.status.read";
    /// Bootstrap capability: read-only ARGUS health.
    pub const ARGUS_HEALTH_READ: &'static str = "argus.health.read";
    /// Bootstrap capability: read-only non-secret configuration.
    pub const ARGUS_CONFIG_READ: &'static str = "argus.config.read";
    /// Bootstrap capability: list plugins.
    pub const ARGUS_PLUGINS_LIST: &'static str = "argus.plugins.list";

    /// Executable capability: restart a systemd service.
    pub const HOST_SERVICE_RESTART: &'static str = "host.service.restart";
    /// Executable capability: stop a systemd service.
    pub const HOST_SERVICE_STOP: &'static str = "host.service.stop";
    /// Executable capability: start a systemd service.
    pub const HOST_SERVICE_START: &'static str = "host.service.start";

    /// Remediation capability (spec 003 M3): deliver a signal to a process.
    pub const HOST_PROCESS_SIGNAL: &'static str = "host.process.signal";
    /// Remediation capability (spec 003 M3): restart a container.
    pub const CONTAINER_RESTART: &'static str = "container.restart";
    /// Resource-control capability (spec 003 M3, FR-017): freeze a cgroup v2
    /// subtree — the policy-approved throttle applied to a non-critical
    /// consumer under pressure.
    pub const HOST_CGROUP_FREEZE: &'static str = "host.cgroup.freeze";
    /// Resource-control capability (spec 003 M3, FR-017): thaw a cgroup v2
    /// subtree; the declared rollback of a freeze.
    pub const HOST_CGROUP_THAW: &'static str = "host.cgroup.thaw";

    /// Read-only Kubernetes intelligence (spec 003 M4, FR-014): cluster summary.
    pub const K8S_CLUSTER_READ: &'static str = "k8s.cluster.read";
    /// Read-only Kubernetes intelligence: nodes.
    pub const K8S_NODE_READ: &'static str = "k8s.node.read";
    /// Read-only Kubernetes intelligence: pods.
    pub const K8S_POD_READ: &'static str = "k8s.pod.read";
    /// Read-only Kubernetes intelligence: deployments.
    pub const K8S_DEPLOYMENT_READ: &'static str = "k8s.deployment.read";

    /// Self-healing capability (spec 003 M4, FR-016): delete a pod so its
    /// controller reschedules it.
    pub const K8S_POD_RESTART: &'static str = "k8s.pod.restart";
    /// Self-healing capability: delete a pod outright (high risk; denied
    /// outright until a policy explicitly permits it).
    pub const K8S_POD_DELETE: &'static str = "k8s.pod.delete";
    /// Self-healing capability: rollout-restart a deployment.
    pub const K8S_DEPLOYMENT_RESTART: &'static str = "k8s.deployment.restart";
    /// Self-healing capability: roll a deployment back to a prior revision.
    pub const K8S_DEPLOYMENT_ROLLBACK: &'static str = "k8s.deployment.rollback";
    /// Self-healing capability: scale a workload, bounded by quota.
    pub const K8S_WORKLOAD_SCALE: &'static str = "k8s.workload.scale";
    /// Self-healing capability: cordon a node.
    pub const K8S_NODE_CORDON: &'static str = "k8s.node.cordon";
    /// Self-healing capability: uncordon a node.
    pub const K8S_NODE_UNCORDON: &'static str = "k8s.node.uncordon";
    /// Self-healing capability: clean up completed/failed jobs.
    pub const K8S_JOB_CLEANUP: &'static str = "k8s.job.cleanup";

    /// Higher-risk Kubernetes actions that remain **deny-by-default and
    /// deferred** (ADR-0037 §4): no policy permits them and no executor
    /// performs them in this milestone.
    pub const K8S_NODE_DRAIN: &'static str = "k8s.node.drain";
    /// See [`CapabilityId::K8S_NODE_DRAIN`].
    pub const K8S_WORKLOAD_RESCHEDULE: &'static str = "k8s.workload.reschedule";

    /// Constructs a capability id, validating the dotted-path grammar.
    pub fn new(path: &str) -> Result<Self, DomainError> {
        if !is_dotted_path(path) {
            return Err(DomainError::InvalidCapabilityId(path.to_string()));
        }
        Ok(Self(path.to_string()))
    }

    /// The full dotted capability path.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for CapabilityId {
    type Error = DomainError;

    /// Validated construction on the deserialization path, so a serialized
    /// string cannot bypass the dotted-path grammar (ADR-0027 §3).
    fn try_from(value: String) -> Result<Self, Self::Error> {
        CapabilityId::new(&value)
    }
}

impl fmt::Display for CapabilityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_id_round_trips_uuid() {
        let id = Uuid::new_v4();
        let env = EnvironmentId::from_uuid(id);
        assert_eq!(env.as_uuid(), id);
    }

    #[test]
    fn environment_id_new_is_unique() {
        assert_ne!(EnvironmentId::new(), EnvironmentId::new());
    }

    #[test]
    fn resource_id_valid() {
        let id = ResourceId::new("host", "abc123").expect("valid resource id");
        assert_eq!(id.kind(), "host");
        assert_eq!(id.identifier(), "abc123");
        assert_eq!(id.as_str(), "host:abc123");
    }

    #[test]
    fn resource_id_rejects_bad_kinds() {
        assert!(ResourceId::new("", "x").is_err());
        assert!(ResourceId::new("Host", "x").is_err());
        assert!(ResourceId::new("1host", "x").is_err());
        assert!(ResourceId::new("-host", "x").is_err());
    }

    #[test]
    fn resource_id_rejects_bad_identifiers() {
        assert!(ResourceId::new("host", "").is_err());
        assert!(ResourceId::new("host", "has space").is_err());
        assert!(ResourceId::new("host", "ctrl\nchar").is_err());
    }

    #[test]
    fn capability_id_valid() {
        let id = CapabilityId::new("host.status.read").expect("valid capability");
        assert_eq!(id.as_str(), "host.status.read");
    }

    #[test]
    fn capability_id_rejects_invalid() {
        assert!(CapabilityId::new("host").is_err());
        assert!(CapabilityId::new("Host.status").is_err());
        assert!(CapabilityId::new("host.").is_err());
        assert!(CapabilityId::new("host.1status").is_err());
    }

    #[test]
    fn capability_constants_are_valid() {
        for path in [
            CapabilityId::HOST_STATUS_READ,
            CapabilityId::ARGUS_HEALTH_READ,
            CapabilityId::ARGUS_CONFIG_READ,
            CapabilityId::ARGUS_PLUGINS_LIST,
        ] {
            assert!(CapabilityId::new(path).is_ok(), "{path} should be valid");
        }
    }

    #[test]
    fn capability_id_deserializes_through_the_validator() {
        let id: CapabilityId =
            serde_json::from_str("\"host.status.read\"").expect("a valid path deserializes");
        assert_eq!(id.as_str(), "host.status.read");
    }

    #[test]
    fn capability_id_rejects_invalid_paths_on_deserialize() {
        for path in ["host", "Host.status", "host."] {
            let json = serde_json::to_string(path).unwrap();
            assert!(
                serde_json::from_str::<CapabilityId>(&json).is_err(),
                "{path} must not deserialize into a CapabilityId"
            );
        }
    }
}
