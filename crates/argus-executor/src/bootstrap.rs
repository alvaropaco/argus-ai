//! Bootstrap executor: implements the four read-only capabilities.

use std::sync::Arc;

use argus_domain::CapabilityId;
use chrono::Utc;
use serde_json::json;

use crate::action::{AuthorizedAction, ExecutionError, ExecutionResult};
use crate::executor::Executor;
use crate::provider::CapabilityProvider;

/// Executes the read-only bootstrap capabilities against a [`CapabilityProvider`].
pub struct BootstrapExecutor {
    provider: Arc<dyn CapabilityProvider>,
}

impl BootstrapExecutor {
    pub fn new(provider: Arc<dyn CapabilityProvider>) -> Self {
        Self { provider }
    }
}

impl Executor for BootstrapExecutor {
    fn execute(&self, action: &AuthorizedAction) -> Result<ExecutionResult, ExecutionError> {
        let capability = action.capability();
        let started_at = Utc::now();

        let evidence = match capability.as_str() {
            CapabilityId::HOST_STATUS_READ => self.provider.host_status(),
            CapabilityId::ARGUS_HEALTH_READ => json!({ "health": self.provider.health() }),
            CapabilityId::ARGUS_CONFIG_READ => self.provider.config(),
            CapabilityId::ARGUS_PLUGINS_LIST => self.provider.plugins(),
            _ => return Err(ExecutionError::Unsupported(capability.clone())),
        };

        Ok(ExecutionResult {
            capability: capability.clone(),
            evidence,
            started_at,
            finished_at: Utc::now(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use argus_domain::{
        AuthorizationRequest, BlastRadius, CapabilityId, CapabilityRequest, HealthStatus,
        PolicyDecision, PolicyOutcome, Principal, RequestContext, RiskClass,
    };
    use chrono::Utc;
    use semver::Version;
    use serde_json::{Value, json};
    use uuid::Uuid;

    use super::*;

    struct MockProvider;

    impl CapabilityProvider for MockProvider {
        fn health(&self) -> HealthStatus {
            HealthStatus::ready(Utc::now())
        }
        fn config(&self) -> Value {
            json!({ "environment_name": "test" })
        }
        fn plugins(&self) -> Value {
            json!([])
        }
        fn host_status(&self) -> Value {
            json!({ "status": "unavailable" })
        }
        fn list_capabilities(&self) -> Vec<CapabilityId> {
            vec![CapabilityId::new(CapabilityId::ARGUS_HEALTH_READ).unwrap()]
        }
    }

    fn context() -> RequestContext {
        RequestContext::new(
            Uuid::new_v4(),
            Version::new(0, 1, 0),
            Principal::new(Some(1000), Some(1000)),
            Utc::now(),
        )
    }

    fn allowed_action(capability: &str) -> AuthorizedAction {
        let request = CapabilityRequest::new(
            CapabilityId::new(capability).unwrap(),
            Principal::new(Some(1000), Some(1000)),
            None,
            json!({}),
            context(),
        );
        let decision = PolicyDecision::allow("test", "ok");
        AuthorizedAction::new(request, decision).expect("action is allowed")
    }

    #[test]
    fn denied_decision_cannot_build_action() {
        let request = CapabilityRequest::new(
            CapabilityId::new("argus.health.read").unwrap(),
            Principal::new(Some(1000), Some(1000)),
            None,
            json!({}),
            context(),
        );
        let decision = PolicyDecision::deny("test", "denied");
        let err = AuthorizedAction::new(request, decision).unwrap_err();
        assert!(matches!(
            err,
            ExecutionError::NotAuthorized(PolicyOutcome::Deny)
        ));
    }

    #[test]
    fn require_approval_cannot_build_action() {
        let request = CapabilityRequest::new(
            CapabilityId::new("argus.health.read").unwrap(),
            Principal::new(Some(1000), Some(1000)),
            None,
            json!({}),
            context(),
        );
        let decision = PolicyDecision::require_approval("test", "needs approval");
        let err = AuthorizedAction::new(request, decision).unwrap_err();
        assert!(matches!(
            err,
            ExecutionError::NotAuthorized(PolicyOutcome::RequireApproval)
        ));
    }

    #[test]
    fn read_only_capability_executes() {
        let executor = BootstrapExecutor::new(Arc::new(MockProvider));
        let action = allowed_action(CapabilityId::ARGUS_HEALTH_READ);
        let result = executor.execute(&action).unwrap();
        assert_eq!(result.capability.as_str(), CapabilityId::ARGUS_HEALTH_READ);
        assert!(result.evidence["health"]["state"].is_string());
    }

    #[test]
    fn unsupported_capability_is_rejected() {
        let executor = BootstrapExecutor::new(Arc::new(MockProvider));
        // `host.process.signal` is a valid id but not supported by the bootstrap executor.
        let action = allowed_action("host.process.signal");
        let err = executor.execute(&action).unwrap_err();
        assert!(matches!(
            err,
            ExecutionError::Unsupported(c) if c.as_str() == "host.process.signal"
        ));
    }

    #[test]
    fn authorization_request_round_trips_through_policy_types() {
        let authz = AuthorizationRequest::new(
            CapabilityRequest::new(
                CapabilityId::new("argus.config.read").unwrap(),
                Principal::new(Some(0), Some(0)),
                None,
                json!({}),
                context(),
            ),
            RiskClass::Read,
            BlastRadius::None,
        );
        assert_eq!(authz.risk_class, RiskClass::Read);
        assert_eq!(authz.blast_radius, BlastRadius::None);
    }
}
