//! The repository abstraction for operational state.

use argus_domain::{DomainEvent, EnvironmentId, HealthStatus, Observation};
use async_trait::async_trait;

/// Errors from the persistence boundary.
#[derive(Debug, thiserror::Error)]
pub enum RepositoryError {
    #[error("repository unavailable: {0}")]
    Unavailable(String),

    #[error("repository operation failed: {0}")]
    Failed(String),

    #[error("stored data is corrupt: {0}")]
    Corrupt(String),
}

/// Persistence boundary for operational state.
///
/// Implementations MUST NOT leak their concrete types (LanceDB, etc.) into the
/// domain model; this trait is the only surface the domain sees.
#[async_trait]
pub trait DomainRepository: Send + Sync {
    async fn save_environment(&self, id: &EnvironmentId) -> Result<(), RepositoryError>;
    async fn get_environment(&self) -> Result<Option<EnvironmentId>, RepositoryError>;

    async fn put_observation(&self, observation: &Observation) -> Result<(), RepositoryError>;

    async fn put_audit_event(&self, event: &DomainEvent) -> Result<(), RepositoryError>;

    async fn save_health(&self, health: &HealthStatus) -> Result<(), RepositoryError>;
    async fn get_health(&self) -> Result<Option<HealthStatus>, RepositoryError>;
}
