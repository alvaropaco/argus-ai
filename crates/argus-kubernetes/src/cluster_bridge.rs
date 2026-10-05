//! The `kube`-feature bridge from the async API client to the executor's sync
//! [`ClusterController`](argus_executor::ClusterController) port.
//!
//! The executor boundary is deliberately synchronous (ADR-0028); the
//! Kubernetes API client is async. This bridge enters the ambient multi-thread
//! runtime through `block_in_place` — legal from every daemon context. Called
//! from a non-tokio thread it panics loudly rather than silently deadlocking.
//!
//! Effects are typed API requests: no `kubectl`, no shell, no string
//! interpolation into commands (ADR-0037 §4). This milestone ships the
//! bodyless effects (pod restart/delete, job cleanup) and all reads; the
//! PATCH-based effects (deployment restart/rollback, scale, node scheduling)
//! degrade with a clear reason until the cluster-configuration wiring lands —
//! they never fake a request they cannot form.

use argus_executor::{ClusterController, RemediationError};
use http::Method;
use serde_json::Value;

/// The sync bridge over a connected kube client.
#[derive(Clone)]
pub struct KubeClusterController {
    client: kube::Client,
}

impl KubeClusterController {
    pub fn new(client: kube::Client) -> Self {
        Self { client }
    }

    /// A bodyless API request (GET reads, DELETE effects).
    fn request(method: Method, path: &str) -> http::Request<Vec<u8>> {
        http::Request::builder()
            .method(method)
            .uri(path)
            .body(Vec::new())
            .expect("a statically valid request")
    }

    /// Runs one async API request on the ambient runtime.
    fn run<F, T>(&self, what: &'static str, fut: F) -> Result<T, RemediationError>
    where
        F: std::future::Future<Output = Result<T, kube::Error>>,
    {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async { fut.await })
        })
        .map_err(|e| RemediationError::Failed(format!("{what} failed: {e}")))
    }

    async fn request_json(&self, method: Method, path: &str) -> Result<Value, kube::Error> {
        let body = self
            .client
            .request_text(Self::request(method, path))
            .await?;
        serde_json::from_str(&body).map_err(kube::Error::SerdeError)
    }

    fn pod_path(namespace: &str, name: &str) -> String {
        format!("/api/v1/namespaces/{namespace}/pods/{name}")
    }

    fn deployment_path(namespace: &str, name: &str) -> String {
        format!("/apis/apps/v1/namespaces/{namespace}/deployments/{name}")
    }

    fn unimplemented(what: &str) -> RemediationError {
        RemediationError::Unavailable(format!(
            "{what} needs the cluster write plumbing (patch body support); it degrades rather than faking a request"
        ))
    }

    /// Whether the live pod carries a controller owner reference. A pod
    /// without one is unrecoverable on delete, so the restart/delete path
    /// refuses it (contract: "deny-by-default where not controller-managed").
    fn pod_is_owned(&self, namespace: &str, name: &str) -> Result<bool, RemediationError> {
        let pod = self.run(
            "pod get",
            self.request_json(Method::GET, &Self::pod_path(namespace, name)),
        )?;
        let owned = pod
            .get("metadata")
            .and_then(|m| m.get("ownerReferences"))
            .and_then(Value::as_array)
            .is_some_and(|refs| !refs.is_empty());
        Ok(owned)
    }
}

impl ClusterController for KubeClusterController {
    fn restart_pod(&self, namespace: &str, name: &str) -> Result<(), RemediationError> {
        // A pod restart *is* a controller-rescheduled delete: the same typed
        // effect, with the ownership check guarding bare pods.
        if !self.pod_is_owned(namespace, name)? {
            return Err(RemediationError::Failed(
                "refusing to restart a pod without a controller owner reference".into(),
            ));
        }
        self.run(
            "pod restart",
            self.request_json(Method::DELETE, &Self::pod_path(namespace, name)),
        )?;
        Ok(())
    }

    fn delete_pod(&self, namespace: &str, name: &str) -> Result<(), RemediationError> {
        if !self.pod_is_owned(namespace, name)? {
            return Err(RemediationError::Failed(
                "refusing to delete a pod without a controller owner reference".into(),
            ));
        }
        self.run(
            "pod delete",
            self.request_json(Method::DELETE, &Self::pod_path(namespace, name)),
        )?;
        Ok(())
    }

    fn restart_deployment(&self, _namespace: &str, _name: &str) -> Result<(), RemediationError> {
        Err(Self::unimplemented("deployment rollout-restart"))
    }

    fn rollback_deployment(
        &self,
        _namespace: &str,
        _name: &str,
        _revision: Option<i64>,
    ) -> Result<(), RemediationError> {
        Err(Self::unimplemented("deployment rollback"))
    }

    fn scale_workload(
        &self,
        _namespace: &str,
        _name: &str,
        _replicas: i64,
    ) -> Result<(), RemediationError> {
        Err(Self::unimplemented("workload scale"))
    }

    fn set_node_schedulable(
        &self,
        _name: &str,
        _schedulable: bool,
    ) -> Result<(), RemediationError> {
        Err(Self::unimplemented("node scheduling change"))
    }

    fn cleanup_job(&self, namespace: &str, name: &str) -> Result<(), RemediationError> {
        self.run(
            "job cleanup",
            self.request_json(
                Method::DELETE,
                &format!("/apis/batch/v1/namespaces/{namespace}/jobs/{name}"),
            ),
        )?;
        Ok(())
    }

    fn pod_running(&self, namespace: &str, name: &str) -> Result<bool, RemediationError> {
        let pod = self.run(
            "pod get",
            self.request_json(Method::GET, &Self::pod_path(namespace, name)),
        )?;
        Ok(pod
            .get("status")
            .and_then(|s| s.get("phase"))
            .and_then(Value::as_str)
            .is_some_and(|phase| phase == "Running"))
    }

    fn deployment_available(&self, namespace: &str, name: &str) -> Result<bool, RemediationError> {
        let deployment = self.run(
            "deployment get",
            self.request_json(Method::GET, &Self::deployment_path(namespace, name)),
        )?;
        let status = deployment.get("status").cloned().unwrap_or(Value::Null);
        let desired = status.get("replicas").and_then(Value::as_i64).unwrap_or(0);
        let available = status
            .get("availableReplicas")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        Ok(desired > 0 && available >= desired)
    }

    fn node_schedulable(&self, name: &str) -> Result<bool, RemediationError> {
        let node = self.run(
            "node get",
            self.request_json(Method::GET, &format!("/api/v1/nodes/{name}")),
        )?;
        Ok(node
            .get("spec")
            .and_then(|s| s.get("unschedulable"))
            .and_then(Value::as_bool)
            .map(|unschedulable| !unschedulable)
            .unwrap_or(true))
    }

    fn read(
        &self,
        resource: &str,
        namespace: Option<&str>,
        name: Option<&str>,
    ) -> Result<Value, RemediationError> {
        let path = match (resource, namespace, name) {
            ("cluster", _, _) => "/version".to_string(),
            ("nodes", _, Some(name)) => format!("/api/v1/nodes/{name}"),
            ("nodes", _, None) => "/api/v1/nodes".to_string(),
            ("pods", Some(ns), Some(name)) => format!("/api/v1/namespaces/{ns}/pods/{name}"),
            ("pods", Some(ns), None) => format!("/api/v1/namespaces/{ns}/pods"),
            ("pods", None, _) => "/api/v1/pods".to_string(),
            ("deployments", Some(ns), Some(name)) => {
                format!("/apis/apps/v1/namespaces/{ns}/deployments/{name}")
            }
            ("deployments", Some(ns), None) => {
                format!("/apis/apps/v1/namespaces/{ns}/deployments")
            }
            ("deployments", None, _) => "/apis/apps/v1/deployments".to_string(),
            (other, _, _) => {
                return Err(RemediationError::Failed(format!(
                    "unknown kubernetes read resource '{other}'"
                )));
            }
        };
        self.run("cluster read", self.request_json(Method::GET, &path))
    }
}
