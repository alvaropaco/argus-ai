//! Typed Kubernetes objects and their JSON parsing.
//!
//! The models carry exactly what observation, evidence bundles, and
//! correlation need — a projection, not a mirror of the Kubernetes API. The
//! parsers are pure functions from JSON payloads (as served by the API or
//! emitted by `k8s-openapi` serialization), so they are fixture-testable
//! anywhere (Principle 9, ADR-0032).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Adapter error, mirroring `argus-container`'s shape: `Unavailable` means no
/// cluster is configured or reachable, which callers treat as graceful
/// degradation, never as a core failure (ADR-0037 §2).
#[derive(Debug, thiserror::Error)]
pub enum KubernetesError {
    #[error("kubernetes is unavailable: {0}")]
    Unavailable(String),
    #[error("kubernetes error: {0}")]
    Other(String),
    #[error("failed to parse kubernetes response: {0}")]
    Parse(String),
}

/// A namespace-scoped object name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectRef {
    pub namespace: String,
    pub name: String,
}

impl ObjectRef {
    pub fn new(namespace: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            name: name.into(),
        }
    }

    /// The `namespace/name` form used in evidence and logs.
    pub fn full(&self) -> String {
        format!("{}/{}", self.namespace, self.name)
    }
}

/// A Kubernetes container's live status within a pod.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerStatus {
    pub name: String,
    /// The runtime container ID (e.g. `containerd://<hex>`), when running.
    pub container_id: Option<String>,
    pub restart_count: u32,
    pub ready: bool,
    /// `waiting.reason` (e.g. `CrashLoopBackOff`, `ImagePullBackOff`).
    pub waiting_reason: Option<String>,
    /// `lastState.terminated.reason` (e.g. `OOMKilled`) and its exit code.
    pub last_termination: Option<(String, i32)>,
}

/// A pod, projected to the fields observation and troubleshooting need.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pod {
    pub namespace: String,
    pub name: String,
    pub phase: String,
    /// The pod's `status.reason` (e.g. `Evicted`, or empty).
    pub reason: Option<String>,
    /// Names of the owning workload (Deployment via ReplicaSet) when present.
    pub owner_kind: Option<String>,
    pub owner_name: Option<String>,
    pub node: Option<String>,
    pub containers: Vec<ContainerStatus>,
    /// `spec.nodeName` scheduling state for Pending diagnosis.
    pub conditions: Vec<(String, String)>,
    pub created_at: Option<String>,
}

impl Pod {
    /// Whether any container is in the named waiting state.
    pub fn waiting_in(&self, reason: &str) -> bool {
        self.containers
            .iter()
            .any(|c| c.waiting_reason.as_deref() == Some(reason))
    }

    /// Whether any container's last termination was the named reason.
    pub fn last_terminated_as(&self, reason: &str) -> bool {
        self.containers.iter().any(|c| {
            c.last_termination
                .as_ref()
                .is_some_and(|(r, _)| r == reason)
        })
    }

    /// The highest restart count across containers.
    pub fn restarts(&self) -> u32 {
        self.containers
            .iter()
            .map(|c| c.restart_count)
            .max()
            .unwrap_or(0)
    }
}

/// A node, projected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    pub name: String,
    pub schedulable: bool,
    /// e.g. `Ready`, `MemoryPressure`, `DiskPressure`.
    pub conditions: Vec<(String, String)>,
}

/// A deployment, projected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Deployment {
    pub namespace: String,
    pub name: String,
    pub replicas: i64,
    pub ready_replicas: i64,
    pub available_replicas: i64,
}

/// A Kubernetes API event, projected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KubeEvent {
    pub namespace: String,
    pub involved_object: String,
    pub reason: String,
    pub message: String,
    pub count: i64,
    pub kind: String,
}

/// Parses a `PodList` JSON payload into the projection.
pub fn parse_pods(payload: &Value) -> Result<Vec<Pod>, KubernetesError> {
    let items = payload
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| KubernetesError::Parse("pod list lacks items".into()))?;
    items.iter().map(parse_pod).collect()
}

/// Parses one pod object.
pub fn parse_pod(item: &Value) -> Result<Pod, KubernetesError> {
    let metadata = item.get("metadata").unwrap_or(&Value::Null);
    let status = item.get("status").unwrap_or(&Value::Null);
    let spec = item.get("spec").unwrap_or(&Value::Null);

    let name = str_of(metadata, "name")
        .ok_or_else(|| KubernetesError::Parse("pod lacks metadata.name".into()))?;
    let namespace = str_of(metadata, "namespace").unwrap_or_else(|| "default".to_string());
    let phase = str_of(status, "phase").unwrap_or_default();
    let reason = str_of(status, "reason").filter(|r| !r.is_empty());

    let owner = metadata
        .get("ownerReferences")
        .and_then(Value::as_array)
        .and_then(|refs| refs.first())
        .map(|owner| {
            (
                str_of(owner, "kind").unwrap_or_default(),
                str_of(owner, "name").unwrap_or_default(),
            )
        });

    let containers = status
        .get("containerStatuses")
        .and_then(Value::as_array)
        .map(|statuses| {
            statuses
                .iter()
                .map(parse_container_status)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let conditions = status
        .get("conditions")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|c| Some((str_of(c, "type")?, str_of(c, "status")?)))
                .collect()
        })
        .unwrap_or_default();

    Ok(Pod {
        namespace,
        name,
        phase,
        reason,
        owner_kind: owner.as_ref().map(|(k, _)| k.clone()),
        owner_name: owner.as_ref().map(|(_, n)| n.clone()),
        node: str_of(spec, "nodeName").filter(|n| !n.is_empty()),
        containers,
        conditions,
        created_at: str_of(metadata, "creationTimestamp"),
    })
}

fn parse_container_status(status: &Value) -> ContainerStatus {
    let waiting_reason = status
        .get("state")
        .and_then(|s| s.get("waiting"))
        .and_then(|w| w.get("reason"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let last_termination = status
        .get("lastState")
        .and_then(|s| s.get("terminated"))
        .and_then(|t| {
            let reason = t.get("reason").and_then(Value::as_str)?;
            let code = t.get("exitCode").and_then(Value::as_i64)? as i32;
            Some((reason.to_string(), code))
        });
    ContainerStatus {
        name: str_of(status, "name").unwrap_or_default(),
        container_id: str_of(status, "containerID").filter(|id| !id.is_empty()),
        restart_count: status
            .get("restartCount")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32,
        ready: status
            .get("ready")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        waiting_reason,
        last_termination,
    }
}

/// Parses a `NodeList` JSON payload.
pub fn parse_nodes(payload: &Value) -> Result<Vec<Node>, KubernetesError> {
    let items = payload
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| KubernetesError::Parse("node list lacks items".into()))?;
    items
        .iter()
        .map(|item| {
            let status = item.get("status").unwrap_or(&Value::Null);
            let spec = item.get("spec").unwrap_or(&Value::Null);
            let conditions = status
                .get("conditions")
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .filter_map(|c| Some((str_of(c, "type")?, str_of(c, "status")?)))
                        .collect()
                })
                .unwrap_or_default();
            Ok(Node {
                name: str_of(item.get("metadata").unwrap_or(&Value::Null), "name")
                    .ok_or_else(|| KubernetesError::Parse("node lacks metadata.name".into()))?,
                schedulable: spec
                    .get("unschedulable")
                    .and_then(Value::as_bool)
                    .map(|unschedulable| !unschedulable)
                    .unwrap_or(true),
                conditions,
            })
        })
        .collect()
}

/// Parses a `DeploymentList` JSON payload.
pub fn parse_deployments(payload: &Value) -> Result<Vec<Deployment>, KubernetesError> {
    let items = payload
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| KubernetesError::Parse("deployment list lacks items".into()))?;
    items
        .iter()
        .map(|item| {
            let metadata = item.get("metadata").unwrap_or(&Value::Null);
            let status = item.get("status").unwrap_or(&Value::Null);
            Ok(Deployment {
                namespace: str_of(metadata, "namespace").unwrap_or_else(|| "default".into()),
                name: str_of(metadata, "name").ok_or_else(|| {
                    KubernetesError::Parse("deployment lacks metadata.name".into())
                })?,
                replicas: int_of(status, "replicas").unwrap_or(0),
                ready_replicas: int_of(status, "readyReplicas").unwrap_or(0),
                available_replicas: int_of(status, "availableReplicas").unwrap_or(0),
            })
        })
        .collect()
}

/// Parses an `EventList` JSON payload.
pub fn parse_events(payload: &Value) -> Result<Vec<KubeEvent>, KubernetesError> {
    let items = payload
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| KubernetesError::Parse("event list lacks items".into()))?;
    items
        .iter()
        .map(|item| {
            let metadata = item.get("metadata").unwrap_or(&Value::Null);
            let involved = item.get("involvedObject").unwrap_or(&Value::Null);
            Ok(KubeEvent {
                namespace: str_of(metadata, "namespace").unwrap_or_else(|| "default".into()),
                involved_object: str_of(involved, "name").unwrap_or_default(),
                reason: str_of(item, "reason").unwrap_or_default(),
                message: str_of(item, "message").unwrap_or_default(),
                count: int_of(item, "count").unwrap_or(1),
                kind: str_of(involved, "kind").unwrap_or_default(),
            })
        })
        .collect()
}

fn str_of(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn int_of(value: &Value, key: &str) -> Option<i64> {
    value.get(key).and_then(Value::as_i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn crashloop_pod() -> Value {
        json!({
            "metadata": {
                "name": "api-7d9f",
                "namespace": "prod",
                "creationTimestamp": "2026-10-01T10:00:00Z",
                "ownerReferences": [
                    { "kind": "ReplicaSet", "name": "api-7d9f-rs" }
                ]
            },
            "spec": { "nodeName": "node-1" },
            "status": {
                "phase": "Running",
                "containerStatuses": [
                    {
                        "name": "api",
                        "containerID": "containerd://abc123",
                        "restartCount": 7,
                        "ready": false,
                        "state": { "waiting": { "reason": "CrashLoopBackOff" } },
                        "lastState": {
                            "terminated": { "reason": "Error", "exitCode": 1 }
                        }
                    }
                ]
            }
        })
    }

    #[test]
    fn a_pod_parses_its_projection() {
        let pod = parse_pod(&crashloop_pod()).unwrap();
        assert_eq!(pod.namespace, "prod");
        assert_eq!(pod.name, "api-7d9f");
        assert_eq!(pod.owner_name.as_deref(), Some("api-7d9f-rs"));
        assert_eq!(pod.node.as_deref(), Some("node-1"));
        assert_eq!(pod.restarts(), 7);
        assert!(pod.waiting_in("CrashLoopBackOff"));
        assert_eq!(
            pod.containers[0].container_id.as_deref(),
            Some("containerd://abc123")
        );
    }

    #[test]
    fn a_pod_without_a_name_is_a_parse_error() {
        let err = parse_pod(&json!({ "status": {} })).unwrap_err();
        assert!(err.to_string().contains("metadata.name"));
    }

    #[test]
    fn pod_node_and_deployment_lists_parse() {
        let pods = parse_pods(&json!({ "items": [crashloop_pod()] })).unwrap();
        assert_eq!(pods.len(), 1);

        let nodes = parse_nodes(&json!({ "items": [
            { "metadata": { "name": "node-1" },
              "spec": { "unschedulable": true },
              "status": { "conditions": [ { "type": "Ready", "status": "True" } ] } }
        ] }))
        .unwrap();
        assert_eq!(nodes.len(), 1);
        assert!(
            !nodes[0].schedulable,
            "unschedulable maps to not schedulable"
        );
        assert_eq!(
            nodes[0].conditions,
            vec![("Ready".to_string(), "True".to_string())]
        );

        let deployments = parse_deployments(&json!({ "items": [
            { "metadata": { "name": "api", "namespace": "prod" },
              "status": { "replicas": 3, "readyReplicas": 2, "availableReplicas": 2 } }
        ] }))
        .unwrap();
        assert_eq!(deployments[0].replicas, 3);
        assert_eq!(deployments[0].ready_replicas, 2);
    }

    #[test]
    fn a_list_without_items_is_a_parse_error() {
        assert!(parse_pods(&json!({})).is_err());
        assert!(parse_nodes(&json!({})).is_err());
        assert!(parse_deployments(&json!({})).is_err());
        assert!(parse_events(&json!({})).is_err());
    }
}
