//! In-memory runtime state for the bootstrap daemon.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use argus_domain::{
    AuthorizationRequest, BlastRadius, CapabilityDescriptor, CapabilityId, CapabilityRegistry,
    CapabilityRequest, EnvironmentId, HealthStatus, PluginManifest, PolicyOutcome,
    PrivilegeDeclaration, Reversibility, RiskClass,
};
use argus_executor::{BootstrapExecutor, CapabilityProvider, ExecutionError, Executor};
use argus_policy::{BootstrapPolicyEvaluator, PolicyEvaluator};
use argus_state::{DomainRepository, RepositoryError, SqliteRepository};
use chrono::Utc;
use semver::Version;
use serde_json::Value;

use argus_cloud::buffer::ReportQueue;
use argus_cloud::state::ConnectivityTracker;

use crate::cloud::{CloudSecretStore, ManagedSettingsStore as CloudSettingsStore};
use crate::config::DaemonConfig;

/// The running daemon and its bootstrap state.
pub struct Daemon {
    started_at: Instant,
    environment_id: EnvironmentId,
    config: DaemonConfig,
    repository: Arc<dyn DomainRepository>,
    secrets: CloudSecretStore,
    tracker: Arc<tokio::sync::Mutex<ConnectivityTracker>>,
    queue: Arc<tokio::sync::Mutex<ReportQueue>>,
    managed_settings: CloudSettingsStore,
    policy: BootstrapPolicyEvaluator,
    executor: BootstrapExecutor,
    registry: CapabilityRegistry,
    plugins: Vec<PluginManifest>,
    /// The local kill switch, shared with the cloud supervisor so that changing it
    /// takes effect without a restart.
    privileged_execution: Arc<AtomicBool>,
}

/// Errors from the capability dispatch boundary.
#[derive(Debug, thiserror::Error)]
pub enum DispatchError {
    #[error("capability denied by policy: {0:?}")]
    Denied(PolicyOutcome),

    #[error("execution failed: {0}")]
    Execution(#[from] ExecutionError),
}

struct DaemonProvider {
    config: DaemonConfig,
}

impl CapabilityProvider for DaemonProvider {
    fn health(&self) -> HealthStatus {
        HealthStatus::ready(Utc::now())
    }

    fn config(&self) -> Value {
        serde_json::to_value(&self.config).unwrap_or(Value::Null)
    }

    fn plugins(&self) -> Value {
        serde_json::json!([])
    }

    fn host_status(&self) -> Value {
        serde_json::json!({ "status": "unavailable" })
    }

    fn list_capabilities(&self) -> Vec<CapabilityId> {
        bootstrap_capabilities()
    }
}

impl Daemon {
    /// Initializes the daemon, opening the state store and persisting/loading
    /// environment identity and an initial health record.
    pub async fn init(config: DaemonConfig) -> Result<Self, RepositoryError> {
        let repository: Arc<dyn DomainRepository> =
            Arc::new(SqliteRepository::open(&config.state_path)?);

        let environment_id = match repository.get_environment().await? {
            Some(id) => id,
            None => {
                let id = EnvironmentId::new();
                repository.save_environment(&id).await?;
                id
            }
        };
        repository
            .save_health(&HealthStatus::ready(Utc::now()))
            .await?;

        let provider = Arc::new(DaemonProvider {
            config: config.clone(),
        });

        let mut registry = CapabilityRegistry::with_sandbox(PrivilegeDeclaration::none());
        for descriptor in bootstrap_descriptors() {
            registry
                .register(descriptor)
                .expect("bootstrap descriptors are unique");
        }

        let buffer_capacity = config.cloud.report_buffer_max_records;
        let privileged_execution =
            Arc::new(AtomicBool::new(config.cloud.allow_privileged_execution));

        Ok(Self {
            started_at: Instant::now(),
            environment_id,
            config,
            repository,
            secrets: CloudSecretStore::default_location(),
            tracker: Arc::new(tokio::sync::Mutex::new(ConnectivityTracker::new())),
            queue: Arc::new(tokio::sync::Mutex::new(ReportQueue::new(buffer_capacity))),
            managed_settings: CloudSettingsStore::default_location(),
            policy: BootstrapPolicyEvaluator::new(),
            executor: BootstrapExecutor::new(provider),
            registry,
            plugins: Vec::new(),
            privileged_execution,
        })
    }

    pub fn config(&self) -> &DaemonConfig {
        &self.config
    }

    pub fn secrets(&self) -> &CloudSecretStore {
        &self.secrets
    }

    /// The shared cloud connectivity view, read by IPC and written by the
    /// supervisor.
    pub fn tracker(&self) -> Arc<tokio::sync::Mutex<ConnectivityTracker>> {
        Arc::clone(&self.tracker)
    }

    /// The shared report queue, so IPC can report its occupancy.
    pub fn report_queue(&self) -> Arc<tokio::sync::Mutex<ReportQueue>> {
        Arc::clone(&self.queue)
    }

    pub fn managed_settings(&self) -> &CloudSettingsStore {
        &self.managed_settings
    }

    pub fn environment_id(&self) -> EnvironmentId {
        self.environment_id
    }

    pub fn repository(&self) -> &Arc<dyn DomainRepository> {
        &self.repository
    }

    pub fn health(&self) -> HealthStatus {
        HealthStatus::ready(Utc::now())
    }

    pub fn status(&self) -> Value {
        serde_json::json!({
            "health": self.health(),
            "environment_id": self.environment_id.as_uuid().to_string(),
            "uptime_seconds": self.started_at.elapsed().as_secs(),
            "plugin_count": 0,
        })
    }

    pub fn plugins(&self) -> Value {
        serde_json::json!(
            self.plugins
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>()
        )
    }

    pub fn capabilities(&self) -> Value {
        serde_json::json!(
            self.registry
                .list()
                .map(|d| d.id().as_str())
                .collect::<Vec<_>>()
        )
    }

    pub fn registry(&self) -> &CapabilityRegistry {
        &self.registry
    }

    /// The executor the cloud channel routes invocations to.
    ///
    /// It covers both published surfaces: read-only capabilities through the
    /// bootstrap provider, and environment-changing ones through the privileged
    /// path. Both remain behind the single `AuthorizedAction` gate.
    pub fn cloud_executor(&self) -> Arc<dyn Executor> {
        Arc::new(argus_executor::CompositeExecutor::new(
            Arc::new(BootstrapExecutor::new(Arc::new(DaemonProvider {
                config: self.config.clone(),
            }))),
            Arc::new(argus_executor::PrivilegedExecutor::new(Arc::new(
                argus_executor::SystemdServiceController::new(),
            ))),
        ))
    }

    /// The policy evaluator the cloud channel routes through.
    pub fn cloud_policy(&self) -> Arc<dyn PolicyEvaluator> {
        Arc::new(BootstrapPolicyEvaluator::new())
    }

    /// Whether cloud-issued privileged execution is currently enabled.
    pub fn privileged_execution_enabled(&self) -> bool {
        self.privileged_execution.load(Ordering::SeqCst)
    }

    /// Flips the local kill switch.
    ///
    /// Only the local socket reaches this, and the daemon has already authorized
    /// that peer by uid. No cloud message is routed here, which is what makes the
    /// switch a local operator control (FR-050).
    pub fn set_privileged_execution(&self, enabled: bool) {
        self.privileged_execution.store(enabled, Ordering::SeqCst);
    }

    /// The shared kill switch the cloud supervisor reads on every invocation.
    pub fn privileged_execution_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.privileged_execution)
    }

    /// Routes a capability request through the policy and executor boundaries.
    ///
    /// The risk class and blast radius are derived from the capability's own
    /// registered descriptor. A capability that is not registered cannot be
    /// authorized, and a registered one that does not declare its blast radius is
    /// classified as `Host` rather than `None` (ADR-0020 §2).
    pub fn authorize_and_execute(
        &self,
        request: CapabilityRequest,
    ) -> Result<Value, DispatchError> {
        let Some(descriptor) = self.registry.get(&request.capability) else {
            return Err(DispatchError::Denied(PolicyOutcome::Deny));
        };

        let authz = AuthorizationRequest::new(
            request,
            descriptor.risk_class(),
            descriptor.effective_blast_radius(),
        );
        let decision = self.policy.evaluate(&authz);

        if !decision.is_allowed() {
            return Err(DispatchError::Denied(decision.outcome));
        }

        let action = argus_executor::AuthorizedAction::new(authz.capability_request, decision)?;
        let result = self.executor.execute(&action)?;
        Ok(result.evidence)
    }
}

fn bootstrap_capabilities() -> Vec<CapabilityId> {
    [
        CapabilityId::HOST_STATUS_READ,
        CapabilityId::ARGUS_HEALTH_READ,
        CapabilityId::ARGUS_CONFIG_READ,
        CapabilityId::ARGUS_PLUGINS_LIST,
        CapabilityId::HOST_SERVICE_RESTART,
        CapabilityId::HOST_SERVICE_STOP,
        CapabilityId::HOST_SERVICE_START,
    ]
    .into_iter()
    .map(|c| CapabilityId::new(c).expect("bootstrap capability ids are valid"))
    .collect()
}

fn bootstrap_descriptors() -> Vec<CapabilityDescriptor> {
    bootstrap_capabilities()
        .into_iter()
        .map(|id| {
            let base = CapabilityDescriptor::new(
                id.clone(),
                "argusd",
                id.as_str(),
                if is_service_capability(&id) {
                    RiskClass::LowRisk
                } else {
                    RiskClass::Read
                },
                Version::new(0, 1, 0),
                serde_json::json!({}),
                serde_json::json!({}),
                if is_service_capability(&id) {
                    Reversibility::Reversible
                } else {
                    Reversibility::None
                },
            );

            if is_service_capability(&id) {
                base.with_blast_radius(BlastRadius::Host)
                    .requiring_approval()
            } else {
                base.with_blast_radius(BlastRadius::None)
            }
        })
        .collect()
}

fn is_service_capability(id: &CapabilityId) -> bool {
    matches!(
        id.as_str(),
        CapabilityId::HOST_SERVICE_RESTART
            | CapabilityId::HOST_SERVICE_STOP
            | CapabilityId::HOST_SERVICE_START
    )
}
