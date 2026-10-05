//! Deterministic troubleshooting evidence bundles (spec 003 M4, CAP-13,
//! FR-015, ADR-0037 §5).
//!
//! For CrashLoopBackOff, OOMKilled, ImagePullBackOff, and Pending, the bundle
//! is gathered from typed inputs *before* any AI reasoning is invoked — a pure
//! function of the pod, its events, and the deployment that owns it. Nothing
//! here reasons about causes; it assembles evidence with cited fields only.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::{Deployment, KubeEvent, Pod};

/// The failure signature a bundle recognizes. One bundle carries exactly one
/// primary signature, detected by the precedence below.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Signature {
    /// A container is in `CrashLoopBackOff`.
    CrashLoopBackOff,
    /// A container's last termination was `OOMKilled`.
    OomKilled,
    /// A container is in `ImagePullBackOff` (or `ErrImagePull`).
    ImagePullBackOff,
    /// The pod cannot schedule (`Pending` without a node, or unschedulable).
    Pending,
}

impl Signature {
    /// The signature's stable string form, used in evidence and events.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CrashLoopBackOff => "crash_loop_back_off",
            Self::OomKilled => "oom_killed",
            Self::ImagePullBackOff => "image_pull_back_off",
            Self::Pending => "pending",
        }
    }
}

/// The deterministic evidence bundle for one troubled pod (AC-011).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceBundle {
    /// The pod the bundle describes.
    pub pod: String,
    pub namespace: String,
    /// The recognized primary signature.
    pub signature: Signature,
    /// The owning workload, when the pod has one (an ownerless pod's bundle
    /// records that absence — it matters for remediation safety).
    pub owner: Option<(String, String)>,
    pub restart_count: u32,
    /// Per-container evidence, cited from the pod's status.
    pub containers: Vec<ContainerEvidence>,
    /// The events that involve this pod, deterministically filtered and ordered.
    pub events: Vec<String>,
    /// The owning deployment's rollout state, when one was supplied.
    pub deployment: Option<DeploymentSummary>,
}

/// Per-container evidence with cited status fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerEvidence {
    pub name: String,
    pub ready: bool,
    pub restart_count: u32,
    /// `state.waiting.reason`, when waiting.
    pub waiting_reason: Option<String>,
    /// `lastState.terminated` reason and exit code, when present.
    pub last_termination: Option<(String, i32)>,
}

/// The owning deployment's rollout state, when supplied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeploymentSummary {
    pub name: String,
    pub replicas: i64,
    pub ready_replicas: i64,
    pub available_replicas: i64,
}

/// Recognizes the pod's primary failure signature, if any.
///
/// Precedence follows remediation relevance: a crash loop is actionable even
/// when the image also failed earlier; a Pending pod that never started has no
/// termination evidence to rank higher.
pub fn detect_signature(pod: &Pod) -> Option<Signature> {
    if pod.waiting_in("CrashLoopBackOff") {
        return Some(Signature::CrashLoopBackOff);
    }
    if pod.last_terminated_as("OOMKilled") {
        return Some(Signature::OomKilled);
    }
    if pod.waiting_in("ImagePullBackOff") || pod.waiting_in("ErrImagePull") {
        return Some(Signature::ImagePullBackOff);
    }
    if pod.phase == "Pending" {
        return Some(Signature::Pending);
    }
    None
}

/// Assembles the bundle for a pod, or `None` when the pod shows no recognized
/// signature (a healthy pod needs no bundle — absence is the signal).
pub fn gather_evidence(
    pod: &Pod,
    events: &[KubeEvent],
    deployment: Option<&Deployment>,
) -> Option<EvidenceBundle> {
    let signature = detect_signature(pod)?;

    // Deterministic event selection: events naming this pod, in input order,
    // rendered `reason xN: message`. No ranking, no model.
    let events = events
        .iter()
        .filter(|event| {
            event.namespace == pod.namespace
                && event.involved_object == pod.name
                && event.kind == "Pod"
        })
        .map(|event| format!("{} x{}: {}", event.reason, event.count, event.message))
        .collect();

    Some(EvidenceBundle {
        pod: pod.name.clone(),
        namespace: pod.namespace.clone(),
        signature,
        owner: pod.owner_kind.clone().zip(pod.owner_name.clone()),
        restart_count: pod.restarts(),
        containers: pod
            .containers
            .iter()
            .map(|c| ContainerEvidence {
                name: c.name.clone(),
                ready: c.ready,
                restart_count: c.restart_count,
                waiting_reason: c.waiting_reason.clone(),
                last_termination: c.last_termination.clone(),
            })
            .collect(),
        events,
        deployment: deployment.map(|d| DeploymentSummary {
            name: d.name.clone(),
            replicas: d.replicas,
            ready_replicas: d.ready_replicas,
            available_replicas: d.available_replicas,
        }),
    })
}

/// Every recognized bundle in a pod list, in list order. The pass a provider
/// runs over each poll: deterministic, no AI, no ranking.
pub fn gather_all(
    pods: &[Pod],
    events: &[KubeEvent],
    deployments: &[Deployment],
) -> Vec<EvidenceBundle> {
    pods.iter()
        .filter_map(|pod| {
            let deployment = deployment_for(pod, deployments);
            gather_evidence(pod, events, deployment)
        })
        .collect()
}

/// Maps a ReplicaSet-owned pod to its Deployment: the generated ReplicaSet
/// carries the deployment's name as a `-`-separated prefix, so the match is
/// the longest deployment name that prefixes the ReplicaSet name. A pod
/// naming its deployment directly also matches. Ambiguity resolves to the
/// longest name — deterministic, and it prefers the most specific owner.
fn deployment_for<'a>(pod: &Pod, deployments: &'a [Deployment]) -> Option<&'a Deployment> {
    if pod.owner_kind.as_deref() != Some("ReplicaSet") {
        return None;
    }
    let rs = pod.owner_name.as_deref()?;
    deployments
        .iter()
        .filter(|d| rs == d.name || rs.strip_prefix(&format!("{}-", d.name)).is_some())
        .max_by_key(|d| d.name.len())
}

/// Serializes a bundle to the JSON shape carried in evidence payloads.
pub fn to_evidence_value(bundle: &EvidenceBundle) -> Value {
    serde_json::to_value(bundle).unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ContainerStatus, parse_pod};
    use serde_json::json;

    fn pod_with(statuses: Vec<ContainerStatus>, phase: &str, node: Option<&str>) -> Pod {
        Pod {
            namespace: "prod".into(),
            name: "api-7d9f".into(),
            phase: phase.into(),
            reason: None,
            owner_kind: Some("ReplicaSet".into()),
            owner_name: Some("api-7d9f-rs".into()),
            node: node.map(str::to_owned),
            containers: statuses,
            conditions: vec![],
            created_at: None,
        }
    }

    fn waiting(name: &str, reason: &str, restarts: u32) -> ContainerStatus {
        ContainerStatus {
            name: name.into(),
            container_id: None,
            restart_count: restarts,
            ready: false,
            waiting_reason: Some(reason.into()),
            last_termination: None,
        }
    }

    fn terminated(name: &str, reason: &str, code: i32, restarts: u32) -> ContainerStatus {
        ContainerStatus {
            name: name.into(),
            container_id: Some("containerd://abc".into()),
            restart_count: restarts,
            ready: false,
            waiting_reason: None,
            last_termination: Some((reason.into(), code)),
        }
    }

    #[test]
    fn each_signature_is_detected_from_its_own_evidence() {
        let crashloop = pod_with(
            vec![waiting("api", "CrashLoopBackOff", 7)],
            "Running",
            Some("node-1"),
        );
        assert_eq!(
            detect_signature(&crashloop),
            Some(Signature::CrashLoopBackOff)
        );

        let oom = pod_with(
            vec![terminated("api", "OOMKilled", 137, 3)],
            "Running",
            Some("node-1"),
        );
        assert_eq!(detect_signature(&oom), Some(Signature::OomKilled));

        let pull = pod_with(vec![waiting("api", "ImagePullBackOff", 0)], "Pending", None);
        assert_eq!(detect_signature(&pull), Some(Signature::ImagePullBackOff));

        // ErrImagePull is the same failure family.
        let err_pull = pod_with(vec![waiting("api", "ErrImagePull", 0)], "Pending", None);
        assert_eq!(
            detect_signature(&err_pull),
            Some(Signature::ImagePullBackOff)
        );

        let pending = pod_with(vec![], "Pending", None);
        assert_eq!(detect_signature(&pending), Some(Signature::Pending));

        let healthy = pod_with(
            vec![ContainerStatus {
                name: "api".into(),
                container_id: Some("containerd://abc".into()),
                restart_count: 0,
                ready: true,
                waiting_reason: None,
                last_termination: None,
            }],
            "Running",
            Some("node-1"),
        );
        assert_eq!(
            detect_signature(&healthy),
            None,
            "healthy pods need no bundle"
        );
    }

    #[test]
    fn a_crash_loop_bundle_precedes_a_pull_failure() {
        // Both present: the crash loop is the actionable primary signature.
        let pod = pod_with(
            vec![
                waiting("sidecar", "ImagePullBackOff", 0),
                terminated("api", "Error", 1, 9),
                waiting("api", "CrashLoopBackOff", 9),
            ],
            "Running",
            Some("node-1"),
        );
        assert_eq!(detect_signature(&pod), Some(Signature::CrashLoopBackOff));
    }

    #[test]
    fn the_bundle_carries_cited_evidence_and_pod_events_only() {
        let raw = json!({
            "metadata": { "name": "api-7d9f", "namespace": "prod",
                "ownerReferences": [ { "kind": "ReplicaSet", "name": "api-7d9f-rs" } ] },
            "spec": { "nodeName": "node-1" },
            "status": { "phase": "Running", "containerStatuses": [
                { "name": "api", "restartCount": 7, "ready": false,
                  "state": { "waiting": { "reason": "CrashLoopBackOff" } },
                  "lastState": { "terminated": { "reason": "Error", "exitCode": 1 } } }
            ] }
        });
        let pod = parse_pod(&raw).unwrap();
        let events = vec![
            KubeEvent {
                namespace: "prod".into(),
                involved_object: "api-7d9f".into(),
                reason: "BackOff".into(),
                message: "Back-off restarting failed container".into(),
                count: 12,
                kind: "Pod".into(),
            },
            KubeEvent {
                namespace: "prod".into(),
                involved_object: "other-svc".into(),
                reason: "Unrelated".into(),
                message: "someone else's event".into(),
                count: 1,
                kind: "Pod".into(),
            },
        ];
        let deployment = Deployment {
            namespace: "prod".into(),
            name: "api".into(),
            replicas: 3,
            ready_replicas: 2,
            available_replicas: 2,
        };

        let bundle = gather_evidence(&pod, &events, Some(&deployment)).unwrap();
        assert_eq!(bundle.signature, Signature::CrashLoopBackOff);
        assert_eq!(bundle.restart_count, 7);
        assert_eq!(
            bundle.owner,
            Some(("ReplicaSet".to_string(), "api-7d9f-rs".to_string()))
        );
        assert_eq!(bundle.events.len(), 1, "only this pod's events");
        assert!(bundle.events[0].starts_with("BackOff x12:"));
        assert_eq!(
            bundle.deployment.as_ref().map(|d| d.ready_replicas),
            Some(2)
        );

        // Determinism: the same inputs always produce the same bundle.
        let again = gather_evidence(&pod, &events, Some(&deployment)).unwrap();
        assert_eq!(bundle, again);
    }

    #[test]
    fn gather_all_maps_replicaset_owners_to_deployments_and_skips_healthy_pods() {
        let raw = json!({ "items": [
            { "metadata": { "name": "api-7d9f", "namespace": "prod",
                "ownerReferences": [ { "kind": "ReplicaSet", "name": "api-7d9f-rs" } ] },
              "spec": { "nodeName": "node-1" },
              "status": { "phase": "Running", "containerStatuses": [
                { "name": "api", "restartCount": 7, "ready": false,
                  "state": { "waiting": { "reason": "CrashLoopBackOff" } } } ] } },
            { "metadata": { "name": "fine-0", "namespace": "prod" },
              "spec": { "nodeName": "node-2" },
              "status": { "phase": "Running", "containerStatuses": [
                { "name": "app", "restartCount": 0, "ready": true,
                  "containerID": "containerd://x" } ] } }
        ] });
        let pods = crate::model::parse_pods(&raw).unwrap();
        let deployments = vec![Deployment {
            namespace: "prod".into(),
            name: "api".into(),
            replicas: 3,
            ready_replicas: 2,
            available_replicas: 2,
        }];

        let bundles = gather_all(&pods, &[], &deployments);
        assert_eq!(bundles.len(), 1, "only the unhealthy pod yields a bundle");
        assert_eq!(
            bundles[0].deployment.as_ref().map(|d| d.name.as_str()),
            Some("api"),
            "the generated ReplicaSet name maps to its deployment"
        );
    }

    #[test]
    fn a_bundle_serializes_with_a_stable_shape() {
        let mut pod = pod_with(vec![waiting("api", "ImagePullBackOff", 0)], "Pending", None);
        pod.owner_kind = None;
        pod.owner_name = None;
        let bundle = gather_evidence(&pod, &[], None).unwrap();
        let value = to_evidence_value(&bundle);
        assert_eq!(value["signature"], "image_pull_back_off");
        assert_eq!(value["pod"], "api-7d9f");
        assert!(
            value["owner"].is_null(),
            "an ownerless pod records the absence"
        );
    }
}
