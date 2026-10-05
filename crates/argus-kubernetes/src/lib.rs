//! ARGUS optional Kubernetes provider (spec 003 M4, ADR-0037).
//!
//! Kubernetes is a runtime-optional provider: when no cluster is configured or
//! reachable, everything here degrades to a clear `Unavailable` and the
//! host/Linux path continues unaffected. The crate carries three concerns:
//!
//! - **model** — typed projections of pods/nodes/deployments/events and their
//!   deterministic parsers;
//! - **bundle** — the deterministic troubleshooting evidence bundles gathered
//!   before any AI reasoning (CAP-13);
//! - **correlate** — the pod → container → cgroup → PID join over the host
//!   inventory (CAP-12, ADR-0037 §3);
//! - **provider** — the optional [`KubernetesProvider`] seam; the `kube`
//!   feature adds the `kube-rs`-backed implementation and a sync
//!   [`argus_executor::ClusterController`] bridge for the self-healing
//!   capabilities.

pub mod bundle;
pub mod correlate;
pub mod model;
pub mod provider;

pub use bundle::{
    ContainerEvidence, DeploymentSummary, EvidenceBundle, Signature, detect_signature, gather_all,
    gather_evidence, to_evidence_value,
};
pub use correlate::{CorrelatedContainer, HostContainerRecord, JoinedHost, correlate};
pub use model::{
    ContainerStatus, Deployment, KubeEvent, KubernetesError, Node, ObjectRef, Pod,
    parse_deployments, parse_events, parse_nodes, parse_pod, parse_pods,
};
pub use provider::{KubernetesProvider, ProviderState, UnconfiguredProvider};

#[cfg(feature = "kube")]
pub use provider::kube_backend::KubeRsProvider;

#[cfg(feature = "kube")]
pub mod cluster_bridge;
#[cfg(feature = "kube")]
pub use cluster_bridge::KubeClusterController;
