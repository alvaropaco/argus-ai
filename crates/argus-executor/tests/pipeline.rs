//! End-to-end pipeline test: policy → executor, proving privileged actions
//! cannot bypass the boundary.

use std::sync::Arc;

use argus_domain::{
    AuthorizationRequest, BlastRadius, CapabilityId, CapabilityRequest, HealthStatus,
    PolicyOutcome, Principal, RequestContext, RiskClass,
};
use argus_executor::{
    AuthorizedAction, BootstrapExecutor, CapabilityProvider, ExecutionError, Executor,
};
use argus_policy::{BootstrapPolicyEvaluator, PolicyEvaluator};
use chrono::Utc;
use semver::Version;
use serde_json::{Value, json};
use uuid::Uuid;

struct MockProvider;

impl CapabilityProvider for MockProvider {
    fn health(&self) -> HealthStatus {
        HealthStatus::ready(Utc::now())
    }
    fn config(&self) -> Value {
        json!({})
    }
    fn plugins(&self) -> Value {
        json!([])
    }
    fn host_status(&self) -> Value {
        json!({ "status": "unavailable" })
    }
    fn list_capabilities(&self) -> Vec<CapabilityId> {
        vec![]
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

fn authorization(capability: &str, risk: RiskClass) -> AuthorizationRequest {
    AuthorizationRequest::new(
        CapabilityRequest::new(
            CapabilityId::new(capability).unwrap(),
            Principal::new(Some(1000), Some(1000)),
            None,
            json!({}),
            context(),
        ),
        risk,
        BlastRadius::None,
    )
}

#[test]
fn privileged_action_cannot_reach_executor() {
    let policy = BootstrapPolicyEvaluator::new();

    let authz = authorization("host.process.signal", RiskClass::HighRisk);
    let decision = policy.evaluate(&authz);
    assert_eq!(decision.outcome, PolicyOutcome::Deny);

    // Defense in depth: even if a caller ignored the decision, the executor
    // boundary refuses to construct an action from a non-allowed decision.
    let err = AuthorizedAction::new(authz.capability_request, decision).unwrap_err();
    assert!(matches!(
        err,
        ExecutionError::NotAuthorized(PolicyOutcome::Deny)
    ));
}

#[test]
fn read_only_action_flows_through_policy_and_executor() {
    let policy = BootstrapPolicyEvaluator::new();
    let executor = BootstrapExecutor::new(Arc::new(MockProvider));

    let authz = authorization("argus.health.read", RiskClass::Read);
    let decision = policy.evaluate(&authz);
    assert_eq!(decision.outcome, PolicyOutcome::Allow);

    let action = AuthorizedAction::new(authz.capability_request, decision).unwrap();
    let result = executor.execute(&action).unwrap();
    assert_eq!(result.capability.as_str(), "argus.health.read");
    assert!(result.evidence["health"]["state"].is_string());
}
