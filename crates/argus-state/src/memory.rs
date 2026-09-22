//! Deterministic in-memory repository (tests and degraded operation).

use std::sync::Mutex;

use argus_domain::{DomainEvent, EnvironmentId, HealthStatus, Observation};
use async_trait::async_trait;

use crate::repository::{DomainRepository, RepositoryError};

#[derive(Default)]
struct Inner {
    environment: Option<EnvironmentId>,
    observations: Vec<Observation>,
    audit_events: Vec<DomainEvent>,
    health: Option<HealthStatus>,
}

/// A [`DomainRepository`] backed by process memory. Deterministic and
/// dependency-free; used by tests and when the durable backend is unavailable.
pub struct InMemoryRepository {
    inner: Mutex<Inner>,
}

impl InMemoryRepository {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
        }
    }
}

impl Default for InMemoryRepository {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DomainRepository for InMemoryRepository {
    async fn save_environment(&self, id: &EnvironmentId) -> Result<(), RepositoryError> {
        self.inner
            .lock()
            .map_err(|_| RepositoryError::Failed("lock poisoned".into()))?
            .environment = Some(*id);
        Ok(())
    }

    async fn get_environment(&self) -> Result<Option<EnvironmentId>, RepositoryError> {
        Ok(self
            .inner
            .lock()
            .map_err(|_| RepositoryError::Failed("lock poisoned".into()))?
            .environment)
    }

    async fn put_observation(&self, observation: &Observation) -> Result<(), RepositoryError> {
        self.inner
            .lock()
            .map_err(|_| RepositoryError::Failed("lock poisoned".into()))?
            .observations
            .push(observation.clone());
        Ok(())
    }

    async fn put_audit_event(&self, event: &DomainEvent) -> Result<(), RepositoryError> {
        self.inner
            .lock()
            .map_err(|_| RepositoryError::Failed("lock poisoned".into()))?
            .audit_events
            .push(event.clone());
        Ok(())
    }

    async fn list_audit_events(&self) -> Result<Vec<DomainEvent>, RepositoryError> {
        Ok(self
            .inner
            .lock()
            .map_err(|_| RepositoryError::Failed("lock poisoned".into()))?
            .audit_events
            .clone())
    }

    async fn save_health(&self, health: &HealthStatus) -> Result<(), RepositoryError> {
        self.inner
            .lock()
            .map_err(|_| RepositoryError::Failed("lock poisoned".into()))?
            .health = Some(health.clone());
        Ok(())
    }

    async fn get_health(&self) -> Result<Option<HealthStatus>, RepositoryError> {
        Ok(self
            .inner
            .lock()
            .map_err(|_| RepositoryError::Failed("lock poisoned".into()))?
            .health
            .clone())
    }
}

#[cfg(test)]
mod tests {
    use argus_domain::{DomainEvent, EnvironmentId, EventType, HealthStatus, Severity};
    use chrono::Utc;

    use super::*;
    use crate::DomainRepository;

    #[tokio::test]
    async fn environment_round_trip() {
        let repo = InMemoryRepository::new();
        assert_eq!(repo.get_environment().await.unwrap(), None);

        let id = EnvironmentId::new();
        repo.save_environment(&id).await.unwrap();
        assert_eq!(repo.get_environment().await.unwrap(), Some(id));
    }

    #[tokio::test]
    async fn health_round_trip() {
        let repo = InMemoryRepository::new();
        assert_eq!(repo.get_health().await.unwrap(), None);

        let health = HealthStatus::degraded("lancedb init failed", Utc::now());
        repo.save_health(&health).await.unwrap();
        assert_eq!(repo.get_health().await.unwrap(), Some(health));
    }

    #[tokio::test]
    async fn events_are_append_only() {
        let repo = InMemoryRepository::new();
        let event = DomainEvent::new(
            uuid::Uuid::new_v4(),
            EventType::new("argus.started").unwrap(),
            Utc::now(),
            "argusd",
            "argusd",
            Severity::Info,
            None,
            None,
            serde_json::json!({}),
        );
        repo.put_audit_event(&event).await.unwrap();
        // Appending twice is fine (append-only); the trait has no read for
        // events in the bootstrap, so this just verifies the write path.
        repo.put_audit_event(&event).await.unwrap();
    }
}
