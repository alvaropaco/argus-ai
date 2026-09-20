//! Control loop: routes a proposed plan through policy, autonomy, and execution.

use argus_ai_core::decision::autonomy::may_execute_without_approval;
use argus_domain::{
    Action, AuthorizationRequest, AutonomyMode, CapabilityId, CapabilityRequest, DomainEvent,
    EventType, Execution, ExecutionStatus, Plan, PolicyOutcome, Principal, RequestContext,
    RiskClass, Severity,
};
use argus_events::{EventBus, types};
use argus_executor::ServiceController;
use argus_policy::PolicyEvaluator;
use chrono::Utc;
use semver::Version;
use serde_json::{Value, json};
use uuid::Uuid;

/// The result of routing a plan through the safety boundary.
#[derive(Debug, Default)]
pub struct ExecutionOutcome {
    pub executions: Vec<Execution>,
    pub denied: Vec<CapabilityId>,
    pub requires_approval: Vec<CapabilityId>,
}

/// Risk class for an executable capability. Service actions are low-risk and
/// reversible; anything unknown is treated as `Controlled` and therefore never
/// auto-executed.
fn risk_for(capability: &CapabilityId) -> RiskClass {
    match capability.as_str() {
        CapabilityId::HOST_SERVICE_RESTART
        | CapabilityId::HOST_SERVICE_STOP
        | CapabilityId::HOST_SERVICE_START => RiskClass::LowRisk,
        _ => RiskClass::Controlled,
    }
}

fn execute_action(action: &Action, service: &dyn ServiceController) -> Result<Value, String> {
    let unit = action
        .arguments
        .get("unit")
        .and_then(Value::as_str)
        .ok_or_else(|| "action missing 'unit' argument".to_string())?;

    let result = match action.capability.as_str() {
        CapabilityId::HOST_SERVICE_RESTART => service.restart(unit),
        CapabilityId::HOST_SERVICE_STOP => service.stop(unit),
        CapabilityId::HOST_SERVICE_START => service.start(unit),
        other => return Err(format!("unsupported executable capability '{other}'")),
    };

    result
        .map(|_| json!({ "unit": unit, "capability": action.capability.as_str() }))
        .map_err(|e| e.to_string())
}

/// Routes every action in `plan` through policy, then autonomy, then execution.
///
/// This is the only path from a proposed plan to execution (FR-004, FR-005).
pub async fn authorize_and_run(
    plan: &Plan,
    policy: &dyn PolicyEvaluator,
    service: &dyn ServiceController,
    events: &dyn EventBus,
    autonomy: AutonomyMode,
) -> ExecutionOutcome {
    let correlation = Uuid::new_v4();
    let _ = events
        .publish(&event(
            types::PLAN_PROPOSED,
            json!({ "objective": plan.objective, "actions": plan.actions.len() }),
            correlation,
            None,
        ))
        .await;

    let mut outcome = ExecutionOutcome::default();

    for action in &plan.actions {
        let risk = risk_for(&action.capability);
        let authz = AuthorizationRequest::new(
            CapabilityRequest::new(
                action.capability.clone(),
                Principal::new(None, None),
                action.resource.clone(),
                action.arguments.clone(),
                request_context(correlation),
            ),
            risk,
            plan.blast_radius,
        );

        match policy.evaluate(&authz).outcome {
            PolicyOutcome::Deny => {
                outcome.denied.push(action.capability.clone());
                let _ = events
                    .publish(&event(
                        types::PLAN_DENIED,
                        json!({ "capability": action.capability.as_str() }),
                        correlation,
                        None,
                    ))
                    .await;
            }
            PolicyOutcome::RequireApproval => {
                outcome.requires_approval.push(action.capability.clone());
            }
            PolicyOutcome::Allow => {
                if !may_execute_without_approval(autonomy, risk) {
                    outcome.requires_approval.push(action.capability.clone());
                    continue;
                }
                let execution = match execute_action(action, service) {
                    Ok(evidence) => {
                        let _ = events
                            .publish(&event(
                                types::ACTION_EXECUTED,
                                evidence.clone(),
                                correlation,
                                None,
                            ))
                            .await;
                        Execution {
                            action: action.clone(),
                            status: ExecutionStatus::Completed,
                            evidence,
                        }
                    }
                    Err(err) => {
                        let _ = events
                            .publish(&event(
                                types::ACTION_FAILED,
                                json!({ "error": err }),
                                correlation,
                                None,
                            ))
                            .await;
                        Execution {
                            action: action.clone(),
                            status: ExecutionStatus::Failed,
                            evidence: json!({ "error": err }),
                        }
                    }
                };
                outcome.executions.push(execution);
            }
        }
    }

    outcome
}

fn request_context(correlation: Uuid) -> RequestContext {
    RequestContext::new(
        correlation,
        Version::new(0, 1, 0),
        Principal::new(None, None),
        Utc::now(),
    )
}

fn event(
    event_type: &str,
    payload: Value,
    correlation: Uuid,
    causation: Option<Uuid>,
) -> DomainEvent {
    DomainEvent::new(
        Uuid::new_v4(),
        EventType::new(event_type).expect("valid event type"),
        Utc::now(),
        "argusd",
        "argusd",
        Severity::Info,
        Some(correlation),
        causation,
        payload,
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use argus_domain::{BlastRadius, PlanStatus};
    use argus_events::LocalEventBus;
    use argus_executor::MockServiceController;
    use argus_policy::BootstrapPolicyEvaluator;

    fn plan(unit: &str) -> Plan {
        Plan {
            objective: "restore nginx".into(),
            actions: vec![Action {
                capability: CapabilityId::new(CapabilityId::HOST_SERVICE_RESTART).unwrap(),
                resource: None,
                arguments: json!({ "unit": unit }),
            }],
            preconditions: vec![],
            expected_outcomes: vec![],
            rollback: None,
            blast_radius: BlastRadius::Host,
            confidence: 0.9,
            status: PlanStatus::Proposed,
        }
    }

    #[tokio::test]
    async fn assisted_mode_executes_allowed_low_risk_action() {
        let service = MockServiceController::new();
        let events = Arc::new(LocalEventBus::new(16));
        let policy = BootstrapPolicyEvaluator::new();

        let outcome = authorize_and_run(
            &plan("nginx.service"),
            &policy,
            &service,
            events.as_ref(),
            AutonomyMode::Assisted,
        )
        .await;

        assert_eq!(outcome.executions.len(), 1);
        assert_eq!(outcome.executions[0].status, ExecutionStatus::Completed);
        assert!(outcome.denied.is_empty());
        assert!(outcome.requires_approval.is_empty());
        assert_eq!(
            service.recorded_calls(),
            vec![("restart".to_string(), "nginx.service".to_string())]
        );
    }

    #[tokio::test]
    async fn propose_mode_requires_approval_and_never_executes() {
        let service = MockServiceController::new();
        let events = Arc::new(LocalEventBus::new(16));
        let policy = BootstrapPolicyEvaluator::new();

        let outcome = authorize_and_run(
            &plan("nginx.service"),
            &policy,
            &service,
            events.as_ref(),
            AutonomyMode::Propose,
        )
        .await;

        assert!(outcome.executions.is_empty());
        assert_eq!(outcome.requires_approval.len(), 1);
        assert!(service.recorded_calls().is_empty());
    }

    #[tokio::test]
    async fn unknown_capability_is_denied_by_policy() {
        let service = MockServiceController::new();
        let events = Arc::new(LocalEventBus::new(16));
        let policy = BootstrapPolicyEvaluator::new();

        let mut p = plan("nginx.service");
        p.actions[0].capability = CapabilityId::new("host.process.signal").unwrap();

        let outcome = authorize_and_run(
            &p,
            &policy,
            &service,
            events.as_ref(),
            AutonomyMode::Assisted,
        )
        .await;

        assert!(outcome.executions.is_empty());
        assert_eq!(outcome.denied.len(), 1);
        assert!(service.recorded_calls().is_empty());
    }

    #[tokio::test]
    async fn missing_unit_argument_fails_execution() {
        let service = MockServiceController::new();
        let events = Arc::new(LocalEventBus::new(16));
        let policy = BootstrapPolicyEvaluator::new();

        let mut p = plan("nginx.service");
        p.actions[0].arguments = json!({});

        let outcome = authorize_and_run(
            &p,
            &policy,
            &service,
            events.as_ref(),
            AutonomyMode::Assisted,
        )
        .await;

        assert_eq!(outcome.executions.len(), 1);
        assert_eq!(outcome.executions[0].status, ExecutionStatus::Failed);
    }
}
