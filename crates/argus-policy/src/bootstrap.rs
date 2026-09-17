//! Bootstrap policy: only the four read-only capabilities are permitted.

use std::collections::HashSet;

use argus_domain::{AuthorizationRequest, CapabilityId, PolicyDecision};

use crate::PolicyEvaluator;

/// The default policy for the bootstrap runtime.
///
/// It allows exactly the four read-only bootstrap capabilities and denies
/// everything else (including privileged operations) by default.
pub struct BootstrapPolicyEvaluator {
    allowed: HashSet<CapabilityId>,
}

impl BootstrapPolicyEvaluator {
    pub fn new() -> Self {
        let allowed = [
            CapabilityId::new(CapabilityId::HOST_STATUS_READ),
            CapabilityId::new(CapabilityId::ARGUS_HEALTH_READ),
            CapabilityId::new(CapabilityId::ARGUS_CONFIG_READ),
            CapabilityId::new(CapabilityId::ARGUS_PLUGINS_LIST),
        ]
        .into_iter()
        .map(|id| id.expect("bootstrap capability ids are valid"))
        .collect();

        Self { allowed }
    }

    /// The read-only capabilities the bootstrap policy permits.
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

        if self.allowed.contains(capability) {
            PolicyDecision::allow(
                "bootstrap.read-only",
                format!(
                    "read-only capability '{}' is permitted",
                    capability.as_str()
                ),
            )
        } else {
            PolicyDecision::deny(
                "bootstrap.read-only",
                format!(
                    "capability '{}' is not permitted in bootstrap",
                    capability.as_str()
                ),
            )
        }
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
}
