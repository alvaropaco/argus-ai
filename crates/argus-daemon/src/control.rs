//! Control loop: routes a proposed plan through governance, policy, autonomy,
//! and execution.

use argus_ai_core::decision::autonomy::may_execute_without_approval;
use argus_domain::{
    Action, AuthorizationRequest, AutonomyMode, CapabilityId, CapabilityRegistry,
    CapabilityRequest, DomainEvent, EventType, Execution, ExecutionStatus, Plan, PlanStatus,
    PolicyDecision, PolicyOutcome, Principal, RequestContext, RiskClass, Severity,
    plan_context_hash,
};
use argus_events::{EventBus, types};
use argus_executor::{
    AuthorizedAction, CgroupController, ContainerController, Executor, RemediationExecutor,
    ServiceController,
};
use argus_policy::{
    ApprovalStore, GovernanceDecision, PolicyEvaluator, ResourceAdjustment, ResourceGovernor,
};
use argus_validate::{desired_state, read_failure_event, validation_event};
use chrono::Utc;
use semver::Version;
use serde_json::{Value, json};
use uuid::Uuid;

/// The remediation ports the control loop executes through (spec 003 M3):
/// the autopilot governor, the container/cgroup live-state readers, and the
/// executor boundary for the remediation capabilities. Services keep their
/// dedicated controller path; everything else routes through `remediation`.
pub struct LoopPorts<'a> {
    pub governor: &'a dyn ResourceGovernor,
    pub containers: &'a dyn ContainerController,
    pub cgroups: &'a dyn CgroupController,
    pub remediation: &'a dyn Executor,
}

impl LoopPorts<'_> {
    /// Ports that govern and execute nothing: the fail-closed stand-in for
    /// callers (and the existing tests) that carry no remediation surface.
    pub fn noop() -> Self {
        static NO_GOVERNOR: argus_policy::NoGovernor = argus_policy::NoGovernor;
        struct NoController;
        impl ContainerController for NoController {
            fn restart(&self, _id: &str) -> Result<(), argus_executor::RemediationError> {
                Err(argus_executor::RemediationError::Unavailable(
                    "no container controller is wired".to_string(),
                ))
            }
            fn is_running(&self, _id: &str) -> Result<bool, argus_executor::RemediationError> {
                Err(argus_executor::RemediationError::Unavailable(
                    "no container controller is wired".to_string(),
                ))
            }
        }
        impl CgroupController for NoController {
            fn set_freeze(
                &self,
                _path: &str,
                _frozen: bool,
            ) -> Result<(), argus_executor::RemediationError> {
                Err(argus_executor::RemediationError::Unavailable(
                    "no cgroup controller is wired".to_string(),
                ))
            }
            fn is_frozen(&self, _path: &str) -> Result<bool, argus_executor::RemediationError> {
                Err(argus_executor::RemediationError::Unavailable(
                    "no cgroup controller is wired".to_string(),
                ))
            }
        }
        struct NoExecutor;
        impl Executor for NoExecutor {
            fn execute(
                &self,
                action: &AuthorizedAction,
            ) -> Result<argus_executor::ExecutionResult, argus_executor::ExecutionError>
            {
                Err(argus_executor::ExecutionError::Unsupported(
                    action.capability().clone(),
                ))
            }
        }

        static NO_CONTAINERS: NoController = NoController;
        static NO_CGROUPS: NoController = NoController;
        static NO_EXECUTOR: NoExecutor = NoExecutor;
        Self {
            governor: &NO_GOVERNOR,
            containers: &NO_CONTAINERS,
            cgroups: &NO_CGROUPS,
            remediation: &NO_EXECUTOR,
        }
    }
}

/// A plan paused at an approval-requiring step, bound to a single-use token and
/// its context hash (ADR-0030 §2, §3).
#[derive(Debug, Clone)]
pub struct PendingPlan {
    pub plan: Plan,
    pub token: Uuid,
    pub context_hash: String,
    /// Indices of steps that already executed before the pause, so a post-resume
    /// failure still rolls back pre-pause effects (ADR-0030 §5).
    pub executed: Vec<usize>,
}

/// The result of routing a plan through the safety boundary.
#[derive(Debug)]
pub enum RunOutcome {
    /// The plan reached a terminal status.
    Finished(ExecutionOutcome),
    /// A step requires approval; the plan paused, executing nothing further.
    Pending(PendingPlan),
}

/// Why a resume was refused (nothing executed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeRefusal {
    /// The grant's context hash does not match the stored plan's.
    Stale,
    /// No valid grant exists (absent, denied, expired, or already consumed).
    NoGrant,
}

/// The result of resuming a paused plan.
#[derive(Debug)]
pub enum ResumeOutcome {
    /// The plan ran to a terminal status.
    Finished(ExecutionOutcome),
    /// The resume was refused; nothing executed.
    Refused(ResumeRefusal),
}

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
        | CapabilityId::HOST_SERVICE_START
        | CapabilityId::HOST_CGROUP_FREEZE
        | CapabilityId::HOST_CGROUP_THAW => RiskClass::LowRisk,
        CapabilityId::HOST_PROCESS_SIGNAL => RiskClass::HighRisk,
        CapabilityId::CONTAINER_RESTART => RiskClass::Controlled,
        _ => RiskClass::Controlled,
    }
}

/// Whether the action names one of the service capabilities the loop executes
/// through its dedicated service controller.
fn is_service_action(capability: &CapabilityId) -> bool {
    matches!(
        capability.as_str(),
        CapabilityId::HOST_SERVICE_RESTART
            | CapabilityId::HOST_SERVICE_STOP
            | CapabilityId::HOST_SERVICE_START
    )
}

/// The subject a governance decision and a live-state read name, per family:
/// the cgroup path for resource-control, the container id, or the unit.
fn action_target(action: &Action) -> Option<&str> {
    ["path", "container", "unit"]
        .iter()
        .find_map(|key| action.arguments.get(*key).and_then(Value::as_str))
}

/// Executes one action through its family's boundary.
///
/// Service actions keep their dedicated controller path. Remediation actions
/// (`host.process.signal`, `container.restart`, `host.cgroup.*`) cross the
/// [`Executor`] boundary as [`AuthorizedAction`]s — the guardrails wired into
/// the remediation executor run there, at execution time (ADR-0028 §1).
/// `decision` is the policy's verdict for this exact step; a rollback carries
/// its own recorded allow (see `rollback_executed`).
fn execute_action(
    action: &Action,
    service: &dyn ServiceController,
    ports: &LoopPorts<'_>,
    decision: &PolicyDecision,
    correlation: Uuid,
) -> Result<Value, String> {
    let capability = &action.capability;

    if is_service_action(capability) {
        let unit = action
            .arguments
            .get("unit")
            .and_then(Value::as_str)
            .ok_or_else(|| "action missing 'unit' argument".to_string())?;

        let result = match capability.as_str() {
            CapabilityId::HOST_SERVICE_RESTART => service.restart(unit),
            CapabilityId::HOST_SERVICE_STOP => service.stop(unit),
            CapabilityId::HOST_SERVICE_START => service.start(unit),
            other => return Err(format!("unsupported executable capability '{other}'")),
        };

        return result
            .map(|_| json!({ "unit": unit, "capability": capability.as_str() }))
            .map_err(|e| e.to_string());
    }

    if RemediationExecutor::handles(capability) {
        let request = CapabilityRequest::new(
            capability.clone(),
            Principal::new(None, None),
            action.resource.clone(),
            action.arguments.clone(),
            RequestContext::new(
                correlation,
                Version::new(0, 1, 0),
                Principal::new(None, None),
                Utc::now(),
            ),
        );
        let authorized = AuthorizedAction::new(request, decision.clone())
            .map_err(|e| format!("action could not be authorized for execution: {e}"))?;
        return ports
            .remediation
            .execute(&authorized)
            .map(|result| result.evidence)
            .map_err(|e| e.to_string());
    }

    Err(format!(
        "unsupported executable capability '{}'",
        capability.as_str()
    ))
}

/// Routes every step in `plan` through policy, then autonomy, then execution.
///
/// This is the only path from a proposed plan to execution (FR-004, FR-005).
/// When a step's policy returns `RequireApproval`, the plan pauses
/// [`RunOutcome::Pending`] and nothing further executes (ADR-0030 §1); resume
/// via [`resume_and_run`]. Execution is fail-stop: on a step failure the
/// already-executed steps' declarative rollbacks run in reverse order, and the
/// plan ends `Failed`, `RolledBack`, or `NeedsManual` — never substituting
/// actions (ADR-0028 §4).
///
/// Runs with no-op remediation ports: governed capabilities are refused and
/// remediation effects unsupported. The remediation loop uses
/// [`authorize_and_run_with_ports`].
pub async fn authorize_and_run(
    plan: &Plan,
    registry: &CapabilityRegistry,
    policy: &dyn PolicyEvaluator,
    service: &dyn ServiceController,
    events: &dyn EventBus,
    autonomy: AutonomyMode,
) -> RunOutcome {
    authorize_and_run_with_ports(
        plan,
        registry,
        policy,
        service,
        events,
        autonomy,
        &LoopPorts::noop(),
    )
    .await
}

/// [`authorize_and_run`] with the remediation ports wired (spec 003 M3):
/// governed capabilities pass the autopilot governor before policy, and
/// remediation effects cross the executor boundary carried by `ports`.
pub async fn authorize_and_run_with_ports(
    plan: &Plan,
    registry: &CapabilityRegistry,
    policy: &dyn PolicyEvaluator,
    service: &dyn ServiceController,
    events: &dyn EventBus,
    autonomy: AutonomyMode,
    ports: &LoopPorts<'_>,
) -> RunOutcome {
    match run_plan(
        plan, registry, policy, service, events, autonomy, None, ports,
    )
    .await
    {
        PlanRun::Finished(outcome) => RunOutcome::Finished(outcome),
        PlanRun::Paused(pending) => RunOutcome::Pending(pending),
    }
}

/// Resumes a paused plan with a previously granted approval.
///
/// The grant is consumed exactly once before the stored plan is re-entered as-is
/// (no re-observe); a missing, denied, expired, already-consumed, or
/// hash-mismatched grant refuses the resume and executes nothing (ADR-0030 §4).
pub async fn resume_and_run(
    pending: &PendingPlan,
    approvals: &ApprovalStore,
    registry: &CapabilityRegistry,
    policy: &dyn PolicyEvaluator,
    service: &dyn ServiceController,
    events: &dyn EventBus,
    autonomy: AutonomyMode,
) -> ResumeOutcome {
    resume_and_run_with_ports(
        pending,
        approvals,
        registry,
        policy,
        service,
        events,
        autonomy,
        &LoopPorts::noop(),
    )
    .await
}

/// [`resume_and_run`] with the remediation ports wired (spec 003 M3).
///
/// The arity is the loop's injected surface: plan, grant store, and the
/// safety boundary's dependencies — none of which collapse without hiding an
/// injection the tests vary individually.
#[allow(clippy::too_many_arguments)]
pub async fn resume_and_run_with_ports(
    pending: &PendingPlan,
    approvals: &ApprovalStore,
    registry: &CapabilityRegistry,
    policy: &dyn PolicyEvaluator,
    service: &dyn ServiceController,
    events: &dyn EventBus,
    autonomy: AutonomyMode,
    ports: &LoopPorts<'_>,
) -> ResumeOutcome {
    // Consume the grant exactly once: it is the operator's single-use
    // authorization for this plan, so a second resume finds nothing to consume.
    if approvals
        .consume(pending.token, &pending.context_hash, Utc::now())
        .is_none()
    {
        let reason = if approvals
            .get(pending.token)
            .is_some_and(|approval| approval.context_hash != pending.context_hash)
        {
            ResumeRefusal::Stale
        } else {
            ResumeRefusal::NoGrant
        };
        return ResumeOutcome::Refused(reason);
    }

    match run_plan(
        &pending.plan,
        registry,
        policy,
        service,
        events,
        autonomy,
        Some(pending.executed.clone()),
        ports,
    )
    .await
    {
        PlanRun::Finished(outcome) => ResumeOutcome::Finished(outcome),
        // A resumed plan is authorized, so it can never pause again.
        PlanRun::Paused(_) => unreachable!("a resumed plan is authorized and cannot pause"),
    }
}

/// The internal outcome of the shared step loop.
enum PlanRun {
    Finished(ExecutionOutcome),
    Paused(PendingPlan),
}

/// The shared step loop behind [`authorize_and_run`] and [`resume_and_run`].
///
/// `resume` is `None` for a first run (pause on `RequireApproval`) and
/// `Some(executed)` for a resume, whose grant was already consumed by
/// [`resume_and_run`]; an approval-requiring step then proceeds as if allowed
/// (ADR-0030 §5). The carried `executed` indices are the steps already run
/// before the pause, so a post-resume failure still rolls back pre-pause
/// effects.
///
/// Governed capabilities (the autopilot families, e.g. `host.cgroup.*`) are
/// consulted with the governor *before* policy: a refusal is recorded exactly
/// like a policy denial and nothing executes (FR-017). A policy denial or an
/// approval requirement always wins over a governor allowance — the governor
/// can only refuse, never authorize (CAP-22).
///
/// The arity is the loop's injected surface; each parameter is a dependency
/// the tests vary individually, so it is not collapsed into a bundle.
#[allow(clippy::too_many_arguments)]
async fn run_plan(
    plan: &Plan,
    registry: &CapabilityRegistry,
    policy: &dyn PolicyEvaluator,
    service: &dyn ServiceController,
    events: &dyn EventBus,
    autonomy: AutonomyMode,
    resume: Option<Vec<usize>>,
    ports: &LoopPorts<'_>,
) -> PlanRun {
    let correlation = Uuid::new_v4();
    // AC-006: every reasoning, decision, execution, and validation step carries
    // the correlation id, so a plan run reads back as one trace.
    tracing::info!(
        correlation_id = %correlation,
        objective = %plan.objective,
        steps = plan.steps.len(),
        resumed = resume.is_some(),
        autonomy = ?autonomy,
        "plan run started"
    );
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
    let resumed = resume.is_some();
    let mut executed = resume.unwrap_or_default();
    let env = RunEnv {
        service,
        events,
        correlation,
        context_hash: plan_context_hash(plan),
        ports,
    };

    for (index, step) in plan.steps.iter().enumerate() {
        // A step already executed before the pause is not re-run on resume; its
        // index stays in the rollback set so a later failure still undoes it.
        if resumed && executed.contains(&index) {
            continue;
        }
        let action = &step.action;
        let risk = risk_for(&action.capability);

        // The autopilot gate: governed families pass the governor first, and a
        // refusal is final for the step regardless of what policy would say.
        if governed(&action.capability, ports.governor) {
            let adjustment = ResourceAdjustment {
                subject: action_target(action).unwrap_or_default().to_string(),
                capability: action.capability.as_str().to_string(),
                reason: format!("plan step {} of '{}'", index, plan.objective),
            };
            match ports.governor.evaluate(&adjustment) {
                GovernanceDecision::Allowed { reason } => {
                    tracing::info!(
                        correlation_id = %correlation,
                        capability = action.capability.as_str(),
                        subject = %adjustment.subject,
                        reason = %reason,
                        "autopilot governor allowed the adjustment"
                    );
                }
                GovernanceDecision::Refused { why } => {
                    tracing::info!(
                        correlation_id = %correlation,
                        capability = action.capability.as_str(),
                        subject = %adjustment.subject,
                        refusal = ?why,
                        "autopilot governor refused the adjustment"
                    );
                    outcome.denied.push(action.capability.clone());
                    let _ = events
                        .publish(&event(
                            types::PLAN_DENIED,
                            json!({
                                "capability": action.capability.as_str(),
                                "governance": why,
                            }),
                            correlation,
                            None,
                        ))
                        .await;
                    continue;
                }
            }
        }

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
        )
        // The approval requirement is sourced from the capability's own
        // descriptor, never a hardcoded list, so the local gate cannot drift
        // from the registry (ADR-0030 §1). A capability absent from the
        // registry defaults to requiring approval — it must not silently lose
        // the gate when its descriptor is missing (fail closed).
        .requiring_approval(
            registry
                .get(&action.capability)
                .map(|descriptor| descriptor.requires_approval())
                .unwrap_or(true),
        );

        let decision = policy.evaluate(&authz);
        match decision.outcome {
            PolicyOutcome::Deny => {
                tracing::info!(
                    correlation_id = %correlation,
                    capability = action.capability.as_str(),
                    step = index,
                    "policy denied the step"
                );
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
                tracing::info!(
                    correlation_id = %correlation,
                    capability = action.capability.as_str(),
                    step = index,
                    "policy requires approval for the step"
                );
                if !resumed {
                    // First run: pause, executing nothing further.
                    tracing::info!(
                        correlation_id = %correlation,
                        "plan paused awaiting operator approval"
                    );
                    let token = Uuid::new_v4();
                    let context_hash = plan_context_hash(plan);
                    let mut paused = plan.clone();
                    paused.status = PlanStatus::AwaitingApproval;
                    return PlanRun::Paused(PendingPlan {
                        plan: paused,
                        token,
                        context_hash,
                        executed: executed.clone(),
                    });
                }
                // Resumed: the approval re-entered policy as input, never as
                // authority; the consumed grant is what lets the step proceed.
                // The execution boundary needs an Allow verdict, and the
                // consumed grant is exactly that — recorded as such, so the
                // audit shows why the step was allowed to run (ADR-0030 §5).
                let granted = PolicyDecision::allow(
                    "plan.approval",
                    "resumed under a consumed single-use operator grant",
                );
                if execute_allowed_step(
                    &mut outcome,
                    plan,
                    index,
                    action,
                    &env,
                    &mut executed,
                    &granted,
                )
                .await
                {
                    return PlanRun::Finished(outcome);
                }
            }
            PolicyOutcome::Allow => {
                if !may_execute_without_approval(autonomy, risk) {
                    outcome.requires_approval.push(action.capability.clone());
                    continue;
                }

                if execute_allowed_step(
                    &mut outcome,
                    plan,
                    index,
                    action,
                    &env,
                    &mut executed,
                    &decision,
                )
                .await
                {
                    return PlanRun::Finished(outcome);
                }
            }
        }
    }

    outcome.status = if outcome.executions.is_empty() {
        PlanStatus::Denied
    } else {
        PlanStatus::Completed
    };
    tracing::info!(
        correlation_id = %correlation,
        status = ?outcome.status,
        executed = outcome.executions.len(),
        denied = outcome.denied.len(),
        "plan run finished"
    );
    PlanRun::Finished(outcome)
}

/// The injected execution environment shared across one plan run.
struct RunEnv<'a> {
    service: &'a dyn ServiceController,
    events: &'a dyn EventBus,
    correlation: Uuid,
    context_hash: String,
    ports: &'a LoopPorts<'a>,
}

/// Whether the capability belongs to a family the governor gates.
fn governed(capability: &CapabilityId, governor: &dyn ResourceGovernor) -> bool {
    governor
        .governed_prefixes()
        .iter()
        .any(|prefix| capability.as_str().starts_with(prefix))
}

/// Executes one policy-approved step: the desired-state idempotency check, then
/// the effect, with fail-stop rollback on failure (ADR-0028 §4, §5).
///
/// Returns `true` when the plan reached a terminal status via rollback, in which
/// case the caller must stop and return the finished outcome.
async fn execute_allowed_step(
    outcome: &mut ExecutionOutcome,
    plan: &Plan,
    index: usize,
    action: &Action,
    env: &RunEnv<'_>,
    executed: &mut Vec<usize>,
    decision: &PolicyDecision,
) -> bool {
    // Idempotency: check the desired state before acting. "Already in the
    // desired state" is a recorded success no-op (ADR-0028 §5).
    match desired_state_holds(action, env) {
        Ok(true) => {
            let evidence = json!({ "already_desired": true });
            let _ = env
                .events
                .publish(&event(
                    types::ACTION_ALREADY_DESIRED,
                    evidence.clone(),
                    env.correlation,
                    None,
                ))
                .await;
            outcome.executions.push(Execution {
                action: action.clone(),
                status: ExecutionStatus::Completed,
                evidence,
            });
            false
        }
        Ok(false) => {
            match execute_action(action, env.service, env.ports, decision, env.correlation) {
                Ok(evidence) => {
                    tracing::info!(
                        correlation_id = %env.correlation,
                        capability = action.capability.as_str(),
                        step = index,
                        "step executed"
                    );
                    let _ = env
                        .events
                        .publish(&event(
                            types::ACTION_EXECUTED,
                            evidence.clone(),
                            env.correlation,
                            None,
                        ))
                        .await;
                    outcome.executions.push(Execution {
                        action: action.clone(),
                        status: ExecutionStatus::Completed,
                        evidence,
                    });
                    executed.push(index);
                    // Re-observe after the step executed and publish the validation
                    // outcome (ADR-0031 §6).
                    validate_step(action, env).await;
                    false
                }
                Err(err) => {
                    tracing::warn!(
                        correlation_id = %env.correlation,
                        capability = action.capability.as_str(),
                        step = index,
                        error = %err,
                        "step failed; rolling back executed steps"
                    );
                    record_failure(outcome, action, err, env.events, env.correlation).await;
                    rollback_executed(outcome, plan, executed, env).await;
                    true
                }
            }
        }
        Err(err) => {
            // A live-state read failure is fail-closed: acting on an unknown
            // desired state is refused.
            tracing::warn!(
                correlation_id = %env.correlation,
                capability = action.capability.as_str(),
                step = index,
                error = %err,
                "desired-state read failed; fail-closed"
            );
            record_failure(outcome, action, err, env.events, env.correlation).await;
            rollback_executed(outcome, plan, executed, env).await;
            true
        }
    }
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
/// mid-failure (ADR-0028 §4). Rollbacks are part of the original plan's
/// authorization — they are never re-governed (recovery must not be blocked by
/// the autopilot budget) and cross the same executor boundary as the steps.
async fn rollback_executed(
    outcome: &mut ExecutionOutcome,
    plan: &Plan,
    executed: &[usize],
    env: &RunEnv<'_>,
) {
    // Nothing took effect, so there is nothing to undo: the plan stays `Failed`.
    if executed.is_empty() {
        return;
    }

    outcome.status = PlanStatus::RollingBack;
    tracing::warn!(
        correlation_id = %env.correlation,
        steps = executed.len(),
        "rolling back executed steps"
    );
    let _ = env
        .events
        .publish(&event(
            types::PLAN_ROLLING_BACK,
            json!({}),
            env.correlation,
            None,
        ))
        .await;

    for &index in executed.iter().rev() {
        let Some(rollback) = &plan.steps[index].rollback else {
            // An executed step with no declarative rollback cannot be undone.
            outcome.status = PlanStatus::NeedsManual;
            let _ = env
                .events
                .publish(&event(
                    types::PLAN_NEEDS_MANUAL,
                    json!({}),
                    env.correlation,
                    None,
                ))
                .await;
            return;
        };
        let decision = PolicyDecision::allow("plan.rollback", "declarative plan rollback");
        match execute_action(rollback, env.service, env.ports, &decision, env.correlation) {
            Ok(evidence) => {
                let _ = env
                    .events
                    .publish(&event(
                        types::ACTION_ROLLED_BACK,
                        evidence,
                        env.correlation,
                        None,
                    ))
                    .await;
            }
            Err(err) => {
                let _ = env
                    .events
                    .publish(&event(
                        types::ACTION_ROLLBACK_FAILED,
                        json!({ "error": err }),
                        env.correlation,
                        None,
                    ))
                    .await;
                outcome.status = PlanStatus::NeedsManual;
                let _ = env
                    .events
                    .publish(&event(
                        types::PLAN_NEEDS_MANUAL,
                        json!({}),
                        env.correlation,
                        None,
                    ))
                    .await;
                return;
            }
        }
    }

    outcome.status = PlanStatus::RolledBack;
    tracing::info!(
        correlation_id = %env.correlation,
        "rollback completed; plan rolled back"
    );
    let _ = env
        .events
        .publish(&event(
            types::PLAN_ROLLED_BACK,
            json!({}),
            env.correlation,
            None,
        ))
        .await;
}

/// Whether the target is already in the action's desired state, read from live
/// state at execution time. `Ok(false)` means the effect should run.
///
/// The desired-state mapping itself is shared with the post-execution validator
/// via `argus_validate::desired_state` (ADR-0031 §2); the live read is
/// family-dispatched: units through the service controller, containers through
/// the container controller, cgroup subtrees through the cgroup controller.
fn desired_state_holds(action: &Action, env: &RunEnv<'_>) -> Result<bool, String> {
    let Some(desired) = desired_state(&action.capability) else {
        return Ok(false);
    };
    let observed = observe_target(action, env)?;
    Ok(observed == desired)
}

/// Reads the live state a desired-state check or a validation compares against.
fn observe_target(action: &Action, env: &RunEnv<'_>) -> Result<bool, String> {
    let capability = &action.capability;
    if is_service_action(capability) {
        let unit = action
            .arguments
            .get("unit")
            .and_then(Value::as_str)
            .ok_or_else(|| "action missing 'unit' argument".to_string())?;
        return env.service.is_active(unit).map_err(|e| e.to_string());
    }
    match capability.as_str() {
        CapabilityId::CONTAINER_RESTART => {
            let id = action
                .arguments
                .get("container")
                .and_then(Value::as_str)
                .ok_or_else(|| "action missing 'container' argument".to_string())?;
            env.ports
                .containers
                .is_running(id)
                .map_err(|e| e.to_string())
        }
        CapabilityId::HOST_CGROUP_FREEZE | CapabilityId::HOST_CGROUP_THAW => {
            let path = action
                .arguments
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| "action missing 'path' argument".to_string())?;
            env.ports.cgroups.is_frozen(path).map_err(|e| e.to_string())
        }
        other => Err(format!("no live-state read for capability '{other}'")),
    }
}

/// Re-observes a just-executed step's target and publishes the validation
/// outcome (ADR-0031 §6). Fail closed: a live-state read failure publishes
/// nothing and records the failure.
async fn validate_step(action: &Action, env: &RunEnv<'_>) {
    let Some(expected) = desired_state(&action.capability) else {
        return;
    };
    let Some(target) = action_target(action) else {
        return;
    };
    let observed = match observe_target(action, env) {
        Ok(observed) => observed,
        Err(error) => {
            // Fail closed: no pass is fabricated when the read fails; record it.
            let payload = read_failure_event(
                &action.capability,
                target,
                &env.context_hash,
                &error.to_string(),
            );
            let _ = env
                .events
                .publish(&event(
                    argus_validate::VALIDATION_READ_FAILED,
                    payload,
                    env.correlation,
                    None,
                ))
                .await;
            return;
        }
    };
    // The shared payload construction (argus_validate) is the single source for
    // both daemon hooks, so they cannot drift (ADR-0031 §6).
    let Some((event_type, payload)) = validation_event(
        &action.capability,
        target,
        expected,
        observed,
        &env.context_hash,
    ) else {
        return;
    };
    let _ = env
        .events
        .publish(&event(event_type, payload, env.correlation, None))
        .await;
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
    use argus_domain::{BlastRadius, CapabilityDescriptor, PlanStatus, PlanStep, Reversibility};
    use argus_events::LocalEventBus;
    use argus_executor::MockServiceController;
    use argus_policy::{ApprovalStore, BootstrapPolicyEvaluator};

    fn restart_action(unit: &str) -> Action {
        Action {
            capability: CapabilityId::new(CapabilityId::HOST_SERVICE_RESTART).unwrap(),
            resource: None,
            arguments: json!({ "unit": unit }),
        }
    }

    /// A registry mirroring the bootstrap service descriptors: the three service
    /// capabilities declare a per-invocation approval requirement, so the plan
    /// loop sources `RequireApproval` from the descriptor, not a hardcoded list.
    fn service_registry() -> CapabilityRegistry {
        let mut registry = CapabilityRegistry::new();
        for capability in [
            CapabilityId::HOST_SERVICE_RESTART,
            CapabilityId::HOST_SERVICE_STOP,
            CapabilityId::HOST_SERVICE_START,
        ] {
            let id = CapabilityId::new(capability).expect("bootstrap capability id");
            let descriptor = CapabilityDescriptor::new(
                id,
                "argusd",
                capability,
                RiskClass::LowRisk,
                Version::new(0, 1, 0),
                json!({
                    "type": "object",
                    "required": ["unit"],
                    "properties": { "unit": { "type": "string" } },
                    "additionalProperties": false,
                }),
                json!({}),
                Reversibility::Reversible,
            )
            .with_blast_radius(BlastRadius::Host)
            .requiring_approval();
            registry.register(descriptor).expect("unique descriptor");
        }
        registry
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

    /// Pauses the plan, grants its token, resumes, and returns the executed
    /// outcome. The common happy path shared by the execution-mechanics tests.
    async fn run_approved(
        plan: &Plan,
        policy: &BootstrapPolicyEvaluator,
        service: &dyn ServiceController,
        events: &dyn EventBus,
        autonomy: AutonomyMode,
    ) -> ExecutionOutcome {
        let registry = service_registry();
        let approvals = ApprovalStore::new();
        let pending =
            match authorize_and_run(plan, &registry, policy, service, events, autonomy).await {
                RunOutcome::Pending(pending) => pending,
                RunOutcome::Finished(outcome) => {
                    panic!("expected a pause, got a finished outcome: {outcome:?}")
                }
            };
        approvals.grant_for_a_while(
            pending.token,
            pending.context_hash.clone(),
            "operator",
            Utc::now(),
            chrono::Duration::minutes(5),
        );
        match resume_and_run(
            &pending, &approvals, &registry, policy, service, events, autonomy,
        )
        .await
        {
            ResumeOutcome::Finished(outcome) => outcome,
            ResumeOutcome::Refused(reason) => {
                panic!("expected the resume to run, refused: {reason:?}")
            }
        }
    }

    /// Pauses a service-action plan and returns the pending approval.
    async fn pause(service: &dyn ServiceController) -> PendingPlan {
        let events = Arc::new(LocalEventBus::new(16));
        let policy = BootstrapPolicyEvaluator::new();
        let registry = service_registry();
        match authorize_and_run(
            &plan("nginx.service"),
            &registry,
            &policy,
            service,
            events.as_ref(),
            AutonomyMode::Assisted,
        )
        .await
        {
            RunOutcome::Pending(pending) => pending,
            RunOutcome::Finished(outcome) => {
                panic!("expected a pause, got a finished outcome: {outcome:?}")
            }
        }
    }

    #[tokio::test]
    async fn a_service_action_pauses_for_approval_and_executes_nothing() {
        let service = MockServiceController::new();
        let events = Arc::new(LocalEventBus::new(16));
        let policy = BootstrapPolicyEvaluator::new();
        let registry = service_registry();

        let outcome = authorize_and_run(
            &plan("nginx.service"),
            &registry,
            &policy,
            &service,
            events.as_ref(),
            AutonomyMode::Assisted,
        )
        .await;

        match outcome {
            RunOutcome::Pending(pending) => {
                assert_eq!(pending.plan.status, PlanStatus::AwaitingApproval);
                assert!(!pending.token.is_nil(), "a single-use token is recorded");
                assert_eq!(pending.context_hash, plan_context_hash(&pending.plan));
                assert!(!pending.context_hash.is_empty());
            }
            RunOutcome::Finished(outcome) => {
                panic!("expected a pause, got a finished outcome: {outcome:?}")
            }
        }
        assert!(
            service.recorded_calls().is_empty(),
            "nothing executes on pause"
        );
    }

    #[tokio::test]
    async fn a_matching_unexpired_grant_resumes_and_consumes_the_token() {
        let service = MockServiceController::new();
        let events = Arc::new(LocalEventBus::new(16));
        let policy = BootstrapPolicyEvaluator::new();
        let registry = service_registry();
        let approvals = ApprovalStore::new();

        let pending = match authorize_and_run(
            &plan("nginx.service"),
            &registry,
            &policy,
            &service,
            events.as_ref(),
            AutonomyMode::Assisted,
        )
        .await
        {
            RunOutcome::Pending(pending) => pending,
            RunOutcome::Finished(outcome) => {
                panic!("expected a pause, got a finished outcome: {outcome:?}")
            }
        };
        approvals.grant_for_a_while(
            pending.token,
            pending.context_hash.clone(),
            "operator",
            Utc::now(),
            chrono::Duration::minutes(5),
        );

        let outcome = resume_and_run(
            &pending,
            &approvals,
            &registry,
            &policy,
            &service,
            events.as_ref(),
            AutonomyMode::Assisted,
        )
        .await;

        match outcome {
            ResumeOutcome::Finished(outcome) => {
                assert_eq!(outcome.executions.len(), 1);
                assert_eq!(outcome.executions[0].status, ExecutionStatus::Completed);
                assert!(outcome.denied.is_empty());
                assert!(outcome.requires_approval.is_empty());
            }
            ResumeOutcome::Refused(reason) => {
                panic!("expected the resume to run, refused: {reason:?}")
            }
        }
        assert_eq!(
            service.recorded_calls(),
            vec![("restart".to_string(), "nginx.service".to_string())]
        );
        assert!(
            approvals.get(pending.token).is_none(),
            "the token is consumed exactly once"
        );
    }

    #[tokio::test]
    async fn a_grant_with_a_different_context_hash_is_refused_as_stale() {
        let service = MockServiceController::new();
        let pending = pause(&service).await;
        let events = Arc::new(LocalEventBus::new(16));
        let policy = BootstrapPolicyEvaluator::new();
        let registry = service_registry();
        let approvals = ApprovalStore::new();
        approvals.grant_for_a_while(
            pending.token,
            "a-different-hash",
            "operator",
            Utc::now(),
            chrono::Duration::minutes(5),
        );

        let outcome = resume_and_run(
            &pending,
            &approvals,
            &registry,
            &policy,
            &service,
            events.as_ref(),
            AutonomyMode::Assisted,
        )
        .await;

        assert_eq!(outcome_refusal(&outcome), ResumeRefusal::Stale);
        assert!(
            service.recorded_calls().is_empty(),
            "a stale grant executes nothing"
        );
    }

    #[tokio::test]
    async fn an_already_consumed_token_is_refused_on_a_second_resume() {
        let service = MockServiceController::new();
        let events = Arc::new(LocalEventBus::new(16));
        let policy = BootstrapPolicyEvaluator::new();
        let registry = service_registry();
        let approvals = ApprovalStore::new();

        let pending = match authorize_and_run(
            &plan("nginx.service"),
            &registry,
            &policy,
            &service,
            events.as_ref(),
            AutonomyMode::Assisted,
        )
        .await
        {
            RunOutcome::Pending(pending) => pending,
            RunOutcome::Finished(outcome) => {
                panic!("expected a pause, got a finished outcome: {outcome:?}")
            }
        };
        approvals.grant_for_a_while(
            pending.token,
            pending.context_hash.clone(),
            "operator",
            Utc::now(),
            chrono::Duration::minutes(5),
        );

        let first = resume_and_run(
            &pending,
            &approvals,
            &registry,
            &policy,
            &service,
            events.as_ref(),
            AutonomyMode::Assisted,
        )
        .await;
        assert!(matches!(first, ResumeOutcome::Finished(_)));

        let second = resume_and_run(
            &pending,
            &approvals,
            &registry,
            &policy,
            &service,
            events.as_ref(),
            AutonomyMode::Assisted,
        )
        .await;
        assert_eq!(outcome_refusal(&second), ResumeRefusal::NoGrant);
        assert_eq!(
            service.recorded_calls(),
            vec![("restart".to_string(), "nginx.service".to_string())],
            "a replayed token executes nothing more"
        );
    }

    #[tokio::test]
    async fn a_denied_grant_never_executes() {
        let service = MockServiceController::new();
        let pending = pause(&service).await;
        let events = Arc::new(LocalEventBus::new(16));
        let policy = BootstrapPolicyEvaluator::new();
        let registry = service_registry();
        let approvals = ApprovalStore::new();
        approvals.deny(
            pending.token,
            pending.context_hash.clone(),
            "operator",
            Utc::now(),
        );

        let outcome = resume_and_run(
            &pending,
            &approvals,
            &registry,
            &policy,
            &service,
            events.as_ref(),
            AutonomyMode::Assisted,
        )
        .await;

        assert_eq!(outcome_refusal(&outcome), ResumeRefusal::NoGrant);
        assert!(service.recorded_calls().is_empty());
    }

    #[tokio::test]
    async fn an_expired_grant_never_executes() {
        let service = MockServiceController::new();
        let pending = pause(&service).await;
        let events = Arc::new(LocalEventBus::new(16));
        let policy = BootstrapPolicyEvaluator::new();
        let registry = service_registry();
        let approvals = ApprovalStore::new();
        let now = Utc::now();
        approvals.grant_for(
            pending.token,
            pending.context_hash.clone(),
            "operator",
            now - chrono::Duration::minutes(10),
            now - chrono::Duration::minutes(5),
        );

        let outcome = resume_and_run(
            &pending,
            &approvals,
            &registry,
            &policy,
            &service,
            events.as_ref(),
            AutonomyMode::Assisted,
        )
        .await;

        assert_eq!(outcome_refusal(&outcome), ResumeRefusal::NoGrant);
        assert!(service.recorded_calls().is_empty());
    }

    #[tokio::test]
    async fn no_grant_keeps_the_plan_paused() {
        let service = MockServiceController::new();
        let pending = pause(&service).await;
        let events = Arc::new(LocalEventBus::new(16));
        let policy = BootstrapPolicyEvaluator::new();
        let registry = service_registry();
        let approvals = ApprovalStore::new();

        let outcome = resume_and_run(
            &pending,
            &approvals,
            &registry,
            &policy,
            &service,
            events.as_ref(),
            AutonomyMode::Assisted,
        )
        .await;

        assert_eq!(outcome_refusal(&outcome), ResumeRefusal::NoGrant);
        assert!(service.recorded_calls().is_empty());
    }

    /// Extracts the refusal from a resume outcome, panicking on a finished run.
    fn outcome_refusal(outcome: &ResumeOutcome) -> ResumeRefusal {
        match outcome {
            ResumeOutcome::Refused(reason) => *reason,
            ResumeOutcome::Finished(outcome) => {
                panic!("expected a refusal, got a finished outcome: {outcome:?}")
            }
        }
    }

    #[tokio::test]
    async fn unknown_capability_is_denied_by_policy() {
        let service = MockServiceController::new();
        let events = Arc::new(LocalEventBus::new(16));
        let policy = BootstrapPolicyEvaluator::new();
        let registry = service_registry();

        let mut p = plan("nginx.service");
        p.steps[0].action.capability = CapabilityId::new("host.process.signal").unwrap();

        let outcome = match authorize_and_run(
            &p,
            &registry,
            &policy,
            &service,
            events.as_ref(),
            AutonomyMode::Assisted,
        )
        .await
        {
            RunOutcome::Finished(outcome) => outcome,
            RunOutcome::Pending(pending) => {
                panic!("expected a finished run, got a pause: {pending:?}")
            }
        };

        assert!(outcome.executions.is_empty());
        assert_eq!(outcome.denied.len(), 1);
        assert_eq!(outcome.status, PlanStatus::Denied);
        assert!(service.recorded_calls().is_empty());
    }

    #[tokio::test]
    async fn a_policy_allowed_read_only_action_is_gated_in_propose_mode() {
        let service = MockServiceController::new();
        let events = Arc::new(LocalEventBus::new(16));
        let policy = BootstrapPolicyEvaluator::new();
        let mut registry = service_registry();
        // The read-only capability must be registered (and not declare approval)
        // so the policy allows it and the autonomy gate — not the approval gate —
        // is what stops it.
        registry
            .register(
                CapabilityDescriptor::new(
                    CapabilityId::new(CapabilityId::HOST_STATUS_READ).unwrap(),
                    "argusd",
                    CapabilityId::HOST_STATUS_READ,
                    RiskClass::Read,
                    Version::new(0, 1, 0),
                    json!({ "type": "object", "additionalProperties": false }),
                    json!({}),
                    Reversibility::None,
                )
                .with_blast_radius(BlastRadius::None),
            )
            .expect("unique descriptor");

        // A read-only capability is allowed by policy (no approval requirement)
        // but still gated by the autonomy mode: Propose executes nothing.
        let mut p = plan("nginx.service");
        p.steps[0].action.capability = CapabilityId::new(CapabilityId::HOST_STATUS_READ).unwrap();

        let outcome = match authorize_and_run(
            &p,
            &registry,
            &policy,
            &service,
            events.as_ref(),
            AutonomyMode::Propose,
        )
        .await
        {
            RunOutcome::Finished(outcome) => outcome,
            RunOutcome::Pending(pending) => {
                panic!("expected a finished run, got a pause: {pending:?}")
            }
        };

        assert!(outcome.executions.is_empty(), "Propose mode never executes");
        assert_eq!(outcome.requires_approval.len(), 1);
        assert_eq!(outcome.status, PlanStatus::Denied);
        assert!(service.recorded_calls().is_empty());
    }

    #[tokio::test]
    async fn an_unregistered_service_step_still_requires_approval() {
        let service = MockServiceController::new();
        let events = Arc::new(LocalEventBus::new(16));
        let policy = BootstrapPolicyEvaluator::new();
        // No service descriptor is registered: the approval gate must fail
        // closed rather than defaulting to "no approval required".
        let registry = CapabilityRegistry::new();

        let outcome = authorize_and_run(
            &plan("nginx.service"),
            &registry,
            &policy,
            &service,
            events.as_ref(),
            AutonomyMode::Assisted,
        )
        .await;

        match outcome {
            RunOutcome::Pending(_) => {}
            RunOutcome::Finished(_) => {
                panic!("an unregistered service step must pause for approval, not execute");
            }
        }
        assert!(service.recorded_calls().is_empty());
    }

    #[tokio::test]
    async fn missing_unit_argument_fails_execution() {
        let service = MockServiceController::new();
        let events = Arc::new(LocalEventBus::new(16));
        let policy = BootstrapPolicyEvaluator::new();

        let mut p = plan("nginx.service");
        p.steps[0].action.arguments = json!({});

        let outcome = run_approved(
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

        let outcome = run_approved(
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

        let outcome = run_approved(
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
        let outcome = run_approved(
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

        let outcome = run_approved(
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

        let outcome = run_approved(
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

        let outcome = run_approved(
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

    #[tokio::test]
    async fn validation_is_published_after_an_executed_step() {
        // The unit starts inactive so the restart effect runs (not a no-op),
        // then the mock's `restart` marks it active — so the post-execution
        // re-observation matches the desired state and publishes a pass.
        let service = MockServiceController::new();
        let events = Arc::new(LocalEventBus::new(16));
        let mut rx = events.subscribe();
        let policy = BootstrapPolicyEvaluator::new();

        let outcome = run_approved(
            &plan("nginx.service"),
            &policy,
            &service,
            events.as_ref(),
            AutonomyMode::Assisted,
        )
        .await;
        assert_eq!(outcome.status, PlanStatus::Completed);

        // The plan-path hook publishes a validation.passed event.
        let mut saw_passed = false;
        while let Ok(ev) = rx.try_recv() {
            if ev.event_type().as_str() == "validation.passed" {
                saw_passed = true;
            }
        }
        assert!(
            saw_passed,
            "a validation.passed event is published after the step"
        );
    }
}
