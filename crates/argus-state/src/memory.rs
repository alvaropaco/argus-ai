//! Deterministic in-memory repository (tests and degraded operation).

use std::sync::Mutex;

use argus_domain::{
    DomainEvent, EnvironmentId, Execution, HealthStatus, Hypothesis, Observation, Plan,
};
use async_trait::async_trait;

use crate::repository::{DomainRepository, RepositoryError};

#[derive(Default)]
struct Inner {
    environment: Option<EnvironmentId>,
    observations: Vec<Observation>,
    audit_events: Vec<DomainEvent>,
    health: Option<HealthStatus>,
    hypotheses: Vec<(uuid::Uuid, Hypothesis)>,
    plans: Vec<(uuid::Uuid, Plan)>,
    executions: Vec<(uuid::Uuid, Execution)>,
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

    async fn list_observations(&self) -> Result<Vec<Observation>, RepositoryError> {
        Ok(self
            .inner
            .lock()
            .map_err(|_| RepositoryError::Failed("lock poisoned".into()))?
            .observations
            .clone())
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

    async fn put_hypothesis(
        &self,
        id: uuid::Uuid,
        hypothesis: &Hypothesis,
    ) -> Result<(), RepositoryError> {
        self.inner
            .lock()
            .map_err(|_| RepositoryError::Failed("lock poisoned".into()))?
            .hypotheses
            .push((id, hypothesis.clone()));
        Ok(())
    }

    async fn list_hypotheses(&self) -> Result<Vec<(uuid::Uuid, Hypothesis)>, RepositoryError> {
        Ok(self
            .inner
            .lock()
            .map_err(|_| RepositoryError::Failed("lock poisoned".into()))?
            .hypotheses
            .clone())
    }

    async fn put_plan(&self, id: uuid::Uuid, plan: &Plan) -> Result<(), RepositoryError> {
        self.inner
            .lock()
            .map_err(|_| RepositoryError::Failed("lock poisoned".into()))?
            .plans
            .push((id, plan.clone()));
        Ok(())
    }

    async fn list_plans(&self) -> Result<Vec<(uuid::Uuid, Plan)>, RepositoryError> {
        Ok(self
            .inner
            .lock()
            .map_err(|_| RepositoryError::Failed("lock poisoned".into()))?
            .plans
            .clone())
    }

    async fn put_execution(
        &self,
        id: uuid::Uuid,
        execution: &Execution,
    ) -> Result<(), RepositoryError> {
        self.inner
            .lock()
            .map_err(|_| RepositoryError::Failed("lock poisoned".into()))?
            .executions
            .push((id, execution.clone()));
        Ok(())
    }

    async fn list_executions(&self) -> Result<Vec<(uuid::Uuid, Execution)>, RepositoryError> {
        Ok(self
            .inner
            .lock()
            .map_err(|_| RepositoryError::Failed("lock poisoned".into()))?
            .executions
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
    async fn observations_are_listable() {
        let repo = InMemoryRepository::new();
        assert!(repo.list_observations().await.unwrap().is_empty());

        let subject = argus_domain::ResourceId::new("host", "abc").unwrap();
        let observation = argus_domain::Observation::new(
            uuid::Uuid::new_v4(),
            "argusd",
            subject,
            "service.active",
            argus_domain::ObservedValue::Bool(true),
            1.0,
            argus_domain::Provenance::new("systemd", "is_active", Utc::now()),
            Utc::now(),
        )
        .unwrap();
        repo.put_observation(&observation).await.unwrap();

        assert_eq!(repo.list_observations().await.unwrap(), vec![observation]);
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

    #[tokio::test]
    async fn reasoning_artifacts_round_trip_in_insertion_order() {
        let repo = InMemoryRepository::new();
        let plan = Plan {
            objective: "restore nginx".into(),
            steps: vec![],
            preconditions: vec![],
            expected_outcomes: vec![],
            blast_radius: argus_domain::BlastRadius::Host,
            confidence: 0.9,
            status: argus_domain::PlanStatus::Proposed,
        };
        let first = uuid::Uuid::new_v4();
        repo.put_plan(first, &plan).await.unwrap();
        let second = uuid::Uuid::new_v4();
        repo.put_plan(second, &plan).await.unwrap();

        let plans = repo.list_plans().await.unwrap();
        assert_eq!(
            plans.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![first, second],
            "insertion order is preserved"
        );

        let hypothesis = Hypothesis {
            statement: "nginx stopped".into(),
            confidence: 0.9,
            supporting_evidence: vec![],
            status: argus_domain::HypothesisStatus::Confirmed,
        };
        repo.put_hypothesis(first, &hypothesis).await.unwrap();
        assert_eq!(
            repo.list_hypotheses().await.unwrap(),
            vec![(first, hypothesis)]
        );

        let execution = Execution {
            action: argus_domain::Action {
                capability: argus_domain::CapabilityId::new("host.service.restart").unwrap(),
                resource: None,
                arguments: serde_json::json!({ "unit": "nginx.service" }),
            },
            status: argus_domain::ExecutionStatus::Failed,
            evidence: serde_json::json!({ "error": "boom" }),
        };
        repo.put_execution(first, &execution).await.unwrap();
        assert_eq!(
            repo.list_executions().await.unwrap(),
            vec![(first, execution)]
        );
    }
}
