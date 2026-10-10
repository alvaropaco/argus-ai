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
    let noop = control::LoopPorts::noop();
    control::LoopPorts {
        governor: noop.governor,
        containers: noop.containers,
        cgroups: noop.cgroups,
        cluster: noop.cluster,
        remediation: noop.remediation,
        kubernetes: noop.kubernetes,
        ledger: ledger.as_ref(),
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
