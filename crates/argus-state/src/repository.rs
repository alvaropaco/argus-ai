//! The repository abstraction for operational state.

use argus_domain::{
    AppliedConfigurationState, CapabilityPublication, CloudCommand, CloudConnection,
    CloudEnrollment, DomainEvent, EnvironmentId, ExecutionApproval, ExecutionDecision,
    HealthStatus, ManagedConfiguration, Observation, ReportBuffer,
};
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
