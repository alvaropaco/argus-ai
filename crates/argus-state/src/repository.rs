//! The repository abstraction for operational state.

use argus_domain::{
    ActionEventFilter, ActionEventRecord, AppliedConfigurationState, BrainTraceRecord,
    CapabilityPublication, CloudCommand, CloudConnection, CloudEnrollment, DomainEvent,
    EnvironmentId, Execution, ExecutionApproval, ExecutionDecision, HealthStatus, Hypothesis,
    ManagedConfiguration, Observation, Plan, ReportBuffer, TokenUsageRecord,
};
use argus_memory::{Episode, Fact, ProcedureRecord};
use async_trait::async_trait;
use uuid::Uuid;

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

    /// The observations recorded so far, in insertion order.
    ///
    /// The read is what lets the validator and the learning pass read persisted
    /// evidence (ADR-0031 §3); `put_observation` alone made observations
    /// write-only.
    async fn list_observations(&self) -> Result<Vec<Observation>, RepositoryError>;

    async fn put_audit_event(&self, event: &DomainEvent) -> Result<(), RepositoryError>;

    /// The audit events recorded so far.
    ///
    /// The read exists so that an audit write can be observed rather than assumed,
    /// which matters for the cloud-issued invocation path: its whole guarantee is
    /// that a cloud-driven action leaves the same trace as a local one (FR-042).
    async fn list_audit_events(&self) -> Result<Vec<DomainEvent>, RepositoryError> {
        Err(RepositoryError::Failed(
            "list_audit_events is not supported by this repository".to_string(),
        ))
    }

    /// Persists one reasoning artifact — a hypothesis, a plan, or an execution —
    /// keyed by the correlation id of the reasoning step that produced it
    /// (FR-008). The audit trail stays the append-only narrative; these records
    /// are the queryable reasoning history behind it.
    ///
    /// The reasoning entities (`Hypothesis`, `Plan`, `Execution`) carry no
    /// identity of their own, so the correlation id is the key and the lists
    /// return `(id, artifact)` pairs in insertion order.
    async fn put_hypothesis(
        &self,
        _id: Uuid,
        _hypothesis: &Hypothesis,
    ) -> Result<(), RepositoryError> {
        Err(reasoning_unsupported("put_hypothesis"))
    }

    async fn list_hypotheses(&self) -> Result<Vec<(Uuid, Hypothesis)>, RepositoryError> {
        Err(reasoning_unsupported("list_hypotheses"))
    }

    async fn put_plan(&self, _id: Uuid, _plan: &Plan) -> Result<(), RepositoryError> {
        Err(reasoning_unsupported("put_plan"))
    }

    async fn list_plans(&self) -> Result<Vec<(Uuid, Plan)>, RepositoryError> {
        Err(reasoning_unsupported("list_plans"))
    }

    async fn put_execution(
        &self,
        _id: Uuid,
        _execution: &Execution,
    ) -> Result<(), RepositoryError> {
        Err(reasoning_unsupported("put_execution"))
    }

    async fn list_executions(&self) -> Result<Vec<(Uuid, Execution)>, RepositoryError> {
        Err(reasoning_unsupported("list_executions"))
    }

    /// Persists one episodic-memory record (ADR-0038 §3: every layer
    /// persists through this abstraction). Re-putting an id replaces the
    /// episode, mirroring `EpisodicMemory::record`'s idempotence.
    async fn put_episode(&self, _id: Uuid, _episode: &Episode) -> Result<(), RepositoryError> {
        Err(memory_unsupported("put_episode"))
    }

    async fn list_episodes(&self) -> Result<Vec<(Uuid, Episode)>, RepositoryError> {
        Err(memory_unsupported("list_episodes"))
    }

    /// Persists one semantic-memory fact. Facts key on
    /// `(subject, attribute)` — re-putting the same pair supersedes, the
    /// same supersede semantics as `SemanticMemory::add_fact`.
    async fn put_fact(&self, _fact: &Fact) -> Result<(), RepositoryError> {
        Err(memory_unsupported("put_fact"))
    }

    async fn list_facts(&self) -> Result<Vec<Fact>, RepositoryError> {
        Err(memory_unsupported("list_facts"))
    }

    /// Persists one procedural-memory record (attempt history included).
    async fn put_procedure(
        &self,
        _id: Uuid,
        _procedure: &ProcedureRecord,
    ) -> Result<(), RepositoryError> {
        Err(memory_unsupported("put_procedure"))
    }

    async fn list_procedures(&self) -> Result<Vec<(Uuid, ProcedureRecord)>, RepositoryError> {
        Err(memory_unsupported("list_procedures"))
    }

    async fn save_health(&self, health: &HealthStatus) -> Result<(), RepositoryError>;
    async fn get_health(&self) -> Result<Option<HealthStatus>, RepositoryError>;

    /// Persists the installation's enrollment, replacing any previous one.
    async fn save_cloud_enrollment(
        &self,
        _enrollment: &CloudEnrollment,
    ) -> Result<(), RepositoryError> {
        Err(cloud_state_unsupported("save_cloud_enrollment"))
    }

    /// The current enrollment, or `None` when the installation is not enrolled.
    async fn get_cloud_enrollment(&self) -> Result<Option<CloudEnrollment>, RepositoryError> {
        Err(cloud_state_unsupported("get_cloud_enrollment"))
    }

    /// Removes the enrollment and every cloud-derived record.
    ///
    /// Backs `cloud.forget`: after this the installation is indistinguishable
    /// from one that was never enrolled, apart from its local operational state.
    async fn clear_cloud_state(&self) -> Result<(), RepositoryError> {
        Err(cloud_state_unsupported("clear_cloud_state"))
    }

    /// Persists the last observed connection summary.
    async fn save_cloud_connection(
        &self,
        _connection: &CloudConnection,
    ) -> Result<(), RepositoryError> {
        Err(cloud_state_unsupported("save_cloud_connection"))
    }

    async fn get_cloud_connection(&self) -> Result<Option<CloudConnection>, RepositoryError> {
        Err(cloud_state_unsupported("get_cloud_connection"))
    }

    /// Persists a command, including its status, so re-delivery and crash
    /// recovery can be decided without re-executing anything.
    async fn put_cloud_command(&self, _command: &CloudCommand) -> Result<(), RepositoryError> {
        Err(cloud_state_unsupported("put_cloud_command"))
    }

    async fn get_cloud_command(
        &self,
        _command_id: Uuid,
    ) -> Result<Option<CloudCommand>, RepositoryError> {
        Err(cloud_state_unsupported("get_cloud_command"))
    }

    /// Commands still in a non-terminal status.
    ///
    /// Anything returned here was interrupted by a crash and MUST be resolved to
    /// a reported unknown state on startup (ADR-0022 §4).
    async fn list_unfinished_cloud_commands(&self) -> Result<Vec<CloudCommand>, RepositoryError> {
        Err(cloud_state_unsupported("list_unfinished_cloud_commands"))
    }

    /// Keeps only the newest `keep` commands, bounding history growth.
    async fn retain_cloud_commands(&self, _keep: usize) -> Result<(), RepositoryError> {
        Err(cloud_state_unsupported("retain_cloud_commands"))
    }

    async fn put_execution_decision(
        &self,
        _decision: &ExecutionDecision,
    ) -> Result<(), RepositoryError> {
        Err(cloud_state_unsupported("put_execution_decision"))
    }

    /// The authorization verdicts recorded for cloud-issued invocations.
    ///
    /// The read lets a test assert the structural guarantee rather than trust it:
    /// every invocation that reaches the executor carries a permitting decision
    /// (SC-016).
    async fn list_execution_decisions(&self) -> Result<Vec<ExecutionDecision>, RepositoryError> {
        Err(cloud_state_unsupported("list_execution_decisions"))
    }

    async fn put_execution_approval(
        &self,
        _approval: &ExecutionApproval,
    ) -> Result<(), RepositoryError> {
        Err(cloud_state_unsupported("put_execution_approval"))
    }

    /// The approval granted for an invocation, if any.
    async fn get_execution_approval(
        &self,
        _command_id: Uuid,
    ) -> Result<Option<ExecutionApproval>, RepositoryError> {
        Err(cloud_state_unsupported("get_execution_approval"))
    }

    async fn save_capability_publication(
        &self,
        _publication: &CapabilityPublication,
    ) -> Result<(), RepositoryError> {
        Err(cloud_state_unsupported("save_capability_publication"))
    }

    /// The most recently published capability surface.
    async fn get_capability_publication(
        &self,
    ) -> Result<Option<CapabilityPublication>, RepositoryError> {
        Err(cloud_state_unsupported("get_capability_publication"))
    }

    async fn save_managed_configuration(
        &self,
        _configuration: &ManagedConfiguration,
    ) -> Result<(), RepositoryError> {
        Err(cloud_state_unsupported("save_managed_configuration"))
    }

    async fn get_managed_configuration(
        &self,
        _configuration_id: Uuid,
    ) -> Result<Option<ManagedConfiguration>, RepositoryError> {
        Err(cloud_state_unsupported("get_managed_configuration"))
    }

    async fn save_applied_configuration(
        &self,
        _state: &AppliedConfigurationState,
    ) -> Result<(), RepositoryError> {
        Err(cloud_state_unsupported("save_applied_configuration"))
    }

    /// Everything currently applied, for reconciliation with the cloud.
    async fn list_applied_configurations(
        &self,
    ) -> Result<Vec<AppliedConfigurationState>, RepositoryError> {
        Err(cloud_state_unsupported("list_applied_configurations"))
    }

    async fn save_report_buffer(&self, _buffer: &ReportBuffer) -> Result<(), RepositoryError> {
        Err(cloud_state_unsupported("save_report_buffer"))
    }

    async fn get_report_buffer(&self) -> Result<Option<ReportBuffer>, RepositoryError> {
        Err(cloud_state_unsupported("get_report_buffer"))
    }

    // --- Action ledger (spec 007): the local source of truth ---

    /// Appends one execution attempt at full local fidelity (FR-001).
    /// Append-only: re-putting an event id is a no-op, never a rewrite.
    async fn put_action_event(&self, _event: &ActionEventRecord) -> Result<(), RepositoryError> {
        Err(ledger_unsupported("put_action_event"))
    }

    /// Ledger rows newest-first, narrowed by the filter (`since`/`kind`/`status`).
    async fn list_action_events(
        &self,
        _filter: &ActionEventFilter,
    ) -> Result<Vec<ActionEventRecord>, RepositoryError> {
        Err(ledger_unsupported("list_action_events"))
    }

    /// Appends one bounded trace (FR-002). Append-only on `trace_id`.
    async fn put_brain_trace(&self, _trace: &BrainTraceRecord) -> Result<(), RepositoryError> {
        Err(ledger_unsupported("put_brain_trace"))
    }

    /// Traces newest-first, optionally narrowed to one cycle.
    async fn list_brain_traces(
        &self,
        _cycle_id: Option<Uuid>,
        _limit: usize,
    ) -> Result<Vec<BrainTraceRecord>, RepositoryError> {
        Err(ledger_unsupported("list_brain_traces"))
    }

    /// Appends one token-usage record (FR-003). Missing counts are unknown.
    async fn put_token_usage(&self, _usage: &TokenUsageRecord) -> Result<(), RepositoryError> {
        Err(ledger_unsupported("put_token_usage"))
    }

    /// Usage records newest-first, up to `limit`.
    async fn list_token_usage(
        &self,
        _limit: usize,
    ) -> Result<Vec<TokenUsageRecord>, RepositoryError> {
        Err(ledger_unsupported("list_token_usage"))
    }

    /// Bounded growth (spec 007 NFR): drops ledger rows older than `days`.
    async fn retain_ledger(&self, _days: i64) -> Result<(), RepositoryError> {
        Err(ledger_unsupported("retain_ledger"))
    }
}

const CLOUD_STATE: &str = "cloud state";

/// Cloud state is optional per backend, so a missing implementation must surface
/// as a clear error rather than a silent no-op that would lose an enrollment or
/// an interrupted command.
fn cloud_state_unsupported(operation: &'static str) -> RepositoryError {
    RepositoryError::Unavailable(format!(
        "{CLOUD_STATE} is not supported by this backend (operation: {operation})"
    ))
}

/// Reasoning persistence is optional per backend for the same reason: a backend
/// that cannot store plans must say so rather than silently drop the reasoning
/// history an operator is entitled to review (FR-008).
fn reasoning_unsupported(operation: &'static str) -> RepositoryError {
    RepositoryError::Unavailable(format!(
        "reasoning state is not supported by this backend (operation: {operation})"
    ))
}

/// Memory persistence (spec 004 FR-001) follows the same rule: a backend
/// that cannot store the memory layers fails loudly instead of letting the
/// runtime silently forget (ADR-0038 §3).
fn memory_unsupported(operation: &'static str) -> RepositoryError {
    RepositoryError::Unavailable(format!(
        "memory state is not supported by this backend (operation: {operation})"
    ))
}

/// The action ledger (spec 007) follows the same rule: a backend that cannot
/// persist it must say so — a silently dropped ledger row would be an audit
/// hole an operator is entitled to know about.
fn ledger_unsupported(operation: &'static str) -> RepositoryError {
    RepositoryError::Unavailable(format!(
        "action ledger is not supported by this backend (operation: {operation})"
    ))
}
