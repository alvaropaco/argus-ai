//! Spec-004 FR-004 wiring tests: at L4/L5 the escalation decision gates the
//! Allow branch (anything short of AUTO-FIX pauses for approval), while
//! L0–L3 semantics stay byte-for-byte unchanged. Policy remains final: the
//! gate only narrows.

use std::sync::{Arc, Mutex};

use argus_daemon::config::DaemonConfig;
use argus_daemon::{Daemon, control};
use argus_domain::{
    Action, AutonomyMode, BlastRadius, CapabilityDescriptor, CapabilityId, CapabilityRegistry,
    Plan, PlanStatus, PlanStep, Reversibility, RiskClass,
};
use argus_events::LocalEventBus;
use argus_executor::{
    CgroupController, ExecutionError, ExecutionResult, Executor, MockServiceController,
    RemediationError, RemediationExecutor, remediation_guardrails,
};
use argus_policy::{AutopilotGovernor, BootstrapPolicyEvaluator, Criticality};
use semver::Version as Semver;
use serde_json::json;

#[derive(Default)]
struct MockCgroups {
    calls: Mutex<Vec<String>>,
}

impl CgroupController for MockCgroups {
    fn set_freeze(&self, path: &str, _frozen: bool) -> Result<(), RemediationError> {
        self.calls.lock().unwrap().push(path.to_string());
        Ok(())
    }
    fn is_frozen(&self, _path: &str) -> Result<bool, RemediationError> {
        Ok(false)
    }
}

struct MockContainers;
impl argus_executor::ContainerController for MockContainers {
    fn restart(&self, _id: &str) -> Result<(), RemediationError> {
        Ok(())
    }
    fn is_running(&self, _id: &str) -> Result<bool, RemediationError> {
        Ok(true)
    }
}

fn registry() -> CapabilityRegistry {
    let mut registry = CapabilityRegistry::new();
    for (capability, risk) in [
        (CapabilityId::HOST_CGROUP_FREEZE, RiskClass::LowRisk),
        (CapabilityId::HOST_CGROUP_THAW, RiskClass::LowRisk),
    ] {
        let descriptor = CapabilityDescriptor::new(
            CapabilityId::new(capability).unwrap(),
            "argusd",
            capability,
            risk,
            Semver::new(0, 1, 0),
            json!({
                "type": "object",
                "required": ["path"],
                "properties": { "path": { "type": "string" } },
                "additionalProperties": false,
            }),
            json!({}),
            Reversibility::Reversible,
        )
        .with_blast_radius(BlastRadius::Host);
        registry.register(descriptor).unwrap();
    }
    registry
}

fn freeze(path: &str) -> Action {
    Action {
        capability: CapabilityId::new(CapabilityId::HOST_CGROUP_FREEZE).unwrap(),
        resource: None,
        arguments: json!({ "path": path }),
    }
}

fn thaw(path: &str) -> Action {
    Action {
        capability: CapabilityId::new(CapabilityId::HOST_CGROUP_THAW).unwrap(),
        resource: None,
        arguments: json!({ "path": path }),
    }
}

fn plan_with_confidence(confidence: f64, rollback: bool) -> Plan {
    Plan {
        objective: "freeze the noisy consumer".into(),
        steps: vec![PlanStep {
            action: freeze("workload.batch/exporter"),
            rollback: rollback.then(|| thaw("workload.batch/exporter")),
        }],
        preconditions: vec![],
        expected_outcomes: vec![],
        blast_radius: BlastRadius::Host,
        confidence,
        status: PlanStatus::Proposed,
    }
}

fn ports<'a>(
    governor: &'a AutopilotGovernor,
    cgroups: &'a MockCgroups,
    executor: &'a dyn Executor,
) -> control::LoopPorts<'a> {
    static CONTAINERS: MockContainers = MockContainers;
    static CLUSTER: argus_executor::UnavailableClusterController =
        argus_executor::UnavailableClusterController;
    struct NoKubernetes;
    impl Executor for NoKubernetes {
        fn execute(
            &self,
            action: &argus_executor::AuthorizedAction,
        ) -> Result<ExecutionResult, ExecutionError> {
            Err(ExecutionError::Unsupported(action.capability().clone()))
        }
    }
    static NO_KUBERNETES: NoKubernetes = NoKubernetes;
    control::LoopPorts {
        governor,
        containers: &CONTAINERS,
        cgroups,
        cluster: &CLUSTER,
        remediation: executor,
        kubernetes: &NO_KUBERNETES,
    }
}

fn harness() -> (AutopilotGovernor, Arc<MockCgroups>, RemediationExecutor) {
    // The freeze target is classified so governance permits it (AC-013
    // shape): the escalation gate is what these tests vary.
    let governor = AutopilotGovernor::default();
    governor.classify("workload.batch/exporter", Criticality::BestEffort);
    let cgroups = Arc::new(MockCgroups::default());
    let executor = RemediationExecutor::new(
        Arc::new(argus_executor::UnixProcessController::new()),
        Arc::new(MockContainers),
        cgroups.clone(),
        remediation_guardrails(&[], &[]),
    );
    (governor, cgroups, executor)
}

async fn run(plan: &Plan, autonomy: AutonomyMode) -> control::RunOutcome {
    let (governor, cgroups, executor) = harness();
    let p = ports(&governor, &cgroups, &executor);
    control::authorize_and_run_with_ports(
        plan,
        &registry(),
        &BootstrapPolicyEvaluator::with_local_remediation(),
        &MockServiceController::new(),
        &LocalEventBus::new(64),
        autonomy,
        &p,
    )
    .await
}

#[tokio::test]
async fn l4_executes_a_high_confidence_reversible_rollback_declared_step() {
    let outcome = run(
        &plan_with_confidence(0.95, true),
        AutonomyMode::L4Autonomous,
    )
    .await;
    let control::RunOutcome::Finished(finished) = outcome else {
        panic!("a strong L4 step executes without approval");
    };
    assert_eq!(finished.status, PlanStatus::Completed);
    assert!(finished.requires_approval.is_empty());
}

#[tokio::test]
async fn l4_routes_a_low_confidence_step_to_a_human() {
    let outcome = run(&plan_with_confidence(0.3, true), AutonomyMode::L4Autonomous).await;
    let control::RunOutcome::Pending(pending) = outcome else {
        panic!("a low-confidence L4 step pauses for approval");
    };
    assert_eq!(pending.plan.status, PlanStatus::AwaitingApproval);
    // The paused plan is approvable: a grant + resume finishes it.
    let approvals = argus_policy::ApprovalStore::default();
    approvals.grant_for_a_while(
        pending.token,
        &pending.context_hash,
        "operator",
        chrono::Utc::now(),
        chrono::Duration::minutes(15),
    );
    let (governor, cgroups, executor) = harness();
    let p = ports(&governor, &cgroups, &executor);
    let resumed = control::resume_and_run_with_ports(
        &pending,
        &approvals,
        &registry(),
        &BootstrapPolicyEvaluator::with_local_remediation(),
        &MockServiceController::new(),
        &LocalEventBus::new(64),
        AutonomyMode::L3Assisted,
        &p,
    )
    .await;
    let control::ResumeOutcome::Finished(finished) = resumed else {
        panic!("the resumed plan finishes")
    };
    assert_eq!(finished.status, PlanStatus::Completed);
}

#[tokio::test]
async fn l4_routes_an_irreversible_step_to_a_human_even_at_high_confidence() {
    // No declared rollback and not a read: not reversible.
    let outcome = run(
        &plan_with_confidence(0.95, false),
        AutonomyMode::L4Autonomous,
    )
    .await;
    assert!(
        matches!(outcome, control::RunOutcome::Pending(_)),
        "an irreversible L4 step pauses for approval"
    );
}

#[tokio::test]
async fn l3_semantics_are_unchanged_by_the_escalation_gate() {
    // The same low-confidence, rollback-less plan that pauses at L4 still
    // executes at L3: L3's contract (policy-permitted low-risk) is untouched.
    let outcome = run(&plan_with_confidence(0.3, false), AutonomyMode::L3Assisted).await;
    let control::RunOutcome::Finished(finished) = outcome else {
        panic!("L3 behavior is unchanged by the escalation gate");
    };
    assert_eq!(finished.status, PlanStatus::Completed);
}

#[tokio::test]
async fn l5_behaves_like_l4_for_the_gate() {
    let strong = run(&plan_with_confidence(0.95, true), AutonomyMode::L5Adaptive).await;
    assert!(matches!(strong, control::RunOutcome::Finished(_)));
    let weak = run(&plan_with_confidence(0.3, true), AutonomyMode::L5Adaptive).await;
    assert!(matches!(weak, control::RunOutcome::Pending(_)));
}

#[tokio::test]
async fn the_sentinel_snapshot_reflects_live_daemon_state() {
    let state_path = std::env::temp_dir()
        .join(format!("argus-sentinel-test-{}.db", std::process::id()))
        .to_string_lossy()
        .into_owned();
    let config = DaemonConfig {
        state_path,
        ..DaemonConfig::default()
    };
    let daemon = Daemon::init(config).await.unwrap();

    let view = daemon.sentinel_snapshot().await;
    assert!(view.provider_ready, "a fresh daemon's provider is ready");
    assert_eq!(view.pending_approvals, 0);
    assert_eq!(view.recent_actions, 0);
    assert_eq!(view.open_incidents, 0);
    assert!(view.predictions.is_empty());
    assert!(view.pressure.is_empty());
    assert_eq!(
        serde_json::to_value(&view).unwrap()["safe_mode"],
        serde_json::json!("none")
    );

    // A finished run's counters are observable for status surfaces.
    assert_eq!(
        daemon.self_observability().autonomous_success_rate(),
        None,
        "no autonomous action has run yet"
    );
}
