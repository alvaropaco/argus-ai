//! Kubernetes self-healing capabilities (spec 003 M4, CAP-14, FR-016,
//! ADR-0037 §4).
//!
//! The [`ClusterController`] port carries every cluster effect as a typed
//! method — there is no command string anywhere in this path, and nothing an
//! LLM produces can become a Kubernetes API call directly. Effects cross the
//! same [`AuthorizedAction`] boundary as the host executors, with guardrails
//! re-checking the namespace/name at execution time.
//!
//! `k8s.node.drain` and `k8s.workload.reschedule` are deliberately absent:
//! they are deny-by-default and deferred (ADR-0037 §4), so [`KubernetesExecutor`]
//! reports them unsupported and no policy permits them.

use std::sync::Arc;

use argus_domain::CapabilityId;
use chrono::Utc;
use serde_json::{Value, json};

use crate::action::{AuthorizedAction, ExecutionError, ExecutionResult};
use crate::executor::Executor;
use crate::guardrail::GuardrailRegistry;
use crate::remediation::RemediationError;

/// The cluster control-plane port behind the `k8s.*` capabilities.
///
/// Implementations talk to the Kubernetes API (or a deterministic mock in
/// tests). `delete_pod` must refuse a pod that has no controller owner
/// reference — deleting a bare pod is unrecoverable, and that check needs the
/// live object, so it lives in the implementation, not the guardrails.
pub trait ClusterController: Send + Sync {
    // Effects (self-healing, each typed, never a command string).
    fn restart_pod(&self, namespace: &str, name: &str) -> Result<(), RemediationError>;
    fn delete_pod(&self, namespace: &str, name: &str) -> Result<(), RemediationError>;
    fn restart_deployment(&self, namespace: &str, name: &str) -> Result<(), RemediationError>;
    fn rollback_deployment(
        &self,
        namespace: &str,
        name: &str,
        revision: Option<i64>,
    ) -> Result<(), RemediationError>;
    fn scale_workload(
        &self,
        namespace: &str,
        name: &str,
        replicas: i64,
    ) -> Result<(), RemediationError>;
    /// `schedulable = false` is a cordon; `true` an uncordon.
    fn set_node_schedulable(&self, name: &str, schedulable: bool) -> Result<(), RemediationError>;
    fn cleanup_job(&self, namespace: &str, name: &str) -> Result<(), RemediationError>;

    // Reads (desired-state checks, validation, and the k8s.*.read surface).
    fn pod_running(&self, namespace: &str, name: &str) -> Result<bool, RemediationError>;
    fn deployment_available(&self, namespace: &str, name: &str) -> Result<bool, RemediationError>;
    fn node_schedulable(&self, name: &str) -> Result<bool, RemediationError>;
    /// A cluster read for the `k8s.*.read` capabilities: `resource` is one of
    /// `cluster`, `nodes`, `pods`, `deployments`.
    fn read(
        &self,
        resource: &str,
        namespace: Option<&str>,
        name: Option<&str>,
    ) -> Result<Value, RemediationError>;
}

/// A cluster controller that is not wired to any cluster: every operation
/// reports a clear degradation (ADR-0037 §2), which is what a single-host
/// daemon ships by default.
#[derive(Debug, Default)]
pub struct UnavailableClusterController;

impl ClusterController for UnavailableClusterController {
    fn restart_pod(&self, _: &str, _: &str) -> Result<(), RemediationError> {
        Err(Self::unavailable())
    }
    fn delete_pod(&self, _: &str, _: &str) -> Result<(), RemediationError> {
        Err(Self::unavailable())
    }
    fn restart_deployment(&self, _: &str, _: &str) -> Result<(), RemediationError> {
        Err(Self::unavailable())
    }
    fn rollback_deployment(
        &self,
        _: &str,
        _: &str,
        _: Option<i64>,
    ) -> Result<(), RemediationError> {
        Err(Self::unavailable())
    }
    fn scale_workload(&self, _: &str, _: &str, _: i64) -> Result<(), RemediationError> {
        Err(Self::unavailable())
    }
    fn set_node_schedulable(&self, _: &str, _: bool) -> Result<(), RemediationError> {
        Err(Self::unavailable())
    }
    fn cleanup_job(&self, _: &str, _: &str) -> Result<(), RemediationError> {
        Err(Self::unavailable())
    }
    fn pod_running(&self, _: &str, _: &str) -> Result<bool, RemediationError> {
        Err(Self::unavailable())
    }
    fn deployment_available(&self, _: &str, _: &str) -> Result<bool, RemediationError> {
        Err(Self::unavailable())
    }
    fn node_schedulable(&self, _: &str) -> Result<bool, RemediationError> {
        Err(Self::unavailable())
    }
    fn read(&self, _: &str, _: Option<&str>, _: Option<&str>) -> Result<Value, RemediationError> {
        Err(Self::unavailable())
    }
}

impl UnavailableClusterController {
    fn unavailable() -> RemediationError {
        RemediationError::Unavailable("no kubernetes cluster is configured".to_string())
    }
}

/// Executes the typed Kubernetes self-healing capabilities through a
/// [`ClusterController`].
pub struct KubernetesExecutor {
    cluster: Arc<dyn ClusterController>,
    guardrails: GuardrailRegistry,
    /// The `k8s.workload.scale` quota ceiling; a guardrail predicate cannot
    /// see numeric arguments, so the bound is checked here, still at the
    /// execution boundary before the effect.
    max_replicas: i64,
}

impl KubernetesExecutor {
    pub fn new(
        cluster: Arc<dyn ClusterController>,
        guardrails: GuardrailRegistry,
        max_replicas: i64,
    ) -> Self {
        Self {
            cluster,
            guardrails,
            max_replicas,
        }
    }

    /// Whether this executor performs the named capability.
    ///
    /// The deferred, deny-by-default actions (`k8s.node.drain`,
    /// `k8s.workload.reschedule`) are *not* handled here and never will be
    /// from this executor: they have no typed effect to route to.
    pub fn handles(capability: &CapabilityId) -> bool {
        matches!(
            capability.as_str(),
            CapabilityId::K8S_POD_RESTART
                | CapabilityId::K8S_POD_DELETE
                | CapabilityId::K8S_DEPLOYMENT_RESTART
                | CapabilityId::K8S_DEPLOYMENT_ROLLBACK
                | CapabilityId::K8S_WORKLOAD_SCALE
                | CapabilityId::K8S_NODE_CORDON
                | CapabilityId::K8S_NODE_UNCORDON
                | CapabilityId::K8S_JOB_CLEANUP
                | CapabilityId::K8S_CLUSTER_READ
                | CapabilityId::K8S_NODE_READ
                | CapabilityId::K8S_POD_READ
                | CapabilityId::K8S_DEPLOYMENT_READ
        )
    }

    fn guard(&self, capability: &CapabilityId, target: &str) -> Result<(), ExecutionError> {
        self.guardrails
            .check(capability, target)
            .map_err(ExecutionError::GuardrailViolation)
    }

    fn namespace_name(
        &self,
        action: &AuthorizedAction,
    ) -> Result<(String, String), ExecutionError> {
        let arguments = &action.request().arguments;
        let namespace = arguments
            .get("namespace")
            .and_then(Value::as_str)
            .unwrap_or("default")
            .to_string();
        let name = arguments
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| ExecutionError::Failed("the request does not name a 'name'".into()))?
            .to_string();
        Ok((namespace, name))
    }

    fn replicas(&self, action: &AuthorizedAction) -> Result<i64, ExecutionError> {
        action
            .request()
            .arguments
            .get("replicas")
            .and_then(Value::as_i64)
            .ok_or_else(|| {
                ExecutionError::Failed("the request does not name numeric 'replicas'".into())
            })
    }

    fn revision(&self, action: &AuthorizedAction) -> Option<i64> {
        action
            .request()
            .arguments
            .get("revision")
            .and_then(Value::as_i64)
    }
}

impl Executor for KubernetesExecutor {
    fn execute(&self, action: &AuthorizedAction) -> Result<ExecutionResult, ExecutionError> {
        let capability = action.capability();
        let started_at = Utc::now();
        let arguments = &action.request().arguments;
        let (namespace, name) = self.namespace_name(action)?;

        let effect = |result: Result<(), RemediationError>,
                      evidence: Value|
         -> Result<ExecutionResult, ExecutionError> {
            result
                .map(|_| ExecutionResult {
                    capability: capability.clone(),
                    evidence,
                    started_at,
                    finished_at: Utc::now(),
                })
                .map_err(|e| ExecutionError::Failed(e.to_string()))
        };

        match capability.as_str() {
            CapabilityId::K8S_POD_RESTART => {
                self.guard(capability, &format!("{namespace}/{name}"))?;
                effect(
                    self.cluster.restart_pod(&namespace, &name),
                    json!({ "namespace": namespace, "name": name, "operation": "restart" }),
                )
            }
            CapabilityId::K8S_POD_DELETE => {
                self.guard(capability, &format!("{namespace}/{name}"))?;
                effect(
                    self.cluster.delete_pod(&namespace, &name),
                    json!({ "namespace": namespace, "name": name, "operation": "delete" }),
                )
            }
            CapabilityId::K8S_DEPLOYMENT_RESTART => {
                self.guard(capability, &format!("{namespace}/{name}"))?;
                effect(
                    self.cluster.restart_deployment(&namespace, &name),
                    json!({ "namespace": namespace, "name": name, "operation": "rollout-restart" }),
                )
            }
            CapabilityId::K8S_DEPLOYMENT_ROLLBACK => {
                self.guard(capability, &format!("{namespace}/{name}"))?;
                let revision = self.revision(action);
                effect(
                    self.cluster
                        .rollback_deployment(&namespace, &name, revision),
                    json!({
                        "namespace": namespace, "name": name,
                        "operation": "rollback",
                        "revision": revision,
                    }),
                )
            }
            CapabilityId::K8S_WORKLOAD_SCALE => {
                self.guard(capability, &format!("{namespace}/{name}"))?;
                let replicas = self.replicas(action)?;
                MaxReplicas::new(self.max_replicas)
                    .check_replicas(replicas)
                    .map_err(ExecutionError::Failed)?;
                effect(
                    self.cluster.scale_workload(&namespace, &name, replicas),
                    json!({ "namespace": namespace, "name": name, "replicas": replicas }),
                )
            }
            CapabilityId::K8S_NODE_CORDON | CapabilityId::K8S_NODE_UNCORDON => {
                self.guard(capability, &name)?;
                let schedulable = capability.as_str() == CapabilityId::K8S_NODE_UNCORDON;
                effect(
                    self.cluster.set_node_schedulable(&name, schedulable),
                    json!({ "name": name, "schedulable": schedulable }),
                )
            }
            CapabilityId::K8S_JOB_CLEANUP => {
                self.guard(capability, &format!("{namespace}/{name}"))?;
                effect(
                    self.cluster.cleanup_job(&namespace, &name),
                    json!({ "namespace": namespace, "name": name, "operation": "cleanup" }),
                )
            }
            CapabilityId::K8S_CLUSTER_READ
            | CapabilityId::K8S_NODE_READ
            | CapabilityId::K8S_POD_READ
            | CapabilityId::K8S_DEPLOYMENT_READ => {
                let resource = read_resource(capability);
                let read_name = arguments.get("name").and_then(Value::as_str);
                self.guard(capability, &format!("{namespace}/{name}"))?;
                let payload = self
                    .cluster
                    .read(resource, Some(&namespace), read_name)
                    .map_err(|e| ExecutionError::Failed(e.to_string()))?;
                Ok(ExecutionResult {
                    capability: capability.clone(),
                    evidence: json!({ "resource": resource, "result": payload }),
                    started_at,
                    finished_at: Utc::now(),
                })
            }
            _ => Err(ExecutionError::Unsupported(capability.clone())),
        }
    }
}

/// The closed set of read resources, one per `k8s.*.read` capability.
fn read_resource(capability: &CapabilityId) -> &'static str {
    match capability.as_str() {
        CapabilityId::K8S_CLUSTER_READ => "cluster",
        CapabilityId::K8S_NODE_READ => "nodes",
        CapabilityId::K8S_POD_READ => "pods",
        CapabilityId::K8S_DEPLOYMENT_READ => "deployments",
        _ => "unknown",
    }
}

/// Guardrails wired for the Kubernetes capabilities.
///
/// - Every effect: namespace and object names are validated as DNS-1123
///   subdomain-ish identifiers at execution time.
/// - Pod effects (`restart`, `delete`): the namespace must not be protected.
///   `kube-system` is protected by default; the operator may add more.
/// - `k8s.workload.scale`: the replica count is bounded by the executor's
///   quota ceiling (`MaxReplicas`), which cannot be a target guardrail.
pub fn kubernetes_guardrails(protected_namespaces: &[String]) -> GuardrailRegistry {
    let mut registry = GuardrailRegistry::new();

    let pod_effects = [CapabilityId::K8S_POD_RESTART, CapabilityId::K8S_POD_DELETE];
    for capability in pod_effects {
        let id = CapabilityId::new(capability).expect("k8s capability ids are valid");
        registry.register(
            id,
            Arc::new(ProtectedNamespaces::new(protected_namespaces.to_vec())),
        );
    }

    for capability in [
        CapabilityId::K8S_POD_RESTART,
        CapabilityId::K8S_POD_DELETE,
        CapabilityId::K8S_DEPLOYMENT_RESTART,
        CapabilityId::K8S_DEPLOYMENT_ROLLBACK,
        CapabilityId::K8S_WORKLOAD_SCALE,
        CapabilityId::K8S_JOB_CLEANUP,
    ] {
        let id = CapabilityId::new(capability).expect("k8s capability ids are valid");
        registry.register(id, Arc::new(ValidK8sName));
    }
    registry
}

/// Refuses a `namespace/name` target whose namespace is on the protected list.
#[derive(Debug)]
pub struct ProtectedNamespaces {
    protected: std::collections::BTreeSet<String>,
}

impl ProtectedNamespaces {
    pub fn new(protected: Vec<String>) -> Self {
        Self {
            protected: protected.into_iter().collect(),
        }
    }
}

impl crate::guardrail::Guardrail for ProtectedNamespaces {
    fn check(&self, target: &str) -> Result<(), String> {
        let namespace = target.split('/').next().unwrap_or_default();
        if self.protected.contains(namespace) {
            return Err(format!(
                "namespace '{namespace}' is protected from kubernetes remediation"
            ));
        }
        Ok(())
    }
}

/// Bounds `k8s.workload.scale`: the replica count must sit in `0..=max`.
///
/// The guardrail registry carries string targets, so this bound is checked
/// directly by the executor at the same boundary — the constructor wires the
/// configured ceiling in.
#[derive(Debug)]
pub struct MaxReplicas {
    max: i64,
}

impl MaxReplicas {
    pub fn new(max: i64) -> Self {
        Self { max }
    }

    pub fn check_replicas(&self, replicas: i64) -> Result<(), String> {
        if !(0..=self.max).contains(&replicas) {
            return Err(format!(
                "replicas {replicas} is outside the quota bound 0..={}",
                self.max
            ));
        }
        Ok(())
    }
}

/// Validates `namespace` and `name` components as DNS-1123-ish identifiers:
/// lowercase alphanumeric with `-` and `.`, no empty or edge separators, and a
/// length cap. This is a safety predicate, not a full RFC 1123 implementation.
#[derive(Debug, Default)]
pub struct ValidK8sName;

impl crate::guardrail::Guardrail for ValidK8sName {
    fn check(&self, target: &str) -> Result<(), String> {
        for part in target.split('/') {
            if part.is_empty() || part.len() > 253 {
                return Err(format!("'{part}' is not a valid kubernetes name"));
            }
            if !part
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
            {
                return Err(format!("'{part}' contains characters outside [a-z0-9.-]"));
            }
            if part.starts_with('-')
                || part.ends_with('-')
                || part.starts_with('.')
                || part.ends_with('.')
            {
                return Err(format!("'{part}' has a leading or trailing separator"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use argus_domain::{CapabilityRequest, PolicyDecision, Principal, RequestContext};
    use semver::Version;
    use serde_json::json;
    use std::sync::Mutex;
    use uuid::Uuid;

    /// A scripted cluster: records calls, optionally fails, and answers the
    /// live-state reads.
    #[derive(Default)]
    struct MockCluster {
        calls: Mutex<Vec<String>>,
        pod_owned: Mutex<std::collections::BTreeSet<String>>,
        pods_running: Mutex<std::collections::BTreeSet<String>>,
    }

    impl ClusterController for MockCluster {
        fn restart_pod(&self, namespace: &str, name: &str) -> Result<(), RemediationError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("restart-pod {namespace}/{name}"));
            Ok(())
        }

        fn delete_pod(&self, namespace: &str, name: &str) -> Result<(), RemediationError> {
            let key = format!("{namespace}/{name}");
            let owned = self.pod_owned.lock().unwrap().contains(&key);
            if !owned {
                return Err(RemediationError::Failed(
                    "refusing to delete a pod without a controller owner reference".into(),
                ));
            }
            self.calls.lock().unwrap().push(format!("delete-pod {key}"));
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

        fn set_node_schedulable(
            &self,
            name: &str,
            schedulable: bool,
        ) -> Result<(), RemediationError> {
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

        fn deployment_available(&self, _: &str, _: &str) -> Result<bool, RemediationError> {
            Ok(true)
        }

        fn node_schedulable(&self, _: &str) -> Result<bool, RemediationError> {
            Ok(true)
        }

        fn read(
            &self,
            resource: &str,
            _: Option<&str>,
            _: Option<&str>,
        ) -> Result<Value, RemediationError> {
            Ok(json!({ "resource": resource }))
        }
    }

    fn action_for(capability: &str, arguments: Value) -> AuthorizedAction {
        let request = CapabilityRequest::new(
            CapabilityId::new(capability).unwrap(),
            Principal::new(Some(0), Some(0)),
            None,
            arguments,
            RequestContext::new(
                Uuid::new_v4(),
                Version::new(0, 1, 0),
                Principal::new(Some(0), Some(0)),
                Utc::now(),
            ),
        );
        AuthorizedAction::new(request, PolicyDecision::allow("test", "ok")).expect("allowed")
    }

    fn executor(protected: &[&str], max_replicas: i64) -> (KubernetesExecutor, Arc<MockCluster>) {
        let cluster = Arc::new(MockCluster::default());
        let guardrails =
            kubernetes_guardrails(&protected.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        (
            KubernetesExecutor::new(cluster.clone(), guardrails, max_replicas),
            cluster,
        )
    }

    #[test]
    fn the_executor_handles_exactly_the_non_deferred_k8s_capabilities() {
        for capability in [
            CapabilityId::K8S_POD_RESTART,
            CapabilityId::K8S_DEPLOYMENT_ROLLBACK,
            CapabilityId::K8S_NODE_CORDON,
            CapabilityId::K8S_POD_READ,
        ] {
            assert!(KubernetesExecutor::handles(
                &CapabilityId::new(capability).unwrap()
            ));
        }
        for deferred in [
            CapabilityId::K8S_NODE_DRAIN,
            CapabilityId::K8S_WORKLOAD_RESCHEDULE,
            CapabilityId::HOST_SERVICE_RESTART,
        ] {
            assert!(
                !KubernetesExecutor::handles(&CapabilityId::new(deferred).unwrap()),
                "{deferred} must have no executor route"
            );
        }
    }

    #[test]
    fn a_pod_restart_reaches_the_controller_with_typed_arguments() {
        let (executor, cluster) = executor(&[], 10);
        let result = executor
            .execute(&action_for(
                CapabilityId::K8S_POD_RESTART,
                json!({ "namespace": "prod", "name": "api-7d9f" }),
            ))
            .expect("restart executes");
        assert_eq!(result.evidence["namespace"], "prod");
        assert_eq!(
            cluster.calls.lock().unwrap().clone(),
            vec!["restart-pod prod/api-7d9f".to_string()]
        );
    }

    #[test]
    fn kube_system_is_protected_from_pod_effects_by_default() {
        let (executor, cluster) = executor(&["kube-system"], 10);
        let err = executor
            .execute(&action_for(
                CapabilityId::K8S_POD_DELETE,
                json!({ "namespace": "kube-system", "name": "coredns-abc" }),
            ))
            .unwrap_err();
        assert!(
            matches!(err, ExecutionError::GuardrailViolation(_)),
            "the protected namespace must refuse before the API: {err:?}"
        );
        assert!(cluster.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn malformed_object_names_are_refused_at_the_boundary() {
        let (executor, cluster) = executor(&[], 10);
        // `api--x` is a legal DNS-1123 name (consecutive inner hyphens are
        // allowed); the refusals are case, edge separators, spaces, empties.
        for name in ["Api", ".hidden", "a b", ""] {
            let arguments = if name.is_empty() {
                json!({ "namespace": "prod" })
            } else {
                json!({ "namespace": "prod", "name": name })
            };
            let err = executor
                .execute(&action_for(CapabilityId::K8S_DEPLOYMENT_RESTART, arguments))
                .unwrap_err();
            assert!(
                matches!(
                    err,
                    ExecutionError::GuardrailViolation(_) | ExecutionError::Failed(_)
                ),
                "name '{name}' must be refused: {err:?}"
            );
        }
        assert!(cluster.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn scaling_beyond_the_quota_is_refused_before_the_api() {
        let (executor, cluster) = executor(&[], 10);
        let err = executor
            .execute(&action_for(
                CapabilityId::K8S_WORKLOAD_SCALE,
                json!({ "namespace": "prod", "name": "api", "replicas": 50 }),
            ))
            .unwrap_err();
        assert!(matches!(err, ExecutionError::Failed(_)), "{err:?}");
        assert!(cluster.calls.lock().unwrap().is_empty());

        executor
            .execute(&action_for(
                CapabilityId::K8S_WORKLOAD_SCALE,
                json!({ "namespace": "prod", "name": "api", "replicas": 10 }),
            ))
            .expect("a scale within the bound executes");
    }

    #[test]
    fn an_ownerless_pod_delete_is_refused_by_the_live_check() {
        let (executor, cluster) = executor(&[], 10);
        let err = executor
            .execute(&action_for(
                CapabilityId::K8S_POD_DELETE,
                json!({ "namespace": "prod", "name": "bare" }),
            ))
            .unwrap_err();
        assert!(matches!(err, ExecutionError::Failed(_)), "{err:?}");
        assert!(
            cluster.calls.lock().unwrap().is_empty(),
            "the live owner check runs before the delete"
        );
    }

    #[test]
    fn the_deferred_drain_and_reschedule_are_unsupported_here() {
        let (executor, _) = executor(&[], 10);
        for capability in [
            CapabilityId::K8S_NODE_DRAIN,
            CapabilityId::K8S_WORKLOAD_RESCHEDULE,
        ] {
            let err = executor
                .execute(&action_for(capability, json!({ "name": "node-1" })))
                .unwrap_err();
            assert!(
                matches!(err, ExecutionError::Unsupported(_)),
                "{capability} has no effect to route to: {err:?}"
            );
        }
    }

    #[test]
    fn cordon_and_uncordon_are_opposite_scheduling_effects() {
        let (executor, cluster) = executor(&[], 10);
        executor
            .execute(&action_for(
                CapabilityId::K8S_NODE_CORDON,
                json!({ "name": "node-1" }),
            ))
            .expect("cordon executes");
        executor
            .execute(&action_for(
                CapabilityId::K8S_NODE_UNCORDON,
                json!({ "name": "node-1" }),
            ))
            .expect("uncordon executes");
        assert_eq!(
            cluster.calls.lock().unwrap().clone(),
            vec![
                "node node-1 schedulable=false".to_string(),
                "node node-1 schedulable=true".to_string(),
            ]
        );
    }

    #[test]
    fn the_unavailable_controller_degrades_every_operation() {
        let cluster = UnavailableClusterController;
        assert!(cluster.restart_pod("prod", "api").is_err());
        assert!(cluster.pod_running("prod", "api").is_err());
    }
}
