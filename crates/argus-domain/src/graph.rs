//! Typed graph vocabulary for the operational environment graph (ADR-0033).
//!
//! Node and relationship kinds are domain enums — never strings — so the graph
//! and its traversals stay type-checked.

use serde::{Deserialize, Serialize};

/// The kinds of nodes in the operational environment graph (glossary.md §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Environment,
    Host,
    Kernel,
    Process,
    Cgroup,
    Namespace,
    Device,
    Filesystem,
    Network,
    Service,
    Container,
    Workload,
    Endpoint,
    Dependency,
    Application,
    K8sCluster,
    K8sNode,
    K8sNamespace,
    K8sDeployment,
    K8sReplicaSet,
    K8sPod,
    CloudResource,
    Observation,
    Situation,
    Incident,
    Evidence,
    Plan,
    Action,
    Execution,
    Agent,
    Plugin,
}

/// The kinds of relationships in the operational environment graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationshipKind {
    BelongsTo,
    RunsOn,
    DependsOn,
    Provides,
    Calls,
    Contains,
    Owns,
    ManagedBy,
    Exposes,
    ConnectedTo,
    CausedBy,
    AffectedBy,
    DeployedBy,
    DerivedFrom,
}

impl RelationshipKind {
    /// True for edges that point from a resource up to its owner/ancestor
    /// (`Pod ManagedBy Deployment`, `Container BelongsTo Pod`, `Process RunsOn Host`).
    pub fn is_ownership(&self) -> bool {
        matches!(
            self,
            Self::RunsOn | Self::BelongsTo | Self::ManagedBy | Self::DeployedBy
        )
    }

    /// True for edges that express a dependency (`App DependsOn Db`).
    pub fn is_dependency(&self) -> bool {
        matches!(self, Self::DependsOn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_and_dependency_kinds_are_classified() {
        assert!(RelationshipKind::ManagedBy.is_ownership());
        assert!(RelationshipKind::BelongsTo.is_ownership());
        assert!(RelationshipKind::RunsOn.is_ownership());
        assert!(RelationshipKind::DependsOn.is_dependency());
        assert!(!RelationshipKind::Contains.is_ownership());
        assert!(!RelationshipKind::DependsOn.is_ownership());
    }

    #[test]
    fn node_kind_serde_round_trips() {
        let json = serde_json::to_string(&NodeKind::K8sDeployment).unwrap();
        assert_eq!(json, "\"k8s_deployment\"");
        let back: NodeKind = serde_json::from_str(&json).unwrap();
        assert_eq!(back, NodeKind::K8sDeployment);
    }
}
