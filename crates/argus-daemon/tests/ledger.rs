//! The action ledger at the control-loop chokepoints (spec 007 FR-001).
//!
//! The plan loop is the one path from a proposed plan to execution, so a
//! recording sink wired through [`control::LoopPorts`] sees every verdict:
//! allows, denials, approval pauses, and rollbacks — with the plan's
//! correlation as the linkage id.

use std::sync::{Arc, Mutex};

use argus_daemon::control;
use argus_domain::{
    Action, BlastRadius, CapabilityDescriptor, CapabilityId, CapabilityRegistry, Plan, PlanStatus,
    PlanStep, Reversibility, RiskClass,
};
use argus_events::{LedgerSink, LocalEventBus};
use argus_executor::MockServiceController;
use argus_policy::{ApprovalStore, BootstrapPolicyEvaluator};
use chrono::Utc;
use semver::Version;
use serde_json::json;

/// Records everything the loop emits.
#[derive(Default)]
struct RecordingLedger {
    actions: Mutex<Vec<argus_domain::ActionEventRecord>>,
}

impl RecordingLedger {
    fn snapshot(&self) -> Vec<argus_domain::ActionEventRecord> {
        self.actions.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl LedgerSink for RecordingLedger {
    async fn record_action(&self, event: argus_domain::ActionEventRecord) {
        self.actions.lock().unwrap().push(event);
    }
    async fn record_trace(&self, _trace: argus_domain::BrainTraceRecord) {}
    async fn record_usage(&self, _usage: argus_domain::TokenUsageRecord) {}
}

fn registry() -> CapabilityRegistry {
    let mut registry = CapabilityRegistry::new();
    for capability in [
        CapabilityId::HOST_SERVICE_RESTART,
        CapabilityId::HOST_SERVICE_STOP,
        CapabilityId::HOST_SERVICE_START,
    ] {
        let id = CapabilityId::new(capability).expect("bootstrap capability id");
        registry
            .register(
                CapabilityDescriptor::new(
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
                .requiring_approval(),
            )
            .expect("unique descriptor");
    }
    registry
}

fn plan() -> Plan {
    Plan {
        objective: "restore nginx".into(),
        steps: vec![PlanStep {
            action: Action {
                capability: CapabilityId::new(CapabilityId::HOST_SERVICE_RESTART).unwrap(),
                resource: None,
                arguments: json!({ "unit": "nginx.service" }),
            },
            rollback: None,
        }],
        preconditions: vec![],
        expected_outcomes: vec![],
        blast_radius: BlastRadius::Host,
        confidence: 0.9,
        status: PlanStatus::Proposed,
        runbook: None,
    }
}

/// Pauses the plan, grants it, resumes — the ordinary approval round-trip —
/// with the recording sink in the ports.
async fn run_approved(ledger: &Arc<RecordingLedger>) -> control::ExecutionOutcome {
    let registry = registry();
    let policy = BootstrapPolicyEvaluator::new();
    let service = MockServiceController::new();
    let events = LocalEventBus::new(64);
    let approvals = ApprovalStore::new();

    let pending = match control::authorize_and_run_with_ports(
        &plan(),
        &registry,
        &policy,
        &service,
        &events,
        argus_domain::AutonomyMode::L3Assisted,
        &ports_with_ledger(ledger),
    )
    .await
    {
        control::RunOutcome::Pending(pending) => pending,
        control::RunOutcome::Finished(outcome) => {
            panic!("expected a pause, got: {outcome:?}")
        }
    };

    approvals.grant_for_a_while(
        pending.token,
        pending.context_hash.clone(),
        "operator",
        Utc::now(),
        chrono::Duration::minutes(5),
    );
    match control::resume_and_run_with_ports(
        &pending,
        &approvals,
        &registry,
        &policy,
        &service,
        &events,
        argus_domain::AutonomyMode::L3Assisted,
        &ports_with_ledger(ledger),
    )
    .await
    {
        control::ResumeOutcome::Finished(outcome) => outcome,
        control::ResumeOutcome::Refused(reason) => panic!("expected a resume, refused: {reason:?}"),
    }
}

/// The noop execution surface with the recording ledger swapped in.
fn ports_with_ledger(ledger: &Arc<RecordingLedger>) -> control::LoopPorts<'_> {
    static NO_BUDGET: argus_daemon::autonomy::NoopBudget = argus_daemon::autonomy::NoopBudget;
    ports_with_budget(ledger, &NO_BUDGET)
}

/// [`ports_with_ledger`] with the blast-radius budget gate swapped too —
/// the seam the spec-008 tests preset.
fn ports_with_budget<'a>(
    ledger: &'a Arc<RecordingLedger>,
    budget: &'a dyn argus_daemon::autonomy::BudgetGate,
) -> control::LoopPorts<'a> {
    let noop = control::LoopPorts::noop();
    control::LoopPorts {
        governor: noop.governor,
        containers: noop.containers,
        cgroups: noop.cgroups,
        cluster: noop.cluster,
        remediation: noop.remediation,
        kubernetes: noop.kubernetes,
        ledger: ledger.as_ref(),
        budget,
    }
}

#[tokio::test]
async fn the_approval_pause_and_the_execution_both_reach_the_ledger() {
    let ledger = Arc::new(RecordingLedger::default());
    let outcome = run_approved(&ledger).await;
    assert_eq!(outcome.status, PlanStatus::Completed);

    let events = ledger.snapshot();
    // First the pause (requires_approval), then the executed step (allow/ok).
    assert_eq!(
        events.len(),
        2,
        "one record per execution attempt: {events:?}"
    );

    let paused = &events[0];
    assert_eq!(paused.verdict, "requires_approval");
    assert_eq!(paused.outcome, "awaiting_approval");
    assert_eq!(paused.kind, "host.service.restart");
    assert_eq!(paused.target.as_deref(), Some("nginx.service"));

    let executed = &events[1];
    assert_eq!(executed.verdict, "allow");
    assert_eq!(executed.outcome, "ok");
    assert!(
        executed.duration_ms.is_some(),
        "the executed action carries its duration"
    );
    assert_eq!(
        executed.policy_id.as_deref(),
        Some("plan.approval"),
        "the consumed grant is the recorded allow"
    );

    // The chain: each record carries the plan linkage of its own run (a
    // resume re-enters as a new run id — the plan.approval grant records why).
    assert_eq!(
        executed.plan_id,
        Some(executed.correlation_id),
        "plan_id is the run's correlation"
    );
    assert_eq!(
        paused.plan_id,
        Some(paused.correlation_id),
        "plan_id is the run's correlation"
    );
    assert!(
        !executed.args.is_null(),
        "raw arguments ride along for the local ledger"
    );
}

/// A policy-denied step is a first-class ledger row (AC-002).
#[tokio::test]
async fn a_policy_denial_lands_in_the_ledger() {
    let ledger = Arc::new(RecordingLedger::default());
    let registry = registry();
    let policy = BootstrapPolicyEvaluator::new();
    let service = MockServiceController::new();
    let events = LocalEventBus::new(64);

    let mut denied_plan = plan();
    denied_plan.steps[0].action.capability = CapabilityId::new("host.process.signal").unwrap();
    denied_plan.steps[0].action.arguments = json!({ "pid": 123 });

    let outcome = control::authorize_and_run_with_ports(
        &denied_plan,
        &registry,
        &policy,
        &service,
        &events,
        argus_domain::AutonomyMode::L3Assisted,
        &ports_with_ledger(&ledger),
    )
    .await;

    let control::RunOutcome::Finished(finished) = outcome else {
        panic!("a denial cannot pause");
    };
    assert_eq!(finished.status, PlanStatus::Denied);

    let events = ledger.snapshot();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].verdict, "deny");
    assert_eq!(events[0].outcome, "denied");
    assert_eq!(events[0].kind, "host.process.signal");
    assert!(
        service.recorded_calls().is_empty(),
        "a denied action never executes"
    );
}

// --- Blast-radius budget gate (spec 008 FR-004, AC-004) ---

/// A budget gate the tests preset: `available` says whether a reservation
/// succeeds, and every reserve/refund lands here tagged with its operation,
/// risk, and scope — so a test can see exactly what the loop consumed and
/// what it gave back.
struct RecordingBudget {
    available: std::sync::atomic::AtomicBool,
    records: Mutex<Vec<(&'static str, RiskClass, BlastRadius)>>,
}

impl RecordingBudget {
    fn exhausted() -> Self {
        Self {
            available: std::sync::atomic::AtomicBool::new(false),
            records: Mutex::new(Vec::new()),
        }
    }

    fn available() -> Self {
        Self {
            available: std::sync::atomic::AtomicBool::new(true),
            records: Mutex::new(Vec::new()),
        }
    }

    fn snapshot(&self) -> Vec<(&'static str, RiskClass, BlastRadius)> {
        self.records.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl argus_daemon::autonomy::BudgetGate for RecordingBudget {
    async fn reserve(
        &self,
        risk: RiskClass,
        scope: BlastRadius,
        _now: chrono::DateTime<chrono::Utc>,
    ) -> bool {
        use std::sync::atomic::Ordering;
        let reserved = self.available.load(Ordering::SeqCst);
        if reserved {
            // Only a successful reserve is a consumption; an exhausted
            // window logs nothing.
            self.records.lock().unwrap().push(("reserve", risk, scope));
        }
        reserved
    }

    async fn refund(
        &self,
        risk: RiskClass,
        scope: BlastRadius,
        _now: chrono::DateTime<chrono::Utc>,
    ) {
        self.records.lock().unwrap().push(("refund", risk, scope));
    }
}

/// A registry whose low-risk service capabilities declare no per-invocation
/// approval, so at L3 the step reaches the Allow branch and auto-executes —
/// the budget gate is what decides its fate.
fn registry_auto() -> CapabilityRegistry {
    let mut registry = CapabilityRegistry::new();
    for capability in [
        CapabilityId::HOST_SERVICE_RESTART,
        CapabilityId::HOST_SERVICE_STOP,
        CapabilityId::HOST_SERVICE_START,
    ] {
        let id = CapabilityId::new(capability).expect("bootstrap capability id");
        registry
            .register(
                CapabilityDescriptor::new(
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
                .with_blast_radius(BlastRadius::Host),
            )
            .expect("unique descriptor");
    }
    registry
}

/// An exhausted budget pauses the PLAN as `requires_approval` with policy id
/// `budget.exhausted` — the ledger event lands, nothing executes, and the
/// operator's grant on the issued token resumes the plan: an operator grant
/// is authority the budget does not ration (AC-004).
#[tokio::test]
async fn an_exhausted_budget_pauses_the_plan_and_a_grant_resumes_it() {
    let ledger = Arc::new(RecordingLedger::default());
    let budget = Arc::new(RecordingBudget::exhausted());
    let registry = registry_auto();
    let policy = BootstrapPolicyEvaluator::new();
    let service = MockServiceController::new();
    let events = LocalEventBus::new(64);

    let outcome = control::authorize_and_run_with_ports(
        &plan(),
        &registry,
        &policy,
        &service,
        &events,
        argus_domain::AutonomyMode::L3Assisted,
        &ports_with_budget(&ledger, budget.as_ref()),
    )
    .await;

    // The plan parks on a single-use token — approvable, not skipped.
    let control::RunOutcome::Pending(pending) = outcome else {
        panic!("exhaustion pauses the plan for approval");
    };
    assert_eq!(pending.plan.status, PlanStatus::AwaitingApproval);
    assert!(!pending.token.is_nil(), "a single-use token is issued");
    assert!(
        service.recorded_calls().is_empty(),
        "nothing executes under exhaustion"
    );

    let rows = ledger.snapshot();
    assert_eq!(rows.len(), 1, "the pause is a first-class ledger row");
    assert_eq!(rows[0].verdict, "requires_approval");
    assert_eq!(
        rows[0].policy_id.as_deref(),
        Some("budget.exhausted"),
        "the policy id names the exhausted budget"
    );
    assert_eq!(rows[0].outcome, "awaiting_approval");
    assert!(
        budget.snapshot().is_empty(),
        "a failed reserve consumed nothing"
    );

    // The operator grants the token: the resume runs under grant-backed
    // authority and executes despite the exhausted budget.
    let approvals = ApprovalStore::new();
    approvals.grant_for_a_while(
        pending.token,
        pending.context_hash.clone(),
        "operator",
        Utc::now(),
        chrono::Duration::minutes(5),
    );
    match control::resume_and_run_with_ports(
        &pending,
        &approvals,
        &registry,
        &policy,
        &service,
        &events,
        argus_domain::AutonomyMode::L3Assisted,
        &ports_with_budget(&ledger, budget.as_ref()),
    )
    .await
    {
        control::ResumeOutcome::Finished(finished) => {
            assert_eq!(finished.status, PlanStatus::Completed);
            assert_eq!(finished.executions.len(), 1, "the granted plan executed");
        }
        control::ResumeOutcome::Refused(reason) => {
            panic!("expected the resume to run, refused: {reason:?}")
        }
    }
    assert_eq!(
        service.recorded_calls(),
        vec![("restart".to_string(), "nginx.service".to_string())],
        "the granted step executed once"
    );
    assert!(
        budget.snapshot().is_empty(),
        "the grant-backed resume reserved nothing"
    );
}

/// A landed auto-execution keeps its reservation; an already-desired no-op
/// reserves and refunds (zero blast radius); an operator-approved step never
/// touches the budget at all.
#[tokio::test]
async fn reservations_track_what_actually_executed() {
    let ledger = Arc::new(RecordingLedger::default());
    let budget = Arc::new(RecordingBudget::available());
    // Bound before the local `registry` below shadows the helper's name.
    let approval_registry = registry();
    let registry = registry_auto();
    let policy = BootstrapPolicyEvaluator::new();
    let service = MockServiceController::new();
    let events = LocalEventBus::new(64);

    // The auto path: the low-risk service step lands through the Allow
    // branch and executes against the mock.
    let outcome = control::authorize_and_run_with_ports(
        &plan(),
        &registry,
        &policy,
        &service,
        &events,
        argus_domain::AutonomyMode::L3Assisted,
        &ports_with_budget(&ledger, budget.as_ref()),
    )
    .await;
    let control::RunOutcome::Finished(finished) = outcome else {
        panic!("the allowed step executes");
    };
    assert_eq!(finished.executions.len(), 1);
    assert_eq!(
        budget.snapshot(),
        vec![("reserve", RiskClass::LowRisk, BlastRadius::Host)],
        "the landed auto-execution consumed the budget"
    );

    // The same target is now already in its desired state (the restart made
    // it so): the second run is a no-op — it reserved, then surrendered the
    // unit, because nothing took effect.
    let outcome = control::authorize_and_run_with_ports(
        &plan(),
        &registry,
        &policy,
        &service,
        &events,
        argus_domain::AutonomyMode::L3Assisted,
        &ports_with_budget(&ledger, budget.as_ref()),
    )
    .await;
    let control::RunOutcome::Finished(no_op) = outcome else {
        panic!("the idempotent step finishes");
    };
    assert_eq!(
        no_op.executions[0].evidence["already_desired"], true,
        "the second run was an already-desired no-op"
    );
    assert_eq!(
        budget.snapshot(),
        vec![
            ("reserve", RiskClass::LowRisk, BlastRadius::Host),
            ("reserve", RiskClass::LowRisk, BlastRadius::Host),
            ("refund", RiskClass::LowRisk, BlastRadius::Host),
        ],
        "the no-op's reservation was surrendered"
    );

    // The operator path: an approval-requiring step (the default registry
    // declares one) pauses, is granted, and the resume executes — reserving
    // nothing, because an operator grant is not the budget's to ration.
    let approvals = ApprovalStore::new();
    let pending = match control::authorize_and_run_with_ports(
        &plan(),
        &approval_registry,
        &policy,
        &service,
        &events,
        argus_domain::AutonomyMode::L3Assisted,
        &ports_with_budget(&ledger, budget.as_ref()),
    )
    .await
    {
        control::RunOutcome::Pending(pending) => pending,
        control::RunOutcome::Finished(outcome) => panic!("expected a pause: {outcome:?}"),
    };
    approvals.grant_for_a_while(
        pending.token,
        pending.context_hash.clone(),
        "operator",
        Utc::now(),
        chrono::Duration::minutes(5),
    );
    match control::resume_and_run_with_ports(
        &pending,
        &approvals,
        &approval_registry,
        &policy,
        &service,
        &events,
        argus_domain::AutonomyMode::L3Assisted,
        &ports_with_budget(&ledger, budget.as_ref()),
    )
    .await
    {
        control::ResumeOutcome::Finished(outcome) => {
            assert_eq!(outcome.executions.len(), 1, "the approved step ran");
        }
        control::ResumeOutcome::Refused(reason) => {
            panic!("expected the resume to run, refused: {reason:?}")
        }
    }
    // Still the two reserves and one refund from runs 1 and 2: the
    // operator-approved resume added nothing.
    assert_eq!(
        budget.snapshot(),
        vec![
            ("reserve", RiskClass::LowRisk, BlastRadius::Host),
            ("reserve", RiskClass::LowRisk, BlastRadius::Host),
            ("refund", RiskClass::LowRisk, BlastRadius::Host),
        ],
        "the operator-approved resume reserved and refunded nothing"
    );
}
