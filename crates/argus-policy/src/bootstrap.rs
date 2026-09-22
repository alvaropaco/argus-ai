//! Bootstrap policy: only the four read-only capabilities are permitted.

use std::collections::HashSet;

use argus_domain::{AuthorizationRequest, CapabilityId, PolicyDecision};

use crate::PolicyEvaluator;

/// The default policy for the runtime.
///
/// It allows the read-only bootstrap capabilities and the reversible, low-risk
/// service capabilities, and denies everything else (including privileged
/// operations) by default.
pub struct BootstrapPolicyEvaluator {
    allowed: HashSet<CapabilityId>,
}

impl BootstrapPolicyEvaluator {
    pub fn new() -> Self {
        let allowed = [
            CapabilityId::HOST_STATUS_READ,
            CapabilityId::ARGUS_HEALTH_READ,
            CapabilityId::ARGUS_CONFIG_READ,
            CapabilityId::ARGUS_PLUGINS_LIST,
            CapabilityId::HOST_SERVICE_RESTART,
            CapabilityId::HOST_SERVICE_STOP,
            CapabilityId::HOST_SERVICE_START,
        ]
        .into_iter()
        .map(|id| CapabilityId::new(id).expect("capability ids are valid"))
        .collect();

        Self { allowed }
    }

    /// The capabilities the policy permits (read-only and low-risk service actions).
    pub fn allowed_capabilities(&self) -> impl Iterator<Item = &CapabilityId> {
        self.allowed.iter()
    }
}

impl Default for BootstrapPolicyEvaluator {
    fn default() -> Self {
        Self::new()
    }
}

impl PolicyEvaluator for BootstrapPolicyEvaluator {
    fn evaluate(&self, request: &AuthorizationRequest) -> PolicyDecision {
        let capability = &request.capability_request.capability;

        if !self.allowed.contains(capability) {
            return PolicyDecision::deny(
                "bootstrap.read-only",
                format!(
                    "capability '{}' is not permitted in bootstrap",
                    capability.as_str()
                ),
            );
        }

        // The principal is deliberately never consulted. A cloud-issued request
        // arrives unattributed and gains no authority from its origin, so routing
        // it here is indistinguishable from any other unverified client
        // (ADR-0020 §1, §5).
        if request.requires_approval {
            return PolicyDecision::require_approval(
                "bootstrap.approval-required",
                format!(
                    "capability '{}' requires a local approval for each invocation",
                    capability.as_str()
                ),
            );
        }

        PolicyDecision::allow(
            "bootstrap.read-only",
            format!(
                "read-only capability '{}' is permitted",
                capability.as_str()
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use argus_domain::{
        AuthorizationRequest, BlastRadius, CapabilityId, CapabilityRequest, PolicyOutcome,
        Principal, RequestContext, RiskClass,
    };
    use chrono::Utc;
    use semver::Version;
    use uuid::Uuid;

    use super::*;

    fn context() -> RequestContext {
        RequestContext::new(
            Uuid::new_v4(),
            Version::new(0, 1, 0),
            Principal::new(Some(1000), Some(1000)),
            Utc::now(),
        )
    }

    fn request_for(capability: &str, risk: RiskClass) -> AuthorizationRequest {
        AuthorizationRequest::new(
            CapabilityRequest::new(
                CapabilityId::new(capability).unwrap(),
                Principal::new(Some(1000), Some(1000)),
                None,
                serde_json::json!({}),
                context(),
            ),
            risk,
            BlastRadius::None,
        )
    }

    #[test]
    fn read_only_capabilities_are_allowed() {
        let policy = BootstrapPolicyEvaluator::new();
        for capability in [
            CapabilityId::HOST_STATUS_READ,
            CapabilityId::ARGUS_HEALTH_READ,
            CapabilityId::ARGUS_CONFIG_READ,
            CapabilityId::ARGUS_PLUGINS_LIST,
        ] {
            let decision = policy.evaluate(&request_for(capability, RiskClass::Read));
            assert_eq!(decision.outcome, PolicyOutcome::Allow, "{capability}");
        }
    }

    #[test]
    fn privileged_capabilities_are_denied() {
        let policy = BootstrapPolicyEvaluator::new();
        let decision = policy.evaluate(&request_for("host.process.signal", RiskClass::HighRisk));
        assert_eq!(decision.outcome, PolicyOutcome::Deny);
    }

    #[test]
    fn low_risk_service_capabilities_are_allowed() {
        let policy = BootstrapPolicyEvaluator::new();
        for capability in [
            CapabilityId::HOST_SERVICE_RESTART,
            CapabilityId::HOST_SERVICE_STOP,
            CapabilityId::HOST_SERVICE_START,
        ] {
            let decision = policy.evaluate(&request_for(capability, RiskClass::LowRisk));
            assert_eq!(decision.outcome, PolicyOutcome::Allow, "{capability}");
        }
    }

    #[test]
    fn unknown_capabilities_are_denied() {
        let policy = BootstrapPolicyEvaluator::new();
        let decision = policy.evaluate(&request_for("container.restart", RiskClass::Controlled));
        assert_eq!(decision.outcome, PolicyOutcome::Deny);
    }

    #[test]
    fn denial_is_by_default_even_for_read_risk() {
        // A read-risk request for a capability that is not registered is still denied.
        let policy = BootstrapPolicyEvaluator::new();
        let decision = policy.evaluate(&request_for("host.filesystem.inspect", RiskClass::Read));
        assert_eq!(decision.outcome, PolicyOutcome::Deny);
    }

    #[test]
    fn a_capability_that_declares_approval_requires_it() {
        let policy = BootstrapPolicyEvaluator::new();
        let mut request = request_for(CapabilityId::HOST_SERVICE_RESTART, RiskClass::LowRisk);
        request = request.requiring_approval(true);

        let decision = policy.evaluate(&request);
        assert_eq!(decision.outcome, PolicyOutcome::RequireApproval);
        assert!(
            !decision.is_allowed(),
            "an approval-gated capability must not be allowed outright"
        );
    }

    #[test]
    fn approval_is_required_only_when_the_capability_declares_it() {
        let policy = BootstrapPolicyEvaluator::new();
        let decision =
            policy.evaluate(&request_for(CapabilityId::HOST_SERVICE_RESTART, RiskClass::LowRisk));
        assert_eq!(decision.outcome, PolicyOutcome::Allow);
    }

    #[test]
    fn an_unpermitted_capability_is_denied_before_approval_is_considered() {
        let policy = BootstrapPolicyEvaluator::new();
        let request = request_for("container.restart", RiskClass::Controlled);
        let request = request.requiring_approval(true);

        assert_eq!(
            policy.evaluate(&request).outcome,
            PolicyOutcome::Deny,
            "approval cannot promote a capability the policy does not permit"
        );
    }

    #[test]
    fn the_cloud_principal_gains_no_authority_from_its_origin() {
        let policy = BootstrapPolicyEvaluator::new();

        let local = AuthorizationRequest::new(
            CapabilityRequest::new(
                CapabilityId::new("host.process.signal").unwrap(),
                Principal::new(Some(1000), Some(1000)),
                None,
                serde_json::json!({}),
                context(),
            ),
            RiskClass::HighRisk,
            BlastRadius::Host,
        );
        let cloud = AuthorizationRequest::new(
            CapabilityRequest::new(
                CapabilityId::new("host.process.signal").unwrap(),
                Principal::cloud(),
                None,
                serde_json::json!({}),
                context(),
            ),
            RiskClass::HighRisk,
            BlastRadius::Host,
        );

        assert_eq!(policy.evaluate(&local).outcome, PolicyOutcome::Deny);
        assert_eq!(
            policy.evaluate(&cloud).outcome,
            PolicyOutcome::Deny,
            "the cloud is a request origin, never an authorization grant"
        );
    }

    #[test]
    fn the_cloud_principal_is_unattributed_locally() {
        let cloud = Principal::cloud();
        assert_eq!(cloud.uid(), None);
        assert_eq!(cloud.gid(), None);
    }
}
