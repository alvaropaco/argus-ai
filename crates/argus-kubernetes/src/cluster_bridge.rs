//! The `kube`-feature bridge from the async API client to the executor's sync
//! [`ClusterController`](argus_executor::ClusterController) port.
//!
//! The executor boundary is deliberately synchronous (ADR-0028); the
//! Kubernetes API client is async. This bridge enters the ambient multi-thread
//! runtime through `block_in_place` — legal from every daemon context. Called
//! from a non-tokio thread it panics loudly rather than silently deadlocking.
//!
//! Effects are typed API requests: no `kubectl`, no shell, no string
//! interpolation into commands (ADR-0037 §4). Reads and the bodyless
//! effects (pod restart/delete, job cleanup) are plain GET/DELETE; the
//! write effects are JSON PATCH requests shaped exactly as `kubectl`
//! shapes them (rollout-restart annotation, replicas, unschedulable,
//! template rollback from the owning ReplicaSet's revision).

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

    /// Connect from a kubeconfig file (or the ambient rules when `path` is
    /// `None`), optionally selecting a context. The string errors keep the
    /// caller (the daemon) free of kube-rs types.
    pub async fn from_kubeconfig(
        path: Option<&str>,
        context: Option<&str>,
    ) -> Result<Self, String> {
        use kube::config::Kubeconfig;
        let kubeconfig = match path {
            Some(path) => {
                Kubeconfig::read_from(path).map_err(|e| format!("read kubeconfig {path}: {e}"))?
            }
            None => Kubeconfig::read().map_err(|e| format!("read ambient kubeconfig: {e}"))?,
        };
        let options = kube::config::KubeConfigOptions {
            context: context.map(str::to_string),
            cluster: None,
            user: None,
        };
        let config = kube::Config::from_custom_kubeconfig(kubeconfig, &options)
            .await
            .map_err(|e| format!("load kubeconfig: {e}"))?;
        let client =
            kube::Client::try_from(config).map_err(|e| format!("connect to the cluster: {e}"))?;
        Ok(Self::new(client))
    }

    /// A bodyless API request (GET reads, DELETE effects).
    fn request(method: Method, path: &str) -> http::Request<Vec<u8>> {
        http::Request::builder()
            .method(method)
            .uri(path)
            .body(Vec::new())
            .expect("a statically valid request")
    }

    /// A JSON PATCH request. `patch_type` is the full content type (e.g.
    /// `application/strategic-merge-patch+json`).
    fn patch_request(path: &str, patch_type: &str, body: &Value) -> http::Request<Vec<u8>> {
        let body = serde_json::to_vec(body).expect("a statically serializable patch");
        http::Request::builder()
            .method(Method::PATCH)
            .uri(path)
            .header(http::header::CONTENT_TYPE, patch_type)
            .body(body)
            .expect("a statically valid patch request")
    }

    /// Runs one PATCH against the API.
    async fn patch_json(
        &self,
        path: &str,
        patch_type: &str,
        body: &Value,
    ) -> Result<Value, kube::Error> {
        let request = Self::patch_request(path, patch_type, body);
        let text = self.client.request_text(request).await?;
        serde_json::from_str(&text).map_err(kube::Error::SerdeError)
    }

    /// The rollout-restart patch `kubectl` applies: a `restartedAt` stamp on
    /// the pod template, which the deployment controller rolls out.
    fn rollout_restart_body(now: &str) -> Value {
        serde_json::json!({
            "spec": {
                "template": {
                    "metadata": {
                        "annotations": {
                            "kubectl.kubernetes.io/restartedAt": now
                        }
                    }
                }
            }
        })
    }

    fn scale_body(replicas: i64) -> Value {
        serde_json::json!({ "spec": { "replicas": replicas } })
    }

    fn schedulable_body(schedulable: bool) -> Value {
        serde_json::json!({ "spec": { "unschedulable": !schedulable } })
    }

    /// Runs one async API request on the ambient runtime.
    fn run<F, T>(&self, what: &'static str, fut: F) -> Result<T, RemediationError>
    where
        F: std::future::Future<Output = Result<T, kube::Error>>,
    {
        tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
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

    fn restart_deployment(&self, namespace: &str, name: &str) -> Result<(), RemediationError> {
        let path = Self::deployment_path(namespace, name);
        let body = Self::rollout_restart_body(&chrono::Utc::now().to_rfc3339());
        self.run(
            "deployment rollout-restart",
            self.patch_json(&path, "application/strategic-merge-patch+json", &body),
        )?;
        Ok(())
    }

    fn rollback_deployment(
        &self,
        namespace: &str,
        name: &str,
        revision: Option<i64>,
    ) -> Result<(), RemediationError> {
        // kubectl rollout undo: find the deployment's ReplicaSets, pick the
        // requested revision (or the previous one), and patch the pod
        // template back to that revision's shape.
        let deployment = self.run(
            "deployment get",
            self.request_json(Method::GET, &Self::deployment_path(namespace, name)),
        )?;
        let owner_uid = deployment
            .get("metadata")
            .and_then(|m| m.get("uid"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                RemediationError::Failed("deployment has no uid to correlate revisions".into())
            })?
            .to_string();

        let list = self.run(
            "replicaset list",
            self.request_json(
                Method::GET,
                &format!("/apis/apps/v1/namespaces/{namespace}/replicasets"),
            ),
        )?;
        let mut revisions: Vec<(i64, Value)> = Vec::new();
        if let Some(items) = list.get("items").and_then(Value::as_array) {
            for rs in items {
                let owned = rs
                    .get("metadata")
                    .and_then(|m| m.get("ownerReferences"))
                    .and_then(Value::as_array)
                    .is_some_and(|refs| {
                        refs.iter().any(|r| {
                            r.get("uid").and_then(Value::as_str) == Some(owner_uid.as_str())
                        })
                    });
                if !owned {
                    continue;
                }
                let revision = rs
                    .get("metadata")
                    .and_then(|m| m.get("annotations"))
                    .and_then(|a| a.get("deployment.kubernetes.io/revision"))
                    .and_then(Value::as_str)
                    .and_then(|r| r.parse::<i64>().ok());
                if let Some(revision) = revision {
                    revisions.push((revision, rs.clone()));
                }
            }
        }
        revisions.sort_by_key(|(revision, _)| std::cmp::Reverse(*revision));
        let target = match revision {
            Some(wanted) => revisions
                .iter()
                .find(|(r, _)| *r == wanted)
                .map(|(_, rs)| rs.clone()),
            None => revisions.get(1).map(|(_, rs)| rs.clone()),
        };
        let Some(rs) = target else {
            return Err(RemediationError::Unavailable(format!(
                "no revision to roll back to for deployment {namespace}/{name}"
            )));
        };
        let template = rs
            .get("spec")
            .and_then(|s| s.get("template"))
            .cloned()
            .ok_or_else(|| {
                RemediationError::Failed("revision's ReplicaSet carries no pod template".into())
            })?;
        let body = serde_json::json!({ "spec": { "template": template } });
        self.run(
            "deployment rollback",
            self.patch_json(
                &Self::deployment_path(namespace, name),
                "application/strategic-merge-patch+json",
                &body,
            ),
        )?;
        Ok(())
    }

    fn scale_workload(
        &self,
        namespace: &str,
        name: &str,
        replicas: i64,
    ) -> Result<(), RemediationError> {
        self.run(
            "workload scale",
            self.patch_json(
                &Self::deployment_path(namespace, name),
                "application/merge-patch+json",
                &Self::scale_body(replicas),
            ),
        )?;
        Ok(())
    }

    fn set_node_schedulable(&self, name: &str, schedulable: bool) -> Result<(), RemediationError> {
        self.run(
            "node scheduling change",
            self.patch_json(
                &format!("/api/v1/nodes/{name}"),
                "application/merge-patch+json",
                &Self::schedulable_body(schedulable),
            ),
        )?;
        Ok(())
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

#[cfg(test)]
mod patch_tests {
    use super::*;

    #[test]
    fn rollout_restart_patch_matches_kubectl_shape() {
        let body = KubeClusterController::rollout_restart_body("2026-10-06T15:00:00+00:00");
        assert_eq!(
            body["spec"]["template"]["metadata"]["annotations"]["kubectl.kubernetes.io/restartedAt"],
            "2026-10-06T15:00:00+00:00"
        );
        let request = KubeClusterController::patch_request(
            "/apis/apps/v1/namespaces/default/deployments/api",
            "application/strategic-merge-patch+json",
            &body,
        );
        assert_eq!(*request.method(), Method::PATCH);
        assert_eq!(
            request.headers().get(http::header::CONTENT_TYPE).unwrap(),
            "application/strategic-merge-patch+json"
        );
        let sent: Value = serde_json::from_slice(request.body()).unwrap();
        assert!(sent["spec"]["template"]["metadata"]["annotations"].is_object());
    }

    #[test]
    fn scale_and_scheduling_patches_are_minimal() {
        let scale = KubeClusterController::scale_body(3);
        assert_eq!(scale, serde_json::json!({ "spec": { "replicas": 3 } }));

        let cordon = KubeClusterController::schedulable_body(false);
        assert_eq!(
            cordon,
            serde_json::json!({ "spec": { "unschedulable": true } })
        );
        let uncordon = KubeClusterController::schedulable_body(true);
        assert_eq!(
            uncordon,
            serde_json::json!({ "spec": { "unschedulable": false } })
        );
    }
}
