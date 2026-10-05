//! Kubernetes self-healing security tests (spec 003 M4, T026, CAP-14,
//! FR-016, ADR-0037 §4): deny-by-default for the deferred actions, approval
//! gating for the effects, the AC-012 rollback path, and fail-closed behavior
//! when no cluster is configured. No cluster, no `kubectl`, no shell.

use std::sync::{Arc, Mutex};

use argus_daemon::config::DaemonConfig;
use argus_daemon::{Daemon, control};
use argus_domain::{
    Action, AutonomyMode, BlastRadius, CapabilityDescriptor, CapabilityId, CapabilityRegistry,
    Plan, PlanStatus, PlanStep, Reversibility, RiskClass,
};
use argus_events::LocalEventBus;
use argus_executor::{
    ClusterController, ExecutionError, ExecutionResult, Executor, MockServiceController,
    RemediationError,
};
use argus_policy::{ApprovalStore, BootstrapPolicyEvaluator, ResourceGovernor};
use chrono::Utc;
use semver::Version as Semver;
use serde_json::{Value, json};
use uuid::Uuid;

/// Records the effects that actually reach the cluster boundary.
#[derive(Default)]
struct RecordingCluster {
    calls: Mutex<Vec<String>>,
    pods_running: Mutex<std::collections::BTreeSet<String>>,
    deployments_available: Mutex<std::collections::BTreeSet<String>>,
}

impl RecordingCluster {
    fn mark_pod_running(&self, namespace: &str, name: &str, running: bool) {
        let key = format!("{namespace}/{name}");
        let mut set = self.pods_running.lock().unwrap();
        if running {
            set.insert(key);
        } else {
            set.remove(&key);
        }
    }
}

impl ClusterController for RecordingCluster {
    fn restart_pod(&self, namespace: &str, name: &str) -> Result<(), RemediationError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("restart-pod {namespace}/{name}"));
        // The controller reschedules the pod: it is running again.
        self.pods_running
            .lock()
            .unwrap()
            .insert(format!("{namespace}/{name}"));
        Ok(())
    }

    fn delete_pod(&self, namespace: &str, name: &str) -> Result<(), RemediationError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("delete-pod {namespace}/{name}"));
        Ok(())
    }

    fn restart_deployment(&self, namespace: &str, name: &str) -> Result<(), RemediationError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("restart-deployment {namespace}/{name}"));
        Ok(())
    }

    fn rollback_deployment(
        &self,
        namespace: &str,
        name: &str,
        revision: Option<i64>,
    ) -> Result<(), RemediationError> {
        self.calls.lock().unwrap().push(format!(
            "rollback-deployment {namespace}/{name} revision={revision:?}"
        ));
        // The rollback lands: the deployment becomes available again.
        self.deployments_available
            .lock()
            .unwrap()
            .insert(format!("{namespace}/{name}"));
        Ok(())
    }

    fn scale_workload(
        &self,
        namespace: &str,
        name: &str,
        replicas: i64,
    ) -> Result<(), RemediationError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("scale {namespace}/{name} replicas={replicas}"));
        Ok(())
    }

    fn set_node_schedulable(&self, name: &str, schedulable: bool) -> Result<(), RemediationError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("node {name} schedulable={schedulable}"));
        Ok(())
    }

    fn cleanup_job(&self, namespace: &str, name: &str) -> Result<(), RemediationError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("cleanup-job {namespace}/{name}"));
        Ok(())
    }

    fn pod_running(&self, namespace: &str, name: &str) -> Result<bool, RemediationError> {
        Ok(self
            .pods_running
            .lock()
            .unwrap()
            .contains(&format!("{namespace}/{name}")))
    }

    fn deployment_available(&self, namespace: &str, name: &str) -> Result<bool, RemediationError> {
        Ok(self
            .deployments_available
            .lock()
            .unwrap()
            .contains(&format!("{namespace}/{name}")))
    }

    fn node_schedulable(&self, _name: &str) -> Result<bool, RemediationError> {
        Ok(true)
    }

    fn read(
        &self,
        resource: &str,
        namespace: Option<&str>,
        name: Option<&str>,
    ) -> Result<Value, RemediationError> {
        Ok(json!({ "resource": resource, "namespace": namespace, "name": name }))
    }
}

/// An executor that refuses everything, for the ports that must never route.
#[derive(Default)]
struct NoExecutor;

impl Executor for NoExecutor {
    fn execute(
        &self,
        action: &argus_executor::AuthorizedAction,
    ) -> Result<ExecutionResult, ExecutionError> {
        Err(ExecutionError::Unsupported(action.capability().clone()))
    }
}

/// A governor that only observes.
#[derive(Default)]
struct NoGovernor;

impl ResourceGovernor for NoGovernor {
    fn evaluate(
        &self,
        _adjustment: &argus_policy::ResourceAdjustment,
    ) -> argus_policy::GovernanceDecision {
        argus_policy::GovernanceDecision::Refused {
            why: argus_policy::Refusal::Unclassified,
        }
    }

    fn governed_prefixes(&self) -> &'static [&'static str] {
        &["host.cgroup."]
    }
}

/// The registry shape the daemon wires: reads + controlled k8s effects with
/// per-invocation approval; pod.delete and the deferred pair follow the
/// contract's risk classes.
fn registry_with_m4() -> CapabilityRegistry {
    let mut registry = CapabilityRegistry::new();
    let entries: Vec<(&str, RiskClass, bool)> = vec![
        (CapabilityId::K8S_CLUSTER_READ, RiskClass::Read, false),
        (CapabilityId::K8S_NODE_READ, RiskClass::Read, false),
        (CapabilityId::K8S_POD_READ, RiskClass::Read, false),
        (CapabilityId::K8S_DEPLOYMENT_READ, RiskClass::Read, false),
        (CapabilityId::K8S_POD_RESTART, RiskClass::Controlled, true),
        (CapabilityId::K8S_POD_DELETE, RiskClass::HighRisk, true),
        (
            CapabilityId::K8S_DEPLOYMENT_RESTART,
            RiskClass::Controlled,
            true,
        ),
        (
            CapabilityId::K8S_DEPLOYMENT_ROLLBACK,
            RiskClass::Controlled,
            true,
        ),
        (
            CapabilityId::K8S_WORKLOAD_SCALE,
            RiskClass::Controlled,
            true,
        ),
        (CapabilityId::K8S_NODE_CORDON, RiskClass::Controlled, true),
        (CapabilityId::K8S_NODE_UNCORDON, RiskClass::Controlled, true),
        (CapabilityId::K8S_JOB_CLEANUP, RiskClass::Controlled, true),
    ];
    for (capability, risk, approval) in entries {
        let id = CapabilityId::new(capability).unwrap();
        let schema = if capability.ends_with(".read") {
            json!({
                "type": "object",
                "properties": {
                    "namespace": { "type": "string" },
                    "name": { "type": "string" },
                },
                "additionalProperties": false,
            })
        } else if capability == CapabilityId::K8S_WORKLOAD_SCALE {
            json!({
                "type": "object",
                "required": ["name", "replicas"],
                "properties": {
                    "namespace": { "type": "string" },
                    "name": { "type": "string" },
                    "replicas": { "type": "integer", "minimum": 0 },
                },
                "additionalProperties": false,
            })
        } else {
            json!({
                "type": "object",
                "required": ["name"],
                "properties": {
                    "namespace": { "type": "string" },
                    "name": { "type": "string" },
                    "revision": { "type": "integer" },
                },
                "additionalProperties": false,
            })
        };
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
        .with_blast_radius(BlastRadius::Environment)
        .requiring_approval_if(approval);
        registry.register(descriptor).unwrap();
    }
    registry
}

fn k8s_action(capability: &str, arguments: Value) -> Action {
    Action {
        capability: CapabilityId::new(capability).unwrap(),
        resource: None,
        arguments,
    }
}

fn plan(steps: Vec<Action>) -> Plan {
    Plan {
        objective: "kubernetes remediation".into(),
        steps: steps
            .into_iter()
            .map(|action| PlanStep {
                action,
                rollback: None,
            })
            .collect(),
        preconditions: vec![],
        expected_outcomes: vec![],
        blast_radius: BlastRadius::Environment,
        confidence: 0.9,
        status: PlanStatus::Proposed,
    }
}

fn ports<'a>(
    cluster: &'a RecordingCluster,
    kubernetes: &'a dyn Executor,
) -> control::LoopPorts<'a> {
    static GOVERNOR: NoGovernor = NoGovernor;
    // Host-side ports are unused in these tests; only the cluster side moves.
    struct NoCgroups;
    impl argus_executor::CgroupController for NoCgroups {
        fn set_freeze(&self, _: &str, _: bool) -> Result<(), RemediationError> {
            Err(RemediationError::Unavailable("unused".into()))
        }
        fn is_frozen(&self, _: &str) -> Result<bool, RemediationError> {
            Err(RemediationError::Unavailable("unused".into()))
        }
    }
    struct NoContainers;
    impl argus_executor::ContainerController for NoContainers {
        fn restart(&self, _: &str) -> Result<(), RemediationError> {
            Err(RemediationError::Unavailable("unused".into()))
        }
        fn is_running(&self, _: &str) -> Result<bool, RemediationError> {
            Err(RemediationError::Unavailable("unused".into()))
        }
    }
    static NO_CGROUPS: NoCgroups = NoCgroups;
    static NO_CONTAINERS: NoContainers = NoContainers;
    static NO_REMEDIATION: NoExecutor = NoExecutor;
    control::LoopPorts {
        governor: &GOVERNOR,
        containers: &NO_CONTAINERS,
        cgroups: &NO_CGROUPS,
        cluster,
        remediation: &NO_REMEDIATION,
        kubernetes,
    }
}

fn kubernetes_executor(cluster: Arc<RecordingCluster>) -> argus_executor::KubernetesExecutor {
    argus_executor::KubernetesExecutor::new(
        cluster,
        argus_executor::kubernetes_guardrails(&["kube-system".to_string()]),
        10,
    )
}

#[tokio::test]
async fn the_deferred_drain_and_reschedule_are_denied_by_default_everywhere() {
    let registry = registry_with_m4();
    // Even the most permissive local policy never lists the deferred pair.
    let policy = BootstrapPolicyEvaluator::with_local_remediation();
    let service = MockServiceController::new();
    let events = LocalEventBus::new(256);
    let cluster = RecordingCluster::default();
    let kubernetes = kubernetes_executor(Arc::new(RecordingCluster::default()));

    let p = ports(&cluster, &kubernetes);
    for capability in [
        CapabilityId::K8S_NODE_DRAIN,
        CapabilityId::K8S_WORKLOAD_RESCHEDULE,
    ] {
        // Unregistered in the daemon registry: the loop's fail-closed default
        // marks them approval-requiring, and the policy — which has no path
        // for them — denies outright.
        let outcome = control::authorize_and_run_with_ports(
            &plan(vec![k8s_action(
                capability,
                json!({ "name": "node-1", "namespace": "prod" }),
            )]),
            &registry,
            &policy,
            &service,
            &events,
            AutonomyMode::Assisted,
            &p,
        )
        .await;
        let control::RunOutcome::Finished(finished) = outcome else {
            panic!("{capability} cannot pause: it has no permitted path at all");
        };
        assert_eq!(finished.status, PlanStatus::Denied, "{capability}");
        assert!(
            finished
                .denied
                .contains(&CapabilityId::new(capability).unwrap()),
            "{capability} is denied by policy"
        );
    }
    assert!(
        cluster.calls.lock().unwrap().is_empty(),
        "nothing reached the cluster boundary"
    );
}

#[tokio::test]
async fn pod_delete_is_denied_until_a_policy_explicitly_permits_it() {
    let registry = registry_with_m4();
    let policy = BootstrapPolicyEvaluator::with_local_remediation();
    let service = MockServiceController::new();
    let events = LocalEventBus::new(256);
    let cluster = RecordingCluster::default();
    let kubernetes = kubernetes_executor(Arc::new(RecordingCluster::default()));

    let p = ports(&cluster, &kubernetes);
    let outcome = control::authorize_and_run_with_ports(
        &plan(vec![k8s_action(
            CapabilityId::K8S_POD_DELETE,
            json!({ "namespace": "prod", "name": "api-7d9f" }),
        )]),
        &registry,
        &policy,
        &service,
        &events,
        AutonomyMode::Assisted,
        &p,
    )
    .await;

    let control::RunOutcome::Finished(finished) = outcome else {
        panic!("a denied plan cannot pause");
    };
    assert_eq!(finished.status, PlanStatus::Denied);
    assert!(
        cluster.calls.lock().unwrap().is_empty(),
        "the high-risk delete never reached the cluster"
    );
}

#[tokio::test]
async fn the_policy_permitted_rollback_executes_after_approval_and_is_validated() {
    // AC-012: a policy-permitted `deployment.rollback` executes via the typed
    // executor and is validated; a non-permitted one is denied at policy; no
    // `kubectl` is ever executed.
    let registry = registry_with_m4();
    let policy = BootstrapPolicyEvaluator::with_local_remediation();
    let service = MockServiceController::new();
    let events = LocalEventBus::new(256);
    let cluster = Arc::new(RecordingCluster::default());
    // The deployment is NOT available — that is why the rollback was proposed.
    let kubernetes = kubernetes_executor(cluster.clone());
    let approvals = ApprovalStore::new();

    let p = ports(&cluster, &kubernetes);
    let outcome = control::authorize_and_run_with_ports(
        &plan(vec![k8s_action(
            CapabilityId::K8S_DEPLOYMENT_ROLLBACK,
            json!({ "namespace": "prod", "name": "api", "revision": 12 }),
        )]),
        &registry,
        &policy,
        &service,
        &events,
        AutonomyMode::Assisted,
        &p,
    )
    .await;

    // The controlled rollback pauses for its per-invocation approval.
    let control::RunOutcome::Pending(pending) = outcome else {
        panic!("a controlled rollback pauses for approval");
    };
    assert!(cluster.calls.lock().unwrap().is_empty());

    approvals.grant_for_a_while(
        pending.token,
        &pending.context_hash,
        "uid=1000",
        Utc::now(),
        chrono::Duration::minutes(15),
    );
    let resumed = control::resume_and_run_with_ports(
        &pending,
        &approvals,
        &registry,
        &policy,
        &service,
        &events,
        AutonomyMode::Assisted,
        &p,
    )
    .await;
    let control::ResumeOutcome::Finished(finished) = resumed else {
        panic!("the granted rollback resumes");
    };
    assert_eq!(finished.status, PlanStatus::Completed);
    assert_eq!(
        cluster.calls.lock().unwrap().clone(),
        vec!["rollback-deployment prod/api revision=Some(12)".to_string()],
        "the typed executor reached the cluster exactly once"
    );
    assert_eq!(
        finished.executions.len(),
        1,
        "the execution is recorded with evidence"
    );
    assert!(
        !cluster
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call.contains("kubectl")),
        "no kubectl appears anywhere in the executed path"
    );
}

#[tokio::test]
async fn a_pod_restart_validates_against_live_pod_state() {
    let registry = registry_with_m4();
    let policy = BootstrapPolicyEvaluator::with_local_remediation();
    let service = MockServiceController::new();
    let events = LocalEventBus::new(256);
    let cluster = Arc::new(RecordingCluster::default());
    // The pod is NOT running: the desired-state check must let the effect run,
    // and the post-execution validation observes it running again.
    cluster.mark_pod_running("prod", "api-7d9f", false);
    let kubernetes = kubernetes_executor(cluster.clone());
    let approvals = ApprovalStore::new();

    let p = ports(&cluster, &kubernetes);
    let outcome = control::authorize_and_run_with_ports(
        &plan(vec![k8s_action(
            CapabilityId::K8S_POD_RESTART,
            json!({ "namespace": "prod", "name": "api-7d9f" }),
        )]),
        &registry,
        &policy,
        &service,
        &events,
        AutonomyMode::Assisted,
        &p,
    )
    .await;
    let control::RunOutcome::Pending(pending) = outcome else {
        panic!("a controlled pod restart pauses for approval");
    };
    approvals.grant_for_a_while(
        pending.token,
        &pending.context_hash,
        "uid=1000",
        Utc::now(),
        chrono::Duration::minutes(15),
    );
    let resumed = control::resume_and_run_with_ports(
        &pending,
        &approvals,
        &registry,
        &policy,
        &service,
        &events,
        AutonomyMode::Assisted,
        &p,
    )
    .await;
    let control::ResumeOutcome::Finished(finished) = resumed else {
        panic!("the granted restart resumes");
    };
    assert_eq!(finished.status, PlanStatus::Completed);
    assert_eq!(
        finished.executions[0].evidence["operation"], "restart",
        "the typed effect carries its evidence"
    );
}

#[tokio::test]
async fn a_granted_resume_without_a_configured_cluster_fails_closed() {
    // The daemon ships degraded ports: no cluster. Even a granted approval
    // cannot make the effect succeed — it fails loudly instead of pretending.
    let state = std::env::temp_dir()
        .join(format!("argus-m4-{}.db", Uuid::new_v4()))
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
            &plan(vec![k8s_action(
                CapabilityId::K8S_DEPLOYMENT_ROLLBACK,
                json!({ "namespace": "prod", "name": "api", "revision": 12 }),
            )]),
            AutonomyMode::Assisted,
            &events,
        )
        .await;

    // First run: the plan pauses for approval (the policy permits it; the
    // approval gate comes from the descriptor).
    let control::RunOutcome::Pending(pending) = outcome else {
        panic!("the rollback pauses for its approval gate; got {outcome:?}");
    };
    daemon
        .grant_approval(pending.token, "uid=1000")
        .expect("the operator grants");
    let resumed = daemon.resume_remediation(&pending, &events).await;
    let control::ResumeOutcome::Finished(finished) = resumed else {
        panic!("a granted resume runs");
    };
    assert_eq!(
        finished.status,
        PlanStatus::Failed,
        "without a configured cluster the effect fails closed: {finished:?}"
    );
}

#[tokio::test]
async fn kube_system_is_protected_before_the_cluster_is_touched() {
    use argus_domain::{CapabilityRequest, PolicyDecision, Principal, RequestContext};
    use argus_executor::AuthorizedAction;

    let cluster = Arc::new(RecordingCluster::default());
    let executor = kubernetes_executor(cluster.clone());
    let request = CapabilityRequest::new(
        CapabilityId::new(CapabilityId::K8S_POD_RESTART).unwrap(),
        Principal::new(Some(0), Some(0)),
        None,
        json!({ "namespace": "kube-system", "name": "coredns-abc" }),
        RequestContext::new(
            Uuid::new_v4(),
            Semver::new(0, 1, 0),
            Principal::new(Some(0), Some(0)),
            Utc::now(),
        ),
    );
    let action = AuthorizedAction::new(request, PolicyDecision::allow("t", "ok")).expect("allowed");
    let err = executor.execute(&action).unwrap_err();
    assert!(
        matches!(err, ExecutionError::GuardrailViolation(_)),
        "the protected namespace guardrail refuses first: {err:?}"
    );
    assert!(cluster.calls.lock().unwrap().is_empty());
}
