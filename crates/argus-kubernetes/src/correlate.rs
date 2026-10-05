//! Cross-layer correlation: pod → container → cgroup → PID (spec 003 M4,
//! CAP-12, FR-014, ADR-0037 §3).
//!
//! Kubernetes is a control-plane layer over the Linux substrate, not a
//! separate telemetry silo. This module joins the control-plane view (pods and
//! their runtime container IDs) with the host view the sensor stack already
//! produces (`/proc`/cgroup records keyed by container ID), producing one
//! correlation record per pod container. It is a pure join — no reads, no
//! cluster access — so it is deterministic and fixture-tested.

use serde::{Deserialize, Serialize};

use crate::model::Pod;

/// One container's host-side evidence, as `argus-observe`/`argus-sensors`
/// already provide it: the runtime container ID (bare hex, without the
/// `containerd://` prefix), the cgroup v2 path it lives under, and the PIDs
/// observed inside it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostContainerRecord {
    /// The bare container ID, lowercased (runtime IDs are hex; comparison is
    /// case-insensitive on the join key).
    pub container_id: String,
    /// The cgroup v2 path the container's tasks run under, relative to the
    /// mount root (e.g. `kubepods.slice/.../cri-containerd-<id>.scope`).
    pub cgroup_path: String,
    /// PIDs observed in that cgroup at last inventory.
    pub pids: Vec<i32>,
    /// The host socket/ports observed for those PIDs, for socket-level
    /// correlation when supplied.
    pub sockets: Vec<String>,
}

/// One pod container joined across the control plane and the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorrelatedContainer {
    pub pod: String,
    pub namespace: String,
    pub container: String,
    /// The runtime container ID the pod status reports, normalized to the bare
    /// lowercase ID (the `containerd://` scheme is presentation, not identity).
    pub container_id: String,
    /// The host record joined onto it, when the container runs on this host.
    pub host: Option<JoinedHost>,
}

/// The host-side half of a correlation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinedHost {
    pub cgroup_path: String,
    pub pids: Vec<i32>,
    pub sockets: Vec<String>,
}

/// Joins every pod container against the host inventory.
///
/// Containers that do not run on this host (scheduled elsewhere) correlate to
/// `None` — the absence is itself evidence (the pod is remote), never an
/// error. The join key is the normalized bare container ID.
pub fn correlate(pods: &[Pod], host: &[HostContainerRecord]) -> Vec<CorrelatedContainer> {
    let mut index: std::collections::HashMap<String, &HostContainerRecord> =
        std::collections::HashMap::new();
    for record in host {
        index.insert(normalize_id(&record.container_id), record);
    }

    let mut joined = Vec::new();
    for pod in pods {
        for status in &pod.containers {
            let normalized = status
                .container_id
                .as_deref()
                .map(normalize_id)
                .unwrap_or_default();
            let host = index
                .get(&normalized)
                .filter(|_| !normalized.is_empty())
                .map(|record| JoinedHost {
                    cgroup_path: record.cgroup_path.clone(),
                    pids: record.pids.clone(),
                    sockets: record.sockets.clone(),
                });
            joined.push(CorrelatedContainer {
                pod: pod.name.clone(),
                namespace: pod.namespace.clone(),
                container: status.name.clone(),
                container_id: normalized,
                host,
            });
        }
    }
    joined
}

/// Normalizes a runtime container reference to the bare lowercase ID:
/// `containerd://abc123` and `abc123` join identically; an absent ID joins to
/// nothing.
fn normalize_id(raw: &str) -> String {
    raw.split_once("://")
        .map(|(_, id)| id)
        .unwrap_or(raw)
        .trim()
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ContainerStatus, parse_pods};
    use serde_json::json;

    fn host(id: &str, cgroup: &str, pids: &[i32]) -> HostContainerRecord {
        HostContainerRecord {
            container_id: id.to_string(),
            cgroup_path: cgroup.to_string(),
            pids: pids.to_vec(),
            sockets: vec![],
        }
    }

    fn running_pod(id: &str, name: &str) -> Pod {
        Pod {
            namespace: "prod".into(),
            name: name.into(),
            phase: "Running".into(),
            reason: None,
            owner_kind: None,
            owner_name: None,
            node: Some("node-1".into()),
            containers: vec![ContainerStatus {
                name: "app".into(),
                container_id: Some(id.to_string()),
                restart_count: 0,
                ready: true,
                waiting_reason: None,
                last_termination: None,
            }],
            conditions: vec![],
            created_at: None,
        }
    }

    #[test]
    fn a_pod_container_joins_its_cgroup_and_pids() {
        let pods = vec![running_pod("containerd://ABC123", "api")];
        let host_records = vec![host(
            "abc123",
            "kubepods.slice/kubepods-burstable.slice/cri-containerd-abc123.scope",
            &[4242, 4243],
        )];

        let joined = correlate(&pods, &host_records);
        assert_eq!(joined.len(), 1);
        let record = &joined[0];
        assert_eq!(record.container_id, "abc123", "IDs normalize and lowercase");
        let host = record.host.as_ref().expect("the local container joins");
        assert!(host.cgroup_path.contains("kubepods.slice"));
        assert_eq!(host.pids, vec![4242, 4243]);
    }

    #[test]
    fn a_remote_container_correlates_to_an_explicit_none() {
        // The pod is scheduled on another node: its container ID is absent
        // from this host's inventory, and the record says so rather than
        // failing.
        let pods = vec![running_pod("containerd://deadbeef", "remote-api")];
        let joined = correlate(&pods, &[host("abc123", "kubepods.slice/x", &[1])]);
        assert!(
            joined[0].host.is_none(),
            "absence is evidence, not an error"
        );
    }

    #[test]
    fn a_pod_without_a_running_container_id_joins_to_nothing() {
        let raw = json!({ "items": [
            { "metadata": { "name": "pending-pod", "namespace": "prod" },
              "spec": {},
              "status": { "phase": "Pending", "containerStatuses": [
                { "name": "app", "restartCount": 0, "ready": false,
                  "state": { "waiting": { "reason": "ImagePullBackOff" } } } ] } }
        ] });
        let pods = parse_pods(&raw).unwrap();
        let joined = correlate(&pods, &[host("abc123", "c", &[1])]);
        assert!(joined[0].host.is_none());
        assert_eq!(
            joined[0].container_id, "",
            "no ID normalizes to the empty key"
        );
    }

    #[test]
    fn the_join_is_deterministic_and_does_not_mutate_inputs() {
        let pods = vec![running_pod("containerd://abc123", "api")];
        let host_records = vec![host("ABC123", "cgroup", &[7])];
        let first = correlate(&pods, &host_records);
        let second = correlate(&pods, &host_records);
        assert_eq!(first, second);
        assert_eq!(
            host_records[0].container_id, "ABC123",
            "inputs are never rewritten"
        );
    }
}
