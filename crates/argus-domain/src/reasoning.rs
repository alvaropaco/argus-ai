//! Reasoning entities: the typed output of the AI reasoning layer.
//!
//! These types are the structured result of the reasoning gateway. They are
//! data, never authority: they flow through policy before any execution.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{BlastRadius, CapabilityId, ResourceId};

/// The configured level of autonomous authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutonomyMode {
    /// Reason, but never propose execution.
    ObserveOnly,
    /// Produce plans that require operator approval.
    #[default]
    Propose,
    /// Execute only explicitly permitted low-risk actions; the rest require approval.
    Assisted,
}

/// A desired condition over a resource (e.g. "service nginx is running").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Intent {
    pub resource: ResourceId,
    pub attribute: String,
    pub desired: Value,
}

/// Lifecycle status of a [`Hypothesis`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HypothesisStatus {
    Open,
    Confirmed,
    Rejected,
}

/// A proposed explanation, produced from structured decisions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hypothesis {
    pub statement: String,
    pub confidence: f64,
    pub supporting_evidence: Vec<ResourceId>,
    pub status: HypothesisStatus,
}

/// A typed, capability-backed operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Action {
    pub capability: CapabilityId,
    pub resource: Option<ResourceId>,
    pub arguments: Value,
}

/// Lifecycle status of a [`Plan`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Proposed,
    Approved,
    Denied,
    Superseded,
    Executing,
    Completed,
    Failed,
    RolledBack,
}

/// A proposed sequence of typed actions to move observed state toward desired state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub objective: String,
    pub actions: Vec<Action>,
    pub preconditions: Vec<String>,
    pub expected_outcomes: Vec<String>,
    pub rollback: Option<String>,
    pub blast_radius: BlastRadius,
    pub confidence: f64,
    pub status: PlanStatus,
}

/// Result of executing a single [`Action`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    Completed,
    Failed,
}

/// One attempted action and its outcome.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Execution {
    pub action: Action,
    pub status: ExecutionStatus,
    pub evidence: Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BlastRadius;

    #[test]
    fn autonomy_mode_defaults_to_conservative_propose() {
        assert_eq!(AutonomyMode::default(), AutonomyMode::Propose);
    }

    #[test]
    fn autonomy_mode_serde_round_trip() {
        let json = serde_json::to_string(&AutonomyMode::Assisted).unwrap();
        assert_eq!(json, "\"assisted\"");
        let back: AutonomyMode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, AutonomyMode::Assisted);
    }

    #[test]
    fn plan_serde_round_trip() {
        let plan = Plan {
            objective: "restore nginx".into(),
            actions: vec![Action {
                capability: CapabilityId::new("host.service.restart").unwrap(),
                resource: None,
                arguments: serde_json::json!({ "unit": "nginx.service" }),
            }],
            preconditions: vec![],
            expected_outcomes: vec!["nginx running".into()],
            rollback: None,
            blast_radius: BlastRadius::Host,
            confidence: 0.9,
            status: PlanStatus::Proposed,
        };
        let json = serde_json::to_string(&plan).unwrap();
        let back: Plan = serde_json::from_str(&json).unwrap();
        assert_eq!(plan, back);
    }

    #[test]
    fn hypothesis_status_and_confidence_round_trip() {
        let h = Hypothesis {
            statement: "nginx degraded due to memory pressure".into(),
            confidence: 0.85,
            supporting_evidence: vec![],
            status: HypothesisStatus::Confirmed,
        };
        let json = serde_json::to_string(&h).unwrap();
        let back: Hypothesis = serde_json::from_str(&json).unwrap();
        assert_eq!(h, back);
    }
}
