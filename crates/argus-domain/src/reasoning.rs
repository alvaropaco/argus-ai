//! Reasoning entities: the typed output of the AI reasoning layer.
//!
//! These types are the structured result of the reasoning gateway. They are
//! data, never authority: they flow through policy before any execution.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{BlastRadius, CapabilityId, ResourceId};

/// The configured level of autonomous authority (ADR-0035): L0 Observe
/// through L5 Adaptive. A higher level grants more *autonomy* — what may run
/// automatically — never more privilege: the security boundary, policy engine,
/// executor, validation, and audit are identical at every level.
///
/// Serde compatibility: the spec-002 three-mode values persist
/// (`observe_only` → L0, `propose` → L2, `assisted` → L3) so previously
/// written configurations still parse; serialization emits the canonical
/// `l0_observe`…`l5_adaptive` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AutonomyMode {
    /// L0 — reason, but never propose execution. The default until an
    /// operator explicitly raises it.
    #[default]
    L0Observe,
    /// L1 — explain what is happening; no proposed actions surface.
    L1Explain,
    /// L2 — produce plans that require operator approval.
    L2Recommend,
    /// L3 — execute only explicitly permitted low-risk actions; the rest
    /// require approval.
    L3Assisted,
    /// L4 — bounded autonomous execution of policy-allowed controlled-risk
    /// actions; high-risk and irreversible stay approval-gated.
    L4Autonomous,
    /// L5 — L4 plus gated learning (candidate runbooks proposed for the
    /// promotion ladder); the action boundary is identical to L4.
    L5Adaptive,
}

impl AutonomyMode {
    /// The spec-002 three-mode names, for compatibility with written state.
    pub fn from_legacy_name(name: &str) -> Option<Self> {
        match name {
            "observe_only" => Some(Self::L0Observe),
            "propose" => Some(Self::L2Recommend),
            "assisted" => Some(Self::L3Assisted),
            _ => None,
        }
    }

    /// The canonical snake_case name (`l0_observe` … `l5_adaptive`).
    pub fn canonical_name(&self) -> &'static str {
        match self {
            Self::L0Observe => "l0_observe",
            Self::L1Explain => "l1_explain",
            Self::L2Recommend => "l2_recommend",
            Self::L3Assisted => "l3_assisted",
            Self::L4Autonomous => "l4_autonomous",
            Self::L5Adaptive => "l5_adaptive",
        }
    }
}

impl Serialize for AutonomyMode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.canonical_name())
    }
}

impl<'de> Deserialize<'de> for AutonomyMode {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        // Accept both the canonical names and the spec-002 legacy names.
        if let Some(mode) = Self::from_legacy_name(&name) {
            return Ok(mode);
        }
        match name.as_str() {
            "l0_observe" => Ok(Self::L0Observe),
            "l1_explain" => Ok(Self::L1Explain),
            "l2_recommend" => Ok(Self::L2Recommend),
            "l3_assisted" => Ok(Self::L3Assisted),
            "l4_autonomous" => Ok(Self::L4Autonomous),
            "l5_adaptive" => Ok(Self::L5Adaptive),
            other => Err(serde::de::Error::unknown_variant(
                other,
                &[
                    "l0_observe",
                    "l1_explain",
                    "l2_recommend",
                    "l3_assisted",
                    "l4_autonomous",
                    "l5_adaptive",
                    "observe_only",
                    "propose",
                    "assisted",
                ],
            )),
        }
    }
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
///
/// Serialization emits the current shape (`steps`); deserialization additionally
/// accepts the pre-ADR-0028 shape (`actions` + a top-level `rollback` marker) so a
/// plan persisted before the step contract can still be read back.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Plan {
    pub objective: String,
    pub steps: Vec<PlanStep>,
    pub preconditions: Vec<String>,
    pub expected_outcomes: Vec<String>,
    pub blast_radius: BlastRadius,
    pub confidence: f64,
    pub status: PlanStatus,
}

impl<'de> Deserialize<'de> for Plan {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct PlanWire {
            objective: String,
            #[serde(default)]
            steps: Option<Vec<PlanStep>>,
            /// The pre-ADR-0028 shape: a flat `actions` list plus a top-level
            /// `rollback` marker. Each action migrates to a `PlanStep` with no
            /// rollback; the marker was a description string, never an
            /// executable action, so it is dropped rather than invented back.
            #[serde(default)]
            actions: Option<Vec<Action>>,
            preconditions: Vec<String>,
            expected_outcomes: Vec<String>,
            blast_radius: BlastRadius,
            confidence: f64,
            status: PlanStatus,
        }

        let wire = PlanWire::deserialize(deserializer)?;
        let steps = match (wire.steps, wire.actions) {
            (Some(steps), _) => steps,
            (None, Some(actions)) => actions
                .into_iter()
                .map(|action| PlanStep {
                    action,
                    rollback: None,
                })
                .collect(),
            (None, None) => {
                return Err(serde::de::Error::custom(
                    "plan must carry `steps` (or the legacy `actions`)",
                ));
            }
        };
        Ok(Plan {
            objective: wire.objective,
            steps,
            preconditions: wire.preconditions,
            expected_outcomes: wire.expected_outcomes,
            blast_radius: wire.blast_radius,
            confidence: wire.confidence,
            status: wire.status,
        })
    }
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
    fn autonomy_mode_defaults_to_the_most_conservative_level() {
        assert_eq!(AutonomyMode::default(), AutonomyMode::L0Observe);
    }

    #[test]
    fn autonomy_mode_serde_round_trip_and_legacy_compat() {
        // Canonical serialization.
        let json = serde_json::to_string(&AutonomyMode::L3Assisted).unwrap();
        assert_eq!(json, "\"l3_assisted\"");
        let back: AutonomyMode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, AutonomyMode::L3Assisted);
        // Spec-002 legacy values still parse (ADR-0035 persisted-value map).
        for (legacy, expected) in [
            ("\"observe_only\"", AutonomyMode::L0Observe),
            ("\"propose\"", AutonomyMode::L2Recommend),
            ("\"assisted\"", AutonomyMode::L3Assisted),
        ] {
            let parsed: AutonomyMode = serde_json::from_str(legacy).unwrap();
            assert_eq!(parsed, expected);
        }
        // Unknown values still fail closed.
        assert!(serde_json::from_str::<AutonomyMode>("\"autonomous\"").is_err());
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
    fn a_legacy_actions_plan_deserializes_into_steps() {
        // The pre-ADR-0028 wire shape: a flat `actions` list and a top-level
        // `rollback` marker. It must read back as one step per action, with no
        // rollback, and the marker is dropped.
        let legacy = serde_json::json!({
            "objective": "restore nginx",
            "actions": [
                { "capability": "host.service.restart", "resource": null,
                  "arguments": { "unit": "nginx.service" } }
            ],
            "rollback": "restart",
            "preconditions": [],
            "expected_outcomes": ["nginx running"],
            "blast_radius": "host",
            "confidence": 0.9,
            "status": "proposed",
        });
        let plan: Plan = serde_json::from_value(legacy).unwrap();

        assert_eq!(plan.steps.len(), 1, "each legacy action becomes a step");
        assert_eq!(
            plan.steps[0].action.capability.as_str(),
            "host.service.restart"
        );
        assert!(
            plan.steps[0].rollback.is_none(),
            "the legacy marker is a description, not an executable rollback"
        );
        assert_eq!(plan.expected_outcomes, vec!["nginx running".to_string()]);
    }

    #[test]
    fn a_plan_serializes_in_the_current_steps_shape() {
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
            expected_outcomes: vec![],
            blast_radius: BlastRadius::Host,
            confidence: 0.9,
            status: PlanStatus::Proposed,
        };
        let json = serde_json::to_value(&plan).unwrap();
        assert!(json.get("steps").is_some(), "serialized with `steps`");
        assert!(
            json.get("actions").is_none(),
            "the legacy `actions` field is not emitted"
        );
        assert!(json.get("rollback").is_none());
    }

    #[test]
    fn a_plan_without_steps_or_actions_is_rejected() {
        // A truncated/corrupt plan carrying neither the current `steps` nor the
        // legacy `actions` must not silently deserialize into an empty no-op
        // plan; it is a serde error.
        let wire = serde_json::json!({
            "objective": "restore nginx",
            "preconditions": [],
            "expected_outcomes": [],
            "blast_radius": "host",
            "confidence": 0.9,
            "status": "proposed",
        });
        let err = serde_json::from_value::<Plan>(wire).unwrap_err();
        assert!(
            err.to_string().contains("`steps`"),
            "the rejection names the missing field: {err}"
        );
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
