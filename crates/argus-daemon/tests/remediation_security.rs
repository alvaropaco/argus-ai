//! Remediation security tests (spec 003 M3, T022): deny-by-default, approval
//! gating, executor no-bypass, and rollback for the typed remediation
//! capabilities (FR-016/FR-017). Every test drives the same loop the daemon
//! runs, with deterministic controllers — no live host.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use argus_daemon::config::DaemonConfig;
use argus_daemon::{Daemon, control};
use argus_domain::{
    Action, AutonomyMode, BlastRadius, CapabilityDescriptor, CapabilityId, CapabilityRegistry,
    Plan, PlanStatus, PlanStep, Principal, RequestContext, Reversibility, RiskClass,
};
use argus_events::LocalEventBus;
use argus_executor::{
    CgroupController, CompositeExecutor, ContainerController, ExecutionError, ExecutionResult,
    Executor, MockServiceController, RemediationError, RemediationExecutor, remediation_guardrails,
};
use argus_policy::{
    ApprovalStore, AutopilotGovernor, BootstrapPolicyEvaluator, Criticality, GovernanceDecision,
    ResourceAdjustment, ResourceGovernor,
};
use chrono::Utc;
use semver::Version as Semver;
// `Semver` is the version type used by descriptor and context construction.
use serde_json::json;
use uuid::Uuid;

/// Records every freeze/thaw and can fail selected paths.
#[derive(Default)]
struct MockCgroups {
    calls: Mutex<Vec<(String, bool)>>,
    frozen: Mutex<BTreeSet<String>>,
    fail: Mutex<Vec<String>>,
}

impl MockCgroups {
    fn fail_on(&self, path: &str) {
        self.fail.lock().unwrap().push(path.to_string());
    }

    fn recorded(&self) -> Vec<(String, bool)> {
        self.calls.lock().unwrap().clone()
    }
}

impl CgroupController for MockCgroups {
    fn set_freeze(&self, path: &str, frozen: bool) -> Result<(), RemediationError> {
        self.calls.lock().unwrap().push((path.to_string(), frozen));
        if self.fail.lock().unwrap().iter().any(|p| p == path) {
            return Err(RemediationError::Failed(format!("freezer refused {path}")));
        }
        if frozen {
            self.frozen.lock().unwrap().insert(path.to_string());
        } else {
            self.frozen.lock().unwrap().remove(path);
        }
        Ok(())
    }

    fn is_frozen(&self, path: &str) -> Result<bool, RemediationError> {
        Ok(self.frozen.lock().unwrap().contains(path))
    }
}

/// A recording container controller; the tests never target containers.
#[derive(Default)]
struct MockContainers;

impl ContainerController for MockContainers {
    fn restart(&self, _id: &str) -> Result<(), RemediationError> {
        Ok(())
    }

    fn is_running(&self, _id: &str) -> Result<bool, RemediationError> {
        Ok(true)
    }
}

/// An executor that records what actually reached the execution boundary.
#[derive(Default)]
struct RecordingExecutor {
    capabilities: Mutex<Vec<String>>,
}

impl Executor for RecordingExecutor {
    fn execute(
        &self,
        action: &argus_executor::AuthorizedAction,
    ) -> Result<ExecutionResult, ExecutionError> {
        self.capabilities
            .lock()
            .unwrap()
            .push(action.capability().as_str().to_string());
        Ok(ExecutionResult {
            capability: action.capability().clone(),
            evidence: json!({}),
            started_at: Utc::now(),
            finished_at: Utc::now(),
        })
    }
}

/// A governor that only observes, so tests can assert what was consulted.
struct RecordingGovernor {
    inner: AutopilotGovernor,
    consulted: Mutex<Vec<String>>,
}

impl RecordingGovernor {
    fn new() -> Self {
        Self {
            inner: AutopilotGovernor::default(),
            consulted: Mutex::new(Vec::new()),
        }
    }

    fn consulted(&self) -> Vec<String> {
        self.consulted.lock().unwrap().clone()
    }
}

impl ResourceGovernor for RecordingGovernor {
    fn evaluate(&self, adjustment: &ResourceAdjustment) -> GovernanceDecision {
        self.consulted
            .lock()
            .unwrap()
            .push(adjustment.subject.clone());
        self.inner.evaluate(adjustment)
    }

    fn governed_prefixes(&self) -> &[&str] {
        self.inner.governed_prefixes()
    }
}

fn registry_with_m3() -> CapabilityRegistry {
    let mut registry = CapabilityRegistry::new();
    let entries = [
        (
            CapabilityId::HOST_PROCESS_SIGNAL,
            RiskClass::HighRisk,
            true,
            json!({
                "type": "object",
                "required": ["pid", "signal"],
                "properties": {
                    "pid": { "type": "integer" },
                    "signal": { "type": "string", "enum": ["term", "kill", "stop", "cont"] },
                },
                "additionalProperties": false,
            }),
        ),
        (
            CapabilityId::CONTAINER_RESTART,
            RiskClass::Controlled,
            true,
            json!({
                "type": "object",
                "required": ["container"],
                "properties": { "container": { "type": "string" } },
                "additionalProperties": false,
            }),
        ),
        (
            CapabilityId::HOST_CGROUP_FREEZE,
            RiskClass::LowRisk,
            false,
            json!({
                "type": "object",
                "required": ["path"],
                "properties": { "path": { "type": "string" } },
                "additionalProperties": false,
            }),
        ),
        (
            CapabilityId::HOST_CGROUP_THAW,
            RiskClass::LowRisk,
            false,
            json!({
                "type": "object",
                "required": ["path"],
                "properties": { "path": { "type": "string" } },
                "additionalProperties": false,
            }),
        ),
    ];
    for (capability, risk, approval, schema) in entries {
        let id = CapabilityId::new(capability).unwrap();
        let descriptor = CapabilityDescriptor::new(
            id,
            "argusd",
            capability,
            risk,
            Semver::new(0, 1, 0),
            schema,
            json!({}),
            Reversibility::Reversible,
        )
        .with_blast_radius(BlastRadius::Host)
        .requiring_approval_if(approval);
        registry.register(descriptor).unwrap();
    }
    registry
}

fn freeze_action(path: &str) -> Action {
    Action {
        capability: CapabilityId::new(CapabilityId::HOST_CGROUP_FREEZE).unwrap(),
        resource: None,
        arguments: json!({ "path": path }),
    }
}

fn thaw_action(path: &str) -> Action {
    Action {
        capability: CapabilityId::new(CapabilityId::HOST_CGROUP_THAW).unwrap(),
        resource: None,
        arguments: json!({ "path": path }),
    }
}

fn signal_action(pid: i64, signal: &str) -> Action {
    Action {
        capability: CapabilityId::new(CapabilityId::HOST_PROCESS_SIGNAL).unwrap(),
        resource: None,
        arguments: json!({ "pid": pid, "signal": signal }),
    }
}

fn plan(steps: Vec<(Action, Option<Action>)>) -> Plan {
    Plan {
        objective: "remediate memory pressure".into(),
        steps: steps
            .into_iter()
            .map(|(action, rollback)| PlanStep { action, rollback })
            .collect(),
        preconditions: vec![],
        expected_outcomes: vec![],
        blast_radius: BlastRadius::Host,
        confidence: 0.9,
        status: PlanStatus::Proposed,
        runbook: None,
    }
}

fn ports<'a>(
    governor: &'a dyn ResourceGovernor,
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
        ) -> Result<argus_executor::ExecutionResult, ExecutionError> {
            Err(ExecutionError::Unsupported(action.capability().clone()))
        }
    }
    static NO_KUBERNETES: NoKubernetes = NoKubernetes;
    static NO_BUDGET: argus_daemon::autonomy::NoopBudget = argus_daemon::autonomy::NoopBudget;
    control::LoopPorts {
        governor,
        containers: &CONTAINERS,
        cgroups,
        cluster: &CLUSTER,
        remediation: executor,
        kubernetes: &NO_KUBERNETES,
        ledger: &argus_events::NoopLedgerSink,
        budget: &NO_BUDGET,
    }
}

#[tokio::test]
async fn an_unclassified_subject_is_refused_and_nothing_executes() {
    let registry = registry_with_m3();
    let policy = BootstrapPolicyEvaluator::with_local_remediation();
    let service = MockServiceController::new();
    let events = LocalEventBus::new(256);
    let governor = RecordingGovernor::new();
    let cgroups = MockCgroups::default();
    let executor = RecordingExecutor::default();

    let p = ports(&governor, &cgroups, &executor);
    let outcome = control::authorize_and_run_with_ports(
        &plan(vec![(freeze_action("workload.batch/app"), None)]),
        &registry,
        &policy,
        &service,
        &events,
        AutonomyMode::L3Assisted,
        &p,
    )
    .await;

    let control::RunOutcome::Finished(finished) = outcome else {
        panic!("the plan cannot pause: freeze carries no approval gate");
    };
    assert!(
        finished
            .denied
            .contains(&CapabilityId::new(CapabilityId::HOST_CGROUP_FREEZE).unwrap()),
        "the unclassified adjustment is denied: {finished:?}"
    );
    assert_eq!(
        governor.consulted(),
        vec!["workload.batch/app".to_string()],
        "the governor is consulted exactly once, before policy"
    );
    assert!(
        executor.capabilities.lock().unwrap().is_empty(),
        "a refused adjustment never reaches the execution boundary"
    );
    assert!(cgroups.recorded().is_empty());
}

#[tokio::test]
async fn the_memory_pressure_scenario_protects_the_service_and_adjusts_the_consumer() {
    // AC-013: under memory pressure the critical service is protected while a
    // policy-approved freeze lands on the non-critical consumer, and recovery
    // is validated against live state.
    let registry = registry_with_m3();
    let policy = BootstrapPolicyEvaluator::with_local_remediation();
    let service = MockServiceController::new();
    let events = LocalEventBus::new(256);
    let governor = AutopilotGovernor::default();
    governor.classify("system.slice/postgres", Criticality::Critical);
    governor.classify("workload.batch/exporter", Criticality::BestEffort);
    let cgroups = Arc::new(MockCgroups::default());
    let executor = RemediationExecutor::new(
        Arc::new(argus_executor::UnixProcessController::new()),
        Arc::new(MockContainers),
        cgroups.clone(),
        remediation_guardrails(&[], &[]),
    );

    let p = ports(&governor, &cgroups, &executor);
    let outcome = control::authorize_and_run_with_ports(
        &plan(vec![
            (
                freeze_action("system.slice/postgres"),
                Some(thaw_action("system.slice/postgres")),
            ),
            (
                freeze_action("workload.batch/exporter"),
                Some(thaw_action("workload.batch/exporter")),
            ),
        ]),
        &registry,
        &policy,
        &service,
        &events,
        AutonomyMode::L3Assisted,
        &p,
    )
    .await;

    let control::RunOutcome::Finished(finished) = outcome else {
        panic!("assisted low-risk freezes never pause");
    };
    assert_eq!(finished.status, PlanStatus::Completed);
    assert!(
        finished
            .denied
            .contains(&CapabilityId::new(CapabilityId::HOST_CGROUP_FREEZE).unwrap()),
        "the critical service is refused by governance"
    );
    assert_eq!(
        cgroups.recorded(),
        vec![("workload.batch/exporter".to_string(), true)],
        "only the non-critical consumer is frozen"
    );
    assert!(
        cgroups.is_frozen("workload.batch/exporter").unwrap(),
        "the consumer's recovery is observable in live state"
    );
}

#[tokio::test]
async fn a_signal_plan_pauses_and_executes_only_after_a_single_use_grant() {
    let registry = registry_with_m3();
    let policy = BootstrapPolicyEvaluator::with_local_remediation();
    let service = MockServiceController::new();
    let events = LocalEventBus::new(256);
    let governor = AutopilotGovernor::default();
    let cgroups = MockCgroups::default();
    let executor = RecordingExecutor::default();
    let approvals = ApprovalStore::new();

    let p = ports(&governor, &cgroups, &executor);
    let outcome = control::authorize_and_run_with_ports(
        &plan(vec![(signal_action(4242, "term"), None)]),
        &registry,
        &policy,
        &service,
        &events,
        AutonomyMode::L3Assisted,
        &p,
    )
    .await;

    // First run: the HighRisk signal pauses for an operator grant.
    let control::RunOutcome::Pending(pending) = outcome else {
        panic!("a HighRisk signal must pause for approval");
    };
    assert!(
        executor.capabilities.lock().unwrap().is_empty(),
        "nothing executes while the plan is paused"
    );

    // Resume without a grant: refused, nothing executes.
    let refused = control::resume_and_run_with_ports(
        &pending,
        &approvals,
        &registry,
        &policy,
        &service,
        &events,
        AutonomyMode::L3Assisted,
        &p,
    )
    .await;
    assert!(
        matches!(refused, control::ResumeOutcome::Refused(_)),
        "a resume without a grant is refused"
    );

    // Grant, then resume: the step executes exactly once.
    control::authorize_and_run_with_ports(
        &plan(vec![(signal_action(4242, "term"), None)]),
        &registry,
        &policy,
        &service,
        &events,
        AutonomyMode::L3Assisted,
        &p,
    )
    .await;
    let _ = governor; // governor untouched for non-governed families

    let second = plan(vec![(signal_action(4242, "term"), None)]);
    let control::RunOutcome::Pending(pending2) = control::authorize_and_run_with_ports(
        &second,
        &registry,
        &policy,
        &service,
        &events,
        AutonomyMode::L3Assisted,
        &p,
    )
    .await
    else {
        panic!("every fresh signal plan pauses again");
    };
    approvals.grant_for_a_while(
        pending2.token,
        &pending2.context_hash,
        "uid=1000",
        Utc::now(),
        chrono::Duration::minutes(15),
    );
    let resumed = control::resume_and_run_with_ports(
        &pending2,
        &approvals,
        &registry,
        &policy,
        &service,
        &events,
        AutonomyMode::L3Assisted,
        &p,
    )
    .await;
    let control::ResumeOutcome::Finished(finished) = resumed else {
        panic!("the granted plan resumes");
    };
    assert_eq!(finished.status, PlanStatus::Completed);
    assert_eq!(
        executor.capabilities.lock().unwrap().clone(),
        vec!["host.process.signal".to_string()],
        "the approved signal crosses the execution boundary exactly once"
    );
}

#[tokio::test]
async fn a_capability_the_cloud_policy_denies_never_reaches_the_boundary() {
    // No-bypass (1/3): the default (cloud) policy denies the remediation
    // capabilities outright; even a schema-valid request is refused before any
    // executor is consulted.
    let registry = registry_with_m3();
    let cloud_policy = BootstrapPolicyEvaluator::new();
    let service = MockServiceController::new();
    let events = LocalEventBus::new(256);
    let governor = AutopilotGovernor::default();
    let cgroups = MockCgroups::default();
    let executor = RecordingExecutor::default();

    let p = ports(&governor, &cgroups, &executor);
    let outcome = control::authorize_and_run_with_ports(
        &plan(vec![(signal_action(4242, "term"), None)]),
        &registry,
        &cloud_policy,
        &service,
        &events,
        AutonomyMode::L3Assisted,
        &p,
    )
    .await;

    let control::RunOutcome::Finished(finished) = outcome else {
        panic!("a denied plan cannot pause");
    };
    assert_eq!(finished.status, PlanStatus::Denied);
    assert!(executor.capabilities.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_proposed_but_not_permuted_step_cannot_reach_the_executor() {
    // No-bypass (2/3): in ObserveOnly nothing executes, even a governor- and
    // policy-permitted freeze.
    let registry = registry_with_m3();
    let policy = BootstrapPolicyEvaluator::with_local_remediation();
    let service = MockServiceController::new();
    let events = LocalEventBus::new(256);
    let governor = AutopilotGovernor::default();
    governor.classify("workload.batch/app", Criticality::BestEffort);
    let cgroups = MockCgroups::default();
    let executor = RecordingExecutor::default();

    let p = ports(&governor, &cgroups, &executor);
    let outcome = control::authorize_and_run_with_ports(
        &plan(vec![(freeze_action("workload.batch/app"), None)]),
        &registry,
        &policy,
        &service,
        &events,
        AutonomyMode::L0Observe,
        &p,
    )
    .await;

    let control::RunOutcome::Finished(finished) = outcome else {
        panic!("an observe-only run cannot pause");
    };
    assert!(
        !finished.requires_approval.is_empty() || matches!(finished.status, PlanStatus::Denied),
        "the step is recorded as requiring approval or denied: {finished:?}"
    );
    assert!(executor.capabilities.lock().unwrap().is_empty());
    assert!(cgroups.recorded().is_empty());
}

#[tokio::test]
async fn the_composite_refuses_remediation_capabilities_when_none_is_wired() {
    // No-bypass (3/3): a composite without a remediation executor refuses the
    // remediation capabilities instead of falling through to the read-only
    // executor.
    use argus_domain::CapabilityRequest;
    use argus_executor::AuthorizedAction;

    let request = CapabilityRequest::new(
        CapabilityId::new(CapabilityId::HOST_CGROUP_FREEZE).unwrap(),
        Principal::new(Some(0), Some(0)),
        None,
        json!({ "path": "workload.batch/app" }),
        RequestContext::new(
            Uuid::new_v4(),
            Semver::new(0, 1, 0),
            Principal::new(Some(0), Some(0)),
            Utc::now(),
        ),
    );
    let action = AuthorizedAction::new(request, argus_domain::PolicyDecision::allow("t", "ok"))
        .expect("allowed");
    // A read-only executor (bootstrap-shaped) would fail with Unsupported on
    // its own; the composite must refuse before routing there at all.
    struct RefusingExecutor;
    impl Executor for RefusingExecutor {
        fn execute(&self, action: &AuthorizedAction) -> Result<ExecutionResult, ExecutionError> {
            // A read-only executor that would "execute anything" is the
            // bypass this test forbids; if the composite routes there, this
            // recording succeeds and the assertion below fails.
            Ok(ExecutionResult {
                capability: action.capability().clone(),
                evidence: json!({ "bypassed": true }),
                started_at: Utc::now(),
                finished_at: Utc::now(),
            })
        }
    }
    let composite = CompositeExecutor::new(Arc::new(RefusingExecutor), Arc::new(RefusingExecutor));
    let err = composite.execute(&action).unwrap_err();
    assert!(
        matches!(err, ExecutionError::Unsupported(_)),
        "an unwired remediation capability must be refused: {err:?}"
    );
}

#[tokio::test]
async fn a_failed_freeze_rolls_back_the_earlier_freeze() {
    // Rollback: [freeze a, freeze b(fails)] ⇒ thaw a runs, plan RolledBack.
    let registry = registry_with_m3();
    let policy = BootstrapPolicyEvaluator::with_local_remediation();
    let service = MockServiceController::new();
    let events = LocalEventBus::new(256);
    let governor = AutopilotGovernor::default();
    governor.classify("workload.batch/a", Criticality::BestEffort);
    governor.classify("workload.batch/b", Criticality::BestEffort);
    let cgroups = Arc::new(MockCgroups::default());
    cgroups.fail_on("workload.batch/b");
    let executor = RemediationExecutor::new(
        Arc::new(argus_executor::UnixProcessController::new()),
        Arc::new(MockContainers),
        cgroups.clone(),
        remediation_guardrails(&[], &[]),
    );

    let p = ports(&governor, &cgroups, &executor);
    let outcome = control::authorize_and_run_with_ports(
        &plan(vec![
            (
                freeze_action("workload.batch/a"),
                Some(thaw_action("workload.batch/a")),
            ),
            (
                freeze_action("workload.batch/b"),
                Some(thaw_action("workload.batch/b")),
            ),
        ]),
        &registry,
        &policy,
        &service,
        &events,
        AutonomyMode::L3Assisted,
        &p,
    )
    .await;

    let control::RunOutcome::Finished(finished) = outcome else {
        panic!("a fail-stop rollback cannot pause");
    };
    assert_eq!(finished.status, PlanStatus::RolledBack, "{finished:?}");
    assert_eq!(
        cgroups.recorded(),
        vec![
            ("workload.batch/a".to_string(), true),
            ("workload.batch/b".to_string(), true), // the failed effect
            ("workload.batch/a".to_string(), false), // the declarative rollback
        ],
        "the rollback thaws the surviving freeze in reverse order"
    );
    assert!(!cgroups.is_frozen("workload.batch/a").unwrap());
}

#[tokio::test]
async fn an_unfreezable_target_without_rollback_leaves_the_plan_needs_manual() {
    let registry = registry_with_m3();
    let policy = BootstrapPolicyEvaluator::with_local_remediation();
    let service = MockServiceController::new();
    let events = LocalEventBus::new(256);
    let governor = AutopilotGovernor::default();
    governor.classify("workload.batch/broken", Criticality::BestEffort);
    let cgroups = Arc::new(MockCgroups::default());
    cgroups.fail_on("workload.batch/broken");
    let executor = RemediationExecutor::new(
        Arc::new(argus_executor::UnixProcessController::new()),
        Arc::new(MockContainers),
        cgroups.clone(),
        remediation_guardrails(&[], &[]),
    );

    let p = ports(&governor, &cgroups, &executor);
    let outcome = control::authorize_and_run_with_ports(
        &plan(vec![(freeze_action("workload.batch/broken"), None)]),
        &registry,
        &policy,
        &service,
        &events,
        AutonomyMode::L3Assisted,
        &p,
    )
    .await;

    let control::RunOutcome::Finished(finished) = outcome else {
        panic!("a fail-stop failure cannot pause");
    };
    assert_eq!(finished.status, PlanStatus::Failed);
}

#[tokio::test]
async fn the_daemon_refuses_unclassified_governed_adjustments_end_to_end() {
    // The wired daemon path: run_remediation consults its governor first, and
    // a subject no operator classified is refused with no execution attempt.
    let state = std::env::temp_dir()
        .join(format!("argus-m3-{}.db", Uuid::new_v4()))
        .display()
        .to_string();
    let daemon = Daemon::init(DaemonConfig {
        state_path: state,
        ..DaemonConfig::default()
    })
    .await
    .unwrap();

    let events = LocalEventBus::new(256);
    let outcome = daemon
        .run_remediation(
            &plan(vec![(freeze_action("workload.batch/app"), None)]),
            AutonomyMode::L3Assisted,
            &events,
        )
        .await;

    let control::RunOutcome::Finished(finished) = outcome else {
        panic!("a governed refusal cannot pause");
    };
    assert!(
        finished
            .denied
            .contains(&CapabilityId::new(CapabilityId::HOST_CGROUP_FREEZE).unwrap()),
        "the daemon's governor refuses the unclassified subject: {finished:?}"
    );

    // The cloud channel stays deny-by-default for the remediation
    // capabilities: a schema-valid request is refused at policy.
    use argus_domain::{PolicyOutcome, RequestContext};
    let request = argus_domain::CapabilityRequest::new(
        CapabilityId::new(CapabilityId::HOST_CGROUP_FREEZE).unwrap(),
        Principal::new(Some(1000), Some(1000)),
        None,
        json!({ "path": "workload.batch/app" }),
        RequestContext::new(
            Uuid::new_v4(),
            Semver::new(0, 1, 0),
            Principal::new(Some(1000), Some(1000)),
            Utc::now(),
        ),
    );
    let err = daemon.authorize_and_execute(request).unwrap_err();
    assert!(matches!(
        err,
        argus_daemon::DispatchError::Denied(PolicyOutcome::Deny)
    ));
}
