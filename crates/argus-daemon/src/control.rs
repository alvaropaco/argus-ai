//! Control loop: routes a proposed plan through policy, autonomy, and execution.

use argus_ai_core::decision::autonomy::may_execute_without_approval;
use argus_domain::{
    Action, AuthorizationRequest, AutonomyMode, CapabilityId, CapabilityRequest, DomainEvent,
    EventType, Execution, ExecutionStatus, Plan, PlanStatus, PolicyOutcome, Principal,
    RequestContext, RiskClass, Severity,
};
use argus_events::{EventBus, types};
use argus_executor::ServiceController;
use argus_policy::PolicyEvaluator;
use chrono::Utc;
use semver::Version;
use serde_json::{Value, json};
use uuid::Uuid;

/// The result of routing a plan through the safety boundary.
#[derive(Debug)]
pub struct ExecutionOutcome {
    pub executions: Vec<Execution>,
    pub denied: Vec<CapabilityId>,
    pub requires_approval: Vec<CapabilityId>,
    /// The plan's final status after execution (fail-stop, rollback, or success).
    pub status: PlanStatus,
}

impl Default for ExecutionOutcome {
    fn default() -> Self {
        Self {
            executions: Vec::new(),
            denied: Vec::new(),
            requires_approval: Vec::new(),
            status: PlanStatus::Proposed,
        }
    }
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

/// Routes every step in `plan` through policy, then autonomy, then execution.
///
/// This is the only path from a proposed plan to execution (FR-004, FR-005).
/// Execution is fail-stop: on a step failure the already-executed steps'
/// declarative rollbacks run in reverse order, and the plan ends `Failed`,
/// `RolledBack`, or `NeedsManual` — never substituting actions (ADR-0028 §4).
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
            json!({ "objective": plan.objective, "steps": plan.steps.len() }),
            correlation,
            None,
        ))
        .await;

    let mut outcome = ExecutionOutcome {
        status: PlanStatus::Executing,
        ..ExecutionOutcome::default()
    };
    // Indices of steps that took effect, for reverse-order rollback.
    let mut executed: Vec<usize> = Vec::new();

    for (index, step) in plan.steps.iter().enumerate() {
        let action = &step.action;
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

                // Idempotency: check the desired state before acting. "Already in
                // the desired state" is a recorded success no-op (ADR-0028 §5).
                match desired_state_holds(action, service) {
                    Ok(true) => {
                        let evidence = json!({ "already_desired": true });
                        let _ = events
                            .publish(&event(
                                types::ACTION_ALREADY_DESIRED,
                                evidence.clone(),
                                correlation,
                                None,
                            ))
                            .await;
                        outcome.executions.push(Execution {
                            action: action.clone(),
                            status: ExecutionStatus::Completed,
                            evidence,
                        });
                    }
                    Ok(false) => match execute_action(action, service) {
                        Ok(evidence) => {
                            let _ = events
                                .publish(&event(
                                    types::ACTION_EXECUTED,
                                    evidence.clone(),
                                    correlation,
                                    None,
                                ))
                                .await;
                            outcome.executions.push(Execution {
                                action: action.clone(),
                                status: ExecutionStatus::Completed,
                                evidence,
                            });
                            executed.push(index);
                        }
                        Err(err) => {
                            record_failure(&mut outcome, action, err, events, correlation).await;
                            rollback_executed(
                                &mut outcome,
                                plan,
                                &executed,
                                service,
                                events,
                                correlation,
                            )
                            .await;
                            return outcome;
                        }
                    },
                    Err(err) => {
                        // A live-state read failure is fail-closed: acting on an
                        // unknown desired state is refused.
                        record_failure(&mut outcome, action, err, events, correlation).await;
                        rollback_executed(
                            &mut outcome,
                            plan,
                            &executed,
                            service,
                            events,
                            correlation,
                        )
                        .await;
                        return outcome;
                    }
                }
            }
        }
    }

    outcome.status = if outcome.executions.is_empty() {
        PlanStatus::Denied
    } else {
        PlanStatus::Completed
    };
    outcome
}

/// Records a failed step and publishes the failure event.
async fn record_failure(
    outcome: &mut ExecutionOutcome,
    action: &Action,
    error: String,
    events: &dyn EventBus,
    correlation: Uuid,
) {
    let _ = events
        .publish(&event(
            types::ACTION_FAILED,
            json!({ "error": error }),
            correlation,
            None,
        ))
        .await;
    outcome.executions.push(Execution {
        action: action.clone(),
        status: ExecutionStatus::Failed,
        evidence: json!({ "error": error }),
    });
    outcome.status = PlanStatus::Failed;
    let _ = events
        .publish(&event(types::PLAN_FAILED, json!({}), correlation, None))
        .await;
}

/// Runs the executed steps' declarative rollbacks in reverse order.
///
/// The plan status moves `Failed → RollingBack → RolledBack`, or `NeedsManual`
/// when a rollback itself fails. The planner never substitutes actions
/// mid-failure (ADR-0028 §4).
async fn rollback_executed(
    outcome: &mut ExecutionOutcome,
    plan: &Plan,
    executed: &[usize],
    service: &dyn ServiceController,
    events: &dyn EventBus,
    correlation: Uuid,
) {
    // Nothing took effect, so there is nothing to undo: the plan stays `Failed`.
    if executed.is_empty() {
        return;
    }

    outcome.status = PlanStatus::RollingBack;
    let _ = events
        .publish(&event(
            types::PLAN_ROLLING_BACK,
            json!({}),
            correlation,
            None,
        ))
        .await;

    for &index in executed.iter().rev() {
        let Some(rollback) = &plan.steps[index].rollback else {
            // An executed step with no declarative rollback cannot be undone.
            outcome.status = PlanStatus::NeedsManual;
            let _ = events
                .publish(&event(
                    types::PLAN_NEEDS_MANUAL,
                    json!({}),
                    correlation,
                    None,
                ))
                .await;
            return;
        };
        match execute_action(rollback, service) {
            Ok(evidence) => {
                let _ = events
                    .publish(&event(
                        types::ACTION_ROLLED_BACK,
                        evidence,
                        correlation,
                        None,
                    ))
                    .await;
            }
            Err(err) => {
                let _ = events
                    .publish(&event(
                        types::ACTION_ROLLBACK_FAILED,
                        json!({ "error": err }),
                        correlation,
                        None,
                    ))
                    .await;
                outcome.status = PlanStatus::NeedsManual;
                let _ = events
                    .publish(&event(
                        types::PLAN_NEEDS_MANUAL,
                        json!({}),
                        correlation,
                        None,
                    ))
                    .await;
                return;
            }
        }
    }

    outcome.status = PlanStatus::RolledBack;
    let _ = events
        .publish(&event(
            types::PLAN_ROLLED_BACK,
            json!({}),
            correlation,
            None,
        ))
        .await;
}

/// The desired state a service capability moves its unit toward, when the action
/// is idempotent-checkable: start/restart move toward active, stop toward
/// inactive. Other capabilities have no desired-state check.
fn desired_state(capability: &CapabilityId) -> Option<bool> {
    match capability.as_str() {
        CapabilityId::HOST_SERVICE_START | CapabilityId::HOST_SERVICE_RESTART => Some(true),
        CapabilityId::HOST_SERVICE_STOP => Some(false),
        _ => None,
    }
}

/// Whether the target is already in the action's desired state, read from live
/// state at execution time. `Ok(false)` means the effect should run.
fn desired_state_holds(action: &Action, service: &dyn ServiceController) -> Result<bool, String> {
    let Some(desired) = desired_state(&action.capability) else {
        return Ok(false);
    };
    let unit = action
        .arguments
        .get("unit")
        .and_then(Value::as_str)
        .ok_or_else(|| "action missing 'unit' argument".to_string())?;
    let active = service.is_active(unit).map_err(|e| e.to_string())?;
    Ok(active == desired)
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
    use argus_domain::{BlastRadius, PlanStatus, PlanStep};
    use argus_events::LocalEventBus;
    use argus_executor::MockServiceController;
    use argus_policy::BootstrapPolicyEvaluator;

    fn restart_action(unit: &str) -> Action {
        Action {
            capability: CapabilityId::new(CapabilityId::HOST_SERVICE_RESTART).unwrap(),
            resource: None,
            arguments: json!({ "unit": unit }),
        }
    }

    fn plan(unit: &str) -> Plan {
        Plan {
            objective: "restore nginx".into(),
            steps: vec![PlanStep {
                action: restart_action(unit),
                rollback: None,
            }],
            preconditions: vec![],
            expected_outcomes: vec![],
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
        assert_eq!(outcome.status, PlanStatus::Denied);
        assert!(service.recorded_calls().is_empty());
    }

    #[tokio::test]
    async fn unknown_capability_is_denied_by_policy() {
        let service = MockServiceController::new();
        let events = Arc::new(LocalEventBus::new(16));
        let policy = BootstrapPolicyEvaluator::new();

        let mut p = plan("nginx.service");
        p.steps[0].action.capability = CapabilityId::new("host.process.signal").unwrap();

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
        assert_eq!(outcome.status, PlanStatus::Denied);
        assert!(service.recorded_calls().is_empty());
    }

    #[tokio::test]
    async fn missing_unit_argument_fails_execution() {
        let service = MockServiceController::new();
        let events = Arc::new(LocalEventBus::new(16));
        let policy = BootstrapPolicyEvaluator::new();

        let mut p = plan("nginx.service");
        p.steps[0].action.arguments = json!({});

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
        assert_eq!(outcome.status, PlanStatus::Failed);
    }

    /// A two-step "bounce" plan: stop then start, each carrying its declarative
    /// inverse (start undoes stop, stop undoes start).
    fn bounce_plan() -> Plan {
        let stop = Action {
            capability: CapabilityId::new(CapabilityId::HOST_SERVICE_STOP).unwrap(),
            resource: None,
            arguments: json!({ "unit": "nginx.service" }),
        };
        let start = Action {
            capability: CapabilityId::new(CapabilityId::HOST_SERVICE_START).unwrap(),
            resource: None,
            arguments: json!({ "unit": "nginx.service" }),
        };
        Plan {
            objective: "bounce nginx".into(),
            steps: vec![
                PlanStep {
                    action: stop.clone(),
                    rollback: Some(start.clone()),
                },
                PlanStep {
                    action: start,
                    rollback: Some(stop),
                },
            ],
            preconditions: vec![],
            expected_outcomes: vec![],
            blast_radius: BlastRadius::Host,
            confidence: 0.9,
            status: PlanStatus::Proposed,
        }
    }

    /// A plan whose second step is a restart (no inverse) that fails, so the
    /// first step's rollback (a start) succeeds and the plan ends rolled-back.
    fn restart_after_stop_plan() -> Plan {
        let stop = Action {
            capability: CapabilityId::new(CapabilityId::HOST_SERVICE_STOP).unwrap(),
            resource: None,
            arguments: json!({ "unit": "nginx.service" }),
        };
        let start = Action {
            capability: CapabilityId::new(CapabilityId::HOST_SERVICE_START).unwrap(),
            resource: None,
            arguments: json!({ "unit": "nginx.service" }),
        };
        let restart = Action {
            capability: CapabilityId::new(CapabilityId::HOST_SERVICE_RESTART).unwrap(),
            resource: None,
            arguments: json!({ "unit": "nginx.service" }),
        };
        Plan {
            objective: "bounce nginx".into(),
            steps: vec![
                PlanStep {
                    action: stop,
                    rollback: Some(start),
                },
                PlanStep {
                    action: restart,
                    rollback: None,
                },
            ],
            preconditions: vec![],
            expected_outcomes: vec![],
            blast_radius: BlastRadius::Host,
            confidence: 0.9,
            status: PlanStatus::Proposed,
        }
    }

    #[tokio::test]
    async fn fail_stop_runs_executed_steps_rollbacks_in_reverse() {
        // Step 1 (stop) succeeds, step 2 (restart) fails; step 1's rollback
        // (start) runs and the plan ends rolled-back.
        let service = FailingMockServiceController::failing(&["restart"]);
        service.set_active("nginx.service");
        let events = Arc::new(LocalEventBus::new(16));
        let policy = BootstrapPolicyEvaluator::new();

        let outcome = authorize_and_run(
            &restart_after_stop_plan(),
            &policy,
            &service,
            events.as_ref(),
            AutonomyMode::Assisted,
        )
        .await;

        assert_eq!(outcome.status, PlanStatus::RolledBack);
        assert_eq!(
            service.recorded_calls(),
            vec![
                ("stop".to_string(), "nginx.service".to_string()),
                ("restart".to_string(), "nginx.service".to_string()),
                ("start".to_string(), "nginx.service".to_string()),
            ],
            "the executed stop is undone by a start after the failure"
        );
    }

    #[tokio::test]
    async fn rollback_failure_marks_the_plan_needs_manual() {
        // Step 1 (stop) succeeds, step 2 (start) fails, and step 1's rollback
        // (also a start) fails too — the plan needs manual intervention.
        let service = FailingMockServiceController::failing(&["start"]);
        service.set_active("nginx.service");
        let events = Arc::new(LocalEventBus::new(16));
        let policy = BootstrapPolicyEvaluator::new();

        let outcome = authorize_and_run(
            &bounce_plan(),
            &policy,
            &service,
            events.as_ref(),
            AutonomyMode::Assisted,
        )
        .await;

        assert_eq!(outcome.status, PlanStatus::NeedsManual);
        assert_eq!(
            service.recorded_calls(),
            vec![
                ("stop".to_string(), "nginx.service".to_string()),
                ("start".to_string(), "nginx.service".to_string()),
                ("start".to_string(), "nginx.service".to_string()),
            ]
        );
    }

    #[tokio::test]
    async fn already_desired_state_is_a_success_noop() {
        let service = MockServiceController::new();
        service.set_active("nginx.service", true);
        let events = Arc::new(LocalEventBus::new(16));
        let policy = BootstrapPolicyEvaluator::new();

        // A restart toward "active" on an already-active unit is a no-op.
        let outcome = authorize_and_run(
            &plan("nginx.service"),
            &policy,
            &service,
            events.as_ref(),
            AutonomyMode::Assisted,
        )
        .await;

        assert_eq!(outcome.status, PlanStatus::Completed);
        assert_eq!(outcome.executions.len(), 1);
        assert_eq!(outcome.executions[0].status, ExecutionStatus::Completed);
        assert_eq!(outcome.executions[0].evidence["already_desired"], true);
        assert!(
            service.recorded_calls().is_empty(),
            "already-desired must not touch the host"
        );
    }

    /// A controller whose configured failing verbs drive the rollback path, and
    /// whose live state reflects the effects of successful start/stop calls.
    struct FailingMockServiceController {
        fail: std::collections::BTreeSet<String>,
        calls: std::sync::Mutex<Vec<(String, String)>>,
        active: std::sync::Mutex<std::collections::BTreeSet<String>>,
    }

    impl FailingMockServiceController {
        fn failing(verbs: &[&str]) -> Self {
            Self {
                fail: verbs.iter().map(|v| v.to_string()).collect(),
                calls: std::sync::Mutex::new(Vec::new()),
                active: std::sync::Mutex::new(std::collections::BTreeSet::new()),
            }
        }

        fn set_active(&self, unit: &str) {
            self.active.lock().unwrap().insert(unit.to_string());
        }

        fn recorded_calls(&self) -> Vec<(String, String)> {
            self.calls.lock().unwrap().clone()
        }

        fn record(&self, verb: &str, unit: &str) -> Result<(), argus_executor::ServiceError> {
            self.calls
                .lock()
                .unwrap()
                .push((verb.to_string(), unit.to_string()));
            if self.fail.contains(verb) {
                return Err(argus_executor::ServiceError::Failed(format!(
                    "{verb} refused"
                )));
            }
            let mut active = self.active.lock().unwrap();
            match verb {
                "stop" => {
                    active.remove(unit);
                }
                "start" | "restart" => {
                    active.insert(unit.to_string());
                }
                _ => {}
            }
            Ok(())
        }
    }

    impl ServiceController for FailingMockServiceController {
        fn restart(&self, unit: &str) -> Result<(), argus_executor::ServiceError> {
            self.record("restart", unit)
        }
        fn stop(&self, unit: &str) -> Result<(), argus_executor::ServiceError> {
            self.record("stop", unit)
        }
        fn start(&self, unit: &str) -> Result<(), argus_executor::ServiceError> {
            self.record("start", unit)
        }
        fn is_active(&self, unit: &str) -> Result<bool, argus_executor::ServiceError> {
            Ok(self.active.lock().unwrap().contains(unit))
        }
    }

    /// A controller whose live-state read always fails, to drive the fail-closed
    /// desired-state path.
    struct UnavailableStateController;

    impl ServiceController for UnavailableStateController {
        fn restart(&self, _unit: &str) -> Result<(), argus_executor::ServiceError> {
            unreachable!("the desired-state read fails before any effect")
        }
        fn stop(&self, _unit: &str) -> Result<(), argus_executor::ServiceError> {
            unreachable!("the desired-state read fails before any effect")
        }
        fn start(&self, _unit: &str) -> Result<(), argus_executor::ServiceError> {
            unreachable!("the desired-state read fails before any effect")
        }
        fn is_active(&self, _unit: &str) -> Result<bool, argus_executor::ServiceError> {
            Err(argus_executor::ServiceError::Failed(
                "systemd unavailable".into(),
            ))
        }
    }

    /// A plan whose first step (a restart) has no inverse and whose second step
    /// (a stop) fails — the executed restart cannot be undone.
    fn restart_then_stop_plan() -> Plan {
        let restart = Action {
            capability: CapabilityId::new(CapabilityId::HOST_SERVICE_RESTART).unwrap(),
            resource: None,
            arguments: json!({ "unit": "nginx.service" }),
        };
        let stop = Action {
            capability: CapabilityId::new(CapabilityId::HOST_SERVICE_STOP).unwrap(),
            resource: None,
            arguments: json!({ "unit": "nginx.service" }),
        };
        let start = Action {
            capability: CapabilityId::new(CapabilityId::HOST_SERVICE_START).unwrap(),
            resource: None,
            arguments: json!({ "unit": "nginx.service" }),
        };
        Plan {
            objective: "restart then stop".into(),
            steps: vec![
                PlanStep {
                    action: restart,
                    rollback: None,
                },
                PlanStep {
                    action: stop,
                    rollback: Some(start),
                },
            ],
            preconditions: vec![],
            expected_outcomes: vec![],
            blast_radius: BlastRadius::Host,
            confidence: 0.9,
            status: PlanStatus::Proposed,
        }
    }

    #[tokio::test]
    async fn an_executed_step_without_a_rollback_needs_manual() {
        // Step 1 (restart, no inverse) succeeds, step 2 (stop) fails; the
        // executed restart cannot be undone, so the plan needs manual help.
        let service = FailingMockServiceController::failing(&["stop"]);
        let events = Arc::new(LocalEventBus::new(16));
        let policy = BootstrapPolicyEvaluator::new();

        let outcome = authorize_and_run(
            &restart_then_stop_plan(),
            &policy,
            &service,
            events.as_ref(),
            AutonomyMode::Assisted,
        )
        .await;

        assert_eq!(outcome.status, PlanStatus::NeedsManual);
    }

    #[tokio::test]
    async fn a_failed_desired_state_read_fails_closed() {
        let service = UnavailableStateController;
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

        assert_eq!(outcome.status, PlanStatus::Failed);
        assert_eq!(outcome.executions.len(), 1);
        assert_eq!(outcome.executions[0].status, ExecutionStatus::Failed);
    }

    #[tokio::test]
    async fn a_mid_plan_denial_is_recorded_and_does_not_fail_stop() {
        let restart_nginx = restart_action("nginx.service");
        let signal = Action {
            capability: CapabilityId::new("host.process.signal").unwrap(),
            resource: None,
            arguments: json!({ "pid": 123 }),
        };
        let restart_postgres = restart_action("postgres.service");
        let plan = Plan {
            objective: "restart two services".into(),
            steps: vec![
                PlanStep {
                    action: restart_nginx,
                    rollback: None,
                },
                PlanStep {
                    action: signal,
                    rollback: None,
                },
                PlanStep {
                    action: restart_postgres,
                    rollback: None,
                },
            ],
            preconditions: vec![],
            expected_outcomes: vec![],
            blast_radius: BlastRadius::Host,
            confidence: 0.9,
            status: PlanStatus::Proposed,
        };

        let service = MockServiceController::new();
        let events = Arc::new(LocalEventBus::new(16));
        let policy = BootstrapPolicyEvaluator::new();

        let outcome = authorize_and_run(
            &plan,
            &policy,
            &service,
            events.as_ref(),
            AutonomyMode::Assisted,
        )
        .await;

        assert_eq!(outcome.denied.len(), 1);
        assert_eq!(outcome.executions.len(), 2, "the two allowed steps run");
        assert_eq!(outcome.status, PlanStatus::Completed);
        assert_eq!(
            service.recorded_calls(),
            vec![
                ("restart".to_string(), "nginx.service".to_string()),
                ("restart".to_string(), "postgres.service".to_string()),
            ]
        );
    }
}
