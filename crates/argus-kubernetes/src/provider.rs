//! The optional Kubernetes provider seam (ADR-0037 §1-§2).
//!
//! The core links to Kubernetes only through [`KubernetesProvider`]. When no
//! cluster is configured, or the API server is unreachable, the provider
//! reports degraded/unconfigured and contributes nothing — the host/Linux path
//! continues unaffected. The `kube`-backed implementation is feature-gated so
//! the single-host build stays lean.

use async_trait::async_trait;

use crate::model::{Deployment, KubeEvent, KubernetesError, Node, Pod};

/// The provider's availability, as the observation surfaces report it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderState {
    /// No cluster is configured; the provider contributes nothing.
    Unconfigured,
    /// The cluster is reachable.
    Available,
    /// A cluster is configured but the last contact failed; `reason` says why.
    Degraded(String),
}

/// The read-only monitoring seam over a Kubernetes cluster.
///
/// `#[async_trait]` keeps the trait object-safe: the daemon holds the
/// configured provider as a `dyn` value behind the graceful-degradation seam.
#[async_trait]
pub trait KubernetesProvider: Send + Sync {
    async fn state(&self) -> ProviderState;

    async fn pods(&self) -> Result<Vec<Pod>, KubernetesError>;
    async fn nodes(&self) -> Result<Vec<Node>, KubernetesError>;
    async fn deployments(&self) -> Result<Vec<Deployment>, KubernetesError>;
    async fn events(&self) -> Result<Vec<KubeEvent>, KubernetesError>;
}

/// The provider every installation starts with: no cluster, no contribution.
///
/// Reads fail with [`KubernetesError::Unavailable`] — a clear degradation, not
/// a core failure (ADR-0037 §2).
#[derive(Debug, Default)]
pub struct UnconfiguredProvider;

#[async_trait]
impl KubernetesProvider for UnconfiguredProvider {
    async fn state(&self) -> ProviderState {
        ProviderState::Unconfigured
    }

    async fn pods(&self) -> Result<Vec<Pod>, KubernetesError> {
        Err(KubernetesError::Unavailable(
            "no kubernetes cluster is configured".to_string(),
        ))
    }

    async fn nodes(&self) -> Result<Vec<Node>, KubernetesError> {
        Err(KubernetesError::Unavailable(
            "no kubernetes cluster is configured".to_string(),
        ))
    }

    async fn deployments(&self) -> Result<Vec<Deployment>, KubernetesError> {
        Err(KubernetesError::Unavailable(
            "no kubernetes cluster is configured".to_string(),
        ))
    }

    async fn events(&self) -> Result<Vec<KubeEvent>, KubernetesError> {
        Err(KubernetesError::Unavailable(
            "no kubernetes cluster is configured".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_unconfigured_provider_degrades_clearly() {
        let provider = UnconfiguredProvider;
        assert_eq!(provider.state().await, ProviderState::Unconfigured);
        let err = provider.pods().await.unwrap_err();
        assert!(
            err.to_string()
                .contains("no kubernetes cluster is configured"),
            "the degradation names itself: {err}"
        );
    }
}

#[cfg(feature = "kube")]
pub mod kube_backend {
    //! The `kube-rs`-backed provider (ADR-0037 §1).
    //!
    //! Reads go through the raw request path and are parsed by this crate's
    //! own projection parsers, so the provider stays decoupled from
    //! `k8s-openapi` type churn and shares its fixtures with the tests.

    use http::Method;
    use kube::Client;

    use super::*;
    use crate::model::{Deployment, KubeEvent, KubernetesError, Node, Pod};

    /// A provider bound to a kubeconfig/in-cluster client.
    #[derive(Clone)]
    pub struct KubeRsProvider {
        client: Client,
    }

    impl KubeRsProvider {
        /// Connects using the standard discovery order (in-cluster
        /// environment, then the user's kubeconfig). Fails clearly when
        /// neither is present — the caller degrades, nothing else breaks.
        pub async fn connect() -> Result<Self, KubernetesError> {
            let client = Client::try_default()
                .await
                .map_err(|e| KubernetesError::Unavailable(format!("no cluster: {e}")))?;
            Ok(Self { client })
        }

        /// A bodyless GET against the API server's root-relative path.
        fn get(path: &str) -> http::Request<Vec<u8>> {
            http::Request::builder()
                .method(Method::GET)
                .uri(path)
                .body(Vec::new())
                .expect("a statically valid request")
        }

        async fn get_json(&self, path: &str) -> Result<serde_json::Value, KubernetesError> {
            let body = self
                .client
                .request_text(Self::get(path))
                .await
                .map_err(|e| KubernetesError::Other(format!("GET {path} failed: {e}")))?;
            serde_json::from_str(&body)
                .map_err(|e| KubernetesError::Parse(format!("GET {path}: {e}")))
        }
    }

    #[async_trait]
    impl KubernetesProvider for KubeRsProvider {
        async fn state(&self) -> ProviderState {
            // A cheap, canonical reachability probe: the version endpoint
            // needs no RBAC beyond authenticated access.
            match self.client.request_text(Self::get("/version")).await {
                Ok(_) => ProviderState::Available,
                Err(e) => ProviderState::Degraded(e.to_string()),
            }
        }

        async fn pods(&self) -> Result<Vec<Pod>, KubernetesError> {
            crate::model::parse_pods(&self.get_json("/api/v1/pods").await?)
        }

        async fn nodes(&self) -> Result<Vec<Node>, KubernetesError> {
            crate::model::parse_nodes(&self.get_json("/api/v1/nodes").await?)
        }

        async fn deployments(&self) -> Result<Vec<Deployment>, KubernetesError> {
            crate::model::parse_deployments(&self.get_json("/apis/apps/v1/deployments").await?)
        }

        async fn events(&self) -> Result<Vec<KubeEvent>, KubernetesError> {
            crate::model::parse_events(&self.get_json("/api/v1/events").await?)
        }
    }
}
