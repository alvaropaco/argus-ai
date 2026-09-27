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
    /// A failed plan's already-executed steps are being undone.
    RollingBack,
    RolledBack,
    /// A rollback itself failed; an operator must intervene.
    NeedsManual,
    /// A step requires operator approval; the plan is paused pending a grant.
    AwaitingApproval,
}

/// One step of a plan: the action to perform and its declarative rollback.
///
/// Every step carries the action that undoes it, when one exists (ADR-0028 §4).
/// The planner never substitutes actions mid-failure; recovery is re-authorization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanStep {
    pub action: Action,
    pub rollback: Option<Action>,
}

/// A proposed sequence of typed actions to move observed state toward desired state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub objective: String,
    pub steps: Vec<PlanStep>,
    pub preconditions: Vec<String>,
    pub expected_outcomes: Vec<String>,
    pub blast_radius: BlastRadius,
    pub confidence: f64,
    pub status: PlanStatus,
}

/// The deterministic context binding hash for a plan (ADR-0030 §3).
///
/// Hashes the plan's content — objective, steps, preconditions, expected
/// outcomes, blast radius, and confidence — never its lifecycle `status`, so
/// the digest is stable across a pause/resume round-trip. It shares the FNV-1a
/// 64-bit hex semantics of decision provenance
/// (`argus-ai-core::decision::provenance::DecisionProvenance::context_hash`),
/// so an approval bound to this digest is invalidated by any change to the plan
/// it reviewed.
pub fn plan_context_hash(plan: &Plan) -> String {
    let mut value = serde_json::to_value(plan).unwrap_or(serde_json::Value::Null);
    if let serde_json::Value::Object(map) = &mut value {
        map.remove("status");
    }
    let canonical = serde_json::to_string(&value).unwrap_or_default();
    fnv1a_hex(&canonical)
}

/// A deterministic FNV-1a (64-bit) hex digest of `input`.
fn fnv1a_hex(input: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in input.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
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
            steps: vec![PlanStep {
                action: Action {
                    capability: CapabilityId::new("host.service.restart").unwrap(),
                    resource: None,
                    arguments: serde_json::json!({ "unit": "nginx.service" }),
                },
                rollback: None,
            }],
            preconditions: vec![],
            expected_outcomes: vec!["nginx running".into()],
            blast_radius: BlastRadius::Host,
            confidence: 0.9,
            status: PlanStatus::Proposed,
        };
        let json = serde_json::to_string(&plan).unwrap();
        let back: Plan = serde_json::from_str(&json).unwrap();
        assert_eq!(plan, back);
    }

    #[test]
    fn plan_status_serde_includes_rollback_and_manual() {
        for (status, expected) in [
            (PlanStatus::RollingBack, "\"rolling_back\""),
            (PlanStatus::NeedsManual, "\"needs_manual\""),
        ] {
            assert_eq!(serde_json::to_string(&status).unwrap(), expected);
            let back: PlanStatus = serde_json::from_str(expected).unwrap();
            assert_eq!(back, status);
        }
    }

    #[test]
    fn plan_context_hash_is_stable_across_status_and_sensitive_to_content() {
        let mut plan = Plan {
            objective: "restore nginx".into(),
            steps: vec![],
            preconditions: vec![],
            expected_outcomes: vec![],
            blast_radius: BlastRadius::Host,
            confidence: 0.9,
            status: PlanStatus::Proposed,
        };
        let proposed = plan_context_hash(&plan);

        // A lifecycle change must not invalidate the binding hash.
        plan.status = PlanStatus::AwaitingApproval;
        assert_eq!(proposed, plan_context_hash(&plan));

        // A content change must.
        plan.objective = "restart postgres".into();
        assert_ne!(proposed, plan_context_hash(&plan));
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
