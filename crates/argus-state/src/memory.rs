//! Deterministic in-memory repository (tests and degraded operation).

use std::sync::Mutex;

use argus_domain::{
    ActionEventFilter, ActionEventRecord, AutonomyState, BrainTraceRecord,
    DEFAULT_ACTION_EVENT_LIMIT, DomainEvent, EnvironmentId, Execution, HealthStatus, Hypothesis,
    Observation, Plan, TokenUsageRecord,
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
    episodes: Vec<(uuid::Uuid, argus_memory::Episode)>,
    facts: Vec<argus_memory::Fact>,
    procedures: Vec<(uuid::Uuid, argus_memory::ProcedureRecord)>,
    /// The action ledger (spec 007): append-only, insertion order.
    action_events: Vec<ActionEventRecord>,
    brain_traces: Vec<BrainTraceRecord>,
    token_usage: Vec<TokenUsageRecord>,
    /// The graduated-autonomy state (spec 008): one row per environment,
    /// re-put supersedes.
    autonomy: Option<AutonomyState>,
    /// Delivered runbooks (spec 009): keyed by name, re-put supersedes.
    runbooks: Vec<argus_runbooks::DeliveredRunbook>,
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

    async fn put_episode(
        &self,
        id: uuid::Uuid,
        episode: &argus_memory::Episode,
    ) -> Result<(), RepositoryError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| RepositoryError::Failed("lock poisoned".into()))?;
        // Idempotent on id, mirroring EpisodicMemory::record.
        inner.episodes.retain(|(existing, _)| *existing != id);
        inner.episodes.push((id, episode.clone()));
        Ok(())
    }

    async fn list_episodes(
        &self,
    ) -> Result<Vec<(uuid::Uuid, argus_memory::Episode)>, RepositoryError> {
        Ok(self
            .inner
            .lock()
            .map_err(|_| RepositoryError::Failed("lock poisoned".into()))?
            .episodes
            .clone())
    }

    async fn put_fact(&self, fact: &argus_memory::Fact) -> Result<(), RepositoryError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| RepositoryError::Failed("lock poisoned".into()))?;
        // Supersede on (subject, attribute), mirroring add_fact; the
        // superseding fact moves to the end of insertion order.
        inner
            .facts
            .retain(|f| !(f.subject == fact.subject && f.attribute == fact.attribute));
        inner.facts.push(fact.clone());
        Ok(())
    }

    async fn list_facts(&self) -> Result<Vec<argus_memory::Fact>, RepositoryError> {
        Ok(self
            .inner
            .lock()
            .map_err(|_| RepositoryError::Failed("lock poisoned".into()))?
            .facts
            .clone())
    }

    async fn put_procedure(
        &self,
        id: uuid::Uuid,
        procedure: &argus_memory::ProcedureRecord,
    ) -> Result<(), RepositoryError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| RepositoryError::Failed("lock poisoned".into()))?;
        inner.procedures.retain(|(existing, _)| *existing != id);
        inner.procedures.push((id, procedure.clone()));
        Ok(())
    }

    async fn list_procedures(
        &self,
    ) -> Result<Vec<(uuid::Uuid, argus_memory::ProcedureRecord)>, RepositoryError> {
        Ok(self
            .inner
            .lock()
            .map_err(|_| RepositoryError::Failed("lock poisoned".into()))?
            .procedures
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

    async fn put_action_event(&self, event: &ActionEventRecord) -> Result<(), RepositoryError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| RepositoryError::Failed("lock poisoned".into()))?;
        // Append-only: an id already present is left as first written.
        if !inner
            .action_events
            .iter()
            .any(|e| e.event_id == event.event_id)
        {
            inner.action_events.push(event.clone());
        }
        Ok(())
    }

    async fn list_action_events(
        &self,
        filter: &ActionEventFilter,
    ) -> Result<Vec<ActionEventRecord>, RepositoryError> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| RepositoryError::Failed("lock poisoned".into()))?;
        let limit = if filter.limit == 0 {
            DEFAULT_ACTION_EVENT_LIMIT
        } else {
            filter.limit
        };
        let mut rows: Vec<ActionEventRecord> = inner
            .action_events
            .iter()
            .filter(|event| match &filter.since {
                Some(since) => event.occurred_at >= *since,
                None => true,
            })
            .filter(|event| match &filter.kind {
                Some(kind) => &event.kind == kind,
                None => true,
            })
            .filter(|event| match &filter.status {
                Some(status) => &event.outcome == status,
                None => true,
            })
            .cloned()
            .collect();
        // Newest first, mirroring the SQLite ordering.
        rows.sort_by_key(|row| std::cmp::Reverse(row.occurred_at));
        rows.truncate(limit);
        Ok(rows)
    }

    async fn put_brain_trace(&self, trace: &BrainTraceRecord) -> Result<(), RepositoryError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| RepositoryError::Failed("lock poisoned".into()))?;
        if !inner
            .brain_traces
            .iter()
            .any(|t| t.trace_id == trace.trace_id)
        {
            inner.brain_traces.push(trace.clone());
        }
        Ok(())
    }

    async fn list_brain_traces(
        &self,
        cycle_id: Option<uuid::Uuid>,
        limit: usize,
    ) -> Result<Vec<BrainTraceRecord>, RepositoryError> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| RepositoryError::Failed("lock poisoned".into()))?;
        let limit = if limit == 0 {
            DEFAULT_ACTION_EVENT_LIMIT
        } else {
            limit
        };
        let mut rows: Vec<BrainTraceRecord> = inner
            .brain_traces
            .iter()
            .filter(|trace| trace.cycle_id == cycle_id || cycle_id.is_none())
            .cloned()
            .collect();
        rows.sort_by_key(|row| std::cmp::Reverse(row.occurred_at));
        rows.truncate(limit);
        Ok(rows)
    }

    async fn put_token_usage(&self, usage: &TokenUsageRecord) -> Result<(), RepositoryError> {
        self.inner
            .lock()
            .map_err(|_| RepositoryError::Failed("lock poisoned".into()))?
            .token_usage
            .push(usage.clone());
        Ok(())
    }

    async fn list_token_usage(
        &self,
        limit: usize,
    ) -> Result<Vec<TokenUsageRecord>, RepositoryError> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| RepositoryError::Failed("lock poisoned".into()))?;
        let limit = if limit == 0 {
            DEFAULT_ACTION_EVENT_LIMIT
        } else {
            limit
        };
        let mut rows = inner.token_usage.clone();
        rows.sort_by_key(|row| std::cmp::Reverse(row.occurred_at));
        rows.truncate(limit);
        Ok(rows)
    }

    async fn retain_ledger(&self, days: i64) -> Result<(), RepositoryError> {
        let cutoff = chrono::Utc::now() - chrono::Duration::days(days.max(0));
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| RepositoryError::Failed("lock poisoned".into()))?;
        inner.action_events.retain(|e| e.occurred_at >= cutoff);
        inner.brain_traces.retain(|t| t.occurred_at >= cutoff);
        inner.token_usage.retain(|u| u.occurred_at >= cutoff);
        Ok(())
    }

    async fn put_autonomy_state(&self, state: &AutonomyState) -> Result<(), RepositoryError> {
        self.inner
            .lock()
            .map_err(|_| RepositoryError::Failed("lock poisoned".into()))?
            .autonomy = Some(state.clone());
        Ok(())
    }

    async fn get_autonomy_state(&self) -> Result<Option<AutonomyState>, RepositoryError> {
        Ok(self
            .inner
            .lock()
            .map_err(|_| RepositoryError::Failed("lock poisoned".into()))?
            .autonomy
            .clone())
    }

    async fn put_runbook(
        &self,
        runbook: &argus_runbooks::DeliveredRunbook,
    ) -> Result<(), RepositoryError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| RepositoryError::Failed("lock poisoned".into()))?;
        inner
            .runbooks
            .retain(|existing| existing.name() != runbook.name());
        inner.runbooks.push(runbook.clone());
        inner.runbooks.sort_by(|a, b| a.name().cmp(b.name()));
        Ok(())
    }

    async fn list_runbooks(
        &self,
    ) -> Result<Vec<argus_runbooks::DeliveredRunbook>, RepositoryError> {
        Ok(self
            .inner
            .lock()
            .map_err(|_| RepositoryError::Failed("lock poisoned".into()))?
            .runbooks
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

    #[tokio::test]
    async fn the_ledger_round_trips_append_only() {
        use argus_domain::{
            ActionEventFilter, ActionEventRecord, BrainTraceRecord, TokenUsageRecord,
        };

        let repo = InMemoryRepository::new();
        let event = ActionEventRecord {
            event_id: uuid::Uuid::new_v4(),
            correlation_id: uuid::Uuid::new_v4(),
            causation_id: None,
            cycle_id: None,
            plan_id: Some(uuid::Uuid::new_v4()),
            kind: "host.service.restart".into(),
            target: Some("nginx.service".into()),
            args: serde_json::json!({ "unit": "nginx.service" }),
            verdict: "allow".into(),
            policy_id: None,
            outcome: "ok".into(),
            duration_ms: Some(5),
            validation: None,
            occurred_at: chrono::Utc::now(),
        };
        repo.put_action_event(&event).await.unwrap();
        repo.put_action_event(&event).await.unwrap();
        let listed = repo
            .list_action_events(&ActionEventFilter::default())
            .await
            .unwrap();
        assert_eq!(
            listed,
            vec![event.clone()],
            "append-only: one row per event id"
        );

        let by_status = repo
            .list_action_events(&ActionEventFilter {
                status: Some("denied".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(by_status.is_empty(), "the status filter narrows");

        let trace = BrainTraceRecord {
            trace_id: uuid::Uuid::new_v4(),
            cycle_id: Some(uuid::Uuid::new_v4()),
            plan_id: None,
            evidence: vec![],
            decision: None,
            objective: Some("restore nginx".into()),
            steps: vec![],
            outcome: Some("Completed".into()),
            occurred_at: chrono::Utc::now(),
        };
        repo.put_brain_trace(&trace).await.unwrap();
        assert_eq!(
            repo.list_brain_traces(trace.cycle_id, 10).await.unwrap(),
            vec![trace]
        );

        let usage = TokenUsageRecord {
            cycle_id: None,
            model: Some("deepseek-chat".into()),
            prompt_tokens: Some(10),
            completion_tokens: None,
            total_tokens: None,
            duration_ms: Some(100),
            occurred_at: chrono::Utc::now(),
        };
        repo.put_token_usage(&usage).await.unwrap();
        assert_eq!(repo.list_token_usage(10).await.unwrap(), vec![usage]);

        repo.retain_ledger(30).await.unwrap();
        assert!(
            !repo
                .list_action_events(&ActionEventFilter::default())
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn the_autonomy_state_supersedes_its_row() {
        let repo = InMemoryRepository::new();
        assert_eq!(repo.get_autonomy_state().await.unwrap(), None);

        let mut state = argus_domain::AutonomyState::fresh(EnvironmentId::new(), Utc::now());
        repo.put_autonomy_state(&state).await.unwrap();
        assert_eq!(
            repo.get_autonomy_state().await.unwrap(),
            Some(state.clone())
        );

        state.earned = argus_domain::AutonomyMode::L2Recommend;
        state.rung_clean_cycles = 3;
        repo.put_autonomy_state(&state).await.unwrap();
        assert_eq!(
            repo.get_autonomy_state().await.unwrap(),
            Some(state),
            "one row per environment: the re-put supersedes"
        );
    }

    #[tokio::test]
    async fn the_delivered_runbooks_supersede_their_row() {
        let repo = InMemoryRepository::new();
        assert!(repo.list_runbooks().await.unwrap().is_empty());

        let delivered = argus_runbooks::DeliveredRunbook {
            runbook: argus_runbooks::Runbook::candidate(
                uuid::Uuid::new_v4(),
                "learned-procedure",
                argus_runbooks::RunbookTrigger::Symptom("restart-loop".into()),
                vec![],
                vec![],
                vec![],
                vec![],
                vec![],
                vec![],
            ),
            configuration_id: uuid::Uuid::new_v4(),
            version_id: uuid::Uuid::new_v4(),
            version_number: 2,
            delivered_at: chrono::Utc::now(),
        };
        repo.put_runbook(&delivered).await.unwrap();
        assert_eq!(repo.list_runbooks().await.unwrap(), vec![delivered.clone()]);

        let mut superseding = delivered.clone();
        superseding.version_number = 3;
        repo.put_runbook(&superseding).await.unwrap();
        let listed = repo.list_runbooks().await.unwrap();
        assert_eq!(listed, vec![superseding], "re-put supersedes by name");
    }
}
