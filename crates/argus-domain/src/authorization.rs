//! Policy and execution boundary contracts.
//!
//! These types cross the authorization boundary: an agent or client proposes a
//! [`CapabilityRequest`], the policy engine evaluates an [`AuthorizationRequest`],
//! and the result is a [`PolicyDecision`]. The executor only ever runs an action
//! that carries an `Allow` decision.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{CapabilityId, Principal, RequestContext, ResourceId, RiskClass};

/// A typed request to invoke a capability.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)] // arguments is serde_json::Value
pub struct CapabilityRequest {
    pub capability: CapabilityId,
    pub principal: Principal,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<ResourceId>,
    #[serde(default)]
    pub arguments: Value,
    pub context: RequestContext,
}

impl CapabilityRequest {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        capability: CapabilityId,
        principal: Principal,
        resource: Option<ResourceId>,
        arguments: Value,
        context: RequestContext,
    ) -> Self {
        Self {
            capability,
            principal,
            resource,
            arguments,
            context,
        }
    }
}

/// The scope of impact of an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlastRadius {
    None,
    Host,
    Environment,
    Fleet,
}

/// A request to authorize a capability invocation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)]
pub struct AuthorizationRequest {
    pub capability_request: CapabilityRequest,
    pub risk_class: RiskClass,
    pub blast_radius: BlastRadius,
}

impl AuthorizationRequest {
    pub fn new(
        capability_request: CapabilityRequest,
        risk_class: RiskClass,
        blast_radius: BlastRadius,
    ) -> Self {
        Self {
            capability_request,
            risk_class,
            blast_radius,
        }
    }
}

/// The outcome of a policy evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyOutcome {
    Allow,
    Deny,
    RequireApproval,
}

/// A deterministic, auditable policy decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyDecision {
    pub outcome: PolicyOutcome,
    pub reason: String,
    pub policy_id: String,
    pub decided_at: DateTime<Utc>,
}

impl PolicyDecision {
    pub fn allow(policy_id: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            outcome: PolicyOutcome::Allow,
            reason: reason.into(),
            policy_id: policy_id.into(),
            decided_at: Utc::now(),
        }
    }

    pub fn deny(policy_id: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            outcome: PolicyOutcome::Deny,
            reason: reason.into(),
            policy_id: policy_id.into(),
            decided_at: Utc::now(),
        }
    }

    pub fn require_approval(policy_id: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            outcome: PolicyOutcome::RequireApproval,
            reason: reason.into(),
            policy_id: policy_id.into(),
            decided_at: Utc::now(),
        }
    }

    pub fn is_allowed(&self) -> bool {
        self.outcome == PolicyOutcome::Allow
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CapabilityId, Principal, RequestContext};
    use chrono::TimeZone;
    use semver::Version;
    use uuid::Uuid;

    fn context() -> RequestContext {
        RequestContext::new(
            Uuid::new_v4(),
            Version::new(0, 1, 0),
            Principal::new(Some(1000), Some(1000)),
            Utc.with_ymd_and_hms(2026, 9, 16, 12, 0, 0).unwrap(),
        )
    }

    #[test]
    fn capability_request_serde_round_trip() {
        let req = CapabilityRequest::new(
            CapabilityId::new("argus.health.read").unwrap(),
            Principal::new(Some(1000), None),
            None,
            serde_json::json!({}),
            context(),
        );
        let json = serde_json::to_string(&req).unwrap();
        let back: CapabilityRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn policy_outcome_serde_is_snake_case() {
        assert_eq!(
            serde_json::to_string(&PolicyOutcome::RequireApproval).unwrap(),
            "\"require_approval\""
        );
    }

    #[test]
    fn decision_allow_is_allowed() {
        let d = PolicyDecision::allow("p", "ok");
        assert!(d.is_allowed());
        assert_eq!(d.outcome, PolicyOutcome::Allow);
    }

    #[test]
    fn decision_deny_is_not_allowed() {
        let d = PolicyDecision::deny("p", "nope");
        assert!(!d.is_allowed());
        assert_eq!(d.outcome, PolicyOutcome::Deny);
    }
}
