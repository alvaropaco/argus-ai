//! In-memory runtime state for the bootstrap daemon.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use argus_ai_core::decision::context::ContextBuilder;
use argus_ai_core::decision::error::DecisionError;
use argus_ai_core::decision::host_health::{ActionPort, DedupPort, RecordPort, run_host_health};
use argus_ai_core::decision::provenance::DecisionProvenance;
use argus_ai_core::decision::provider::DecisionProvider;
use argus_domain::{
    Action, AuthorizationRequest, BlastRadius, CapabilityDescriptor, CapabilityId,
    CapabilityRegistry, CapabilityRequest, DomainEvent, EnvironmentId, EventType, HealthStatus,
    Observation, ObservedValue, Plan, PluginManifest, PolicyOutcome, Principal,
    PrivilegeDeclaration, Provenance, RequestContext, ResourceId, Reversibility, RiskClass,
    Severity,
};
use argus_executor::{BootstrapExecutor, CapabilityProvider, ExecutionError, Executor};
use argus_policy::{BootstrapPolicyEvaluator, PolicyEvaluator};
use argus_state::{DomainRepository, RepositoryError, SqliteRepository};
use chrono::Utc;
use semver::Version;
use serde_json::Value;
use uuid::Uuid;

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
    /// Remembers which observations have already been reasoned about, so a
    /// repeated observation spawns no plan (ADR-0028 §5).
    dedup: InMemoryDedup,
    /// Wakes the cloud supervisor out of a reconnect backoff when the enrollment
    /// changes, so a fresh credential is used immediately rather than after the
    /// current delay elapses.
    cloud_wake: Arc<tokio::sync::Notify>,
}

/// Errors from the capability dispatch boundary.
#[derive(Debug, thiserror::Error)]
pub enum DispatchError {
    #[error("capability denied by policy: {0:?}")]
    Denied(PolicyOutcome),

    #[error("capability input violates its declared schema: {0}")]
    InvalidInput(CapabilityId),

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
            dedup: InMemoryDedup::new(),
            cloud_wake: Arc::new(tokio::sync::Notify::new()),
        })
    }

    pub fn config(&self) -> &DaemonConfig {
        &self.config
    }

    /// Signals the cloud supervisor to re-evaluate immediately.
    pub fn cloud_wake(&self) -> Arc<tokio::sync::Notify> {
        Arc::clone(&self.cloud_wake)
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
            Arc::new(argus_executor::PrivilegedExecutor::new(
                Arc::new(argus_executor::SystemdServiceController::new()),
                service_guardrails(DAEMON_UNIT),
            )),
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

        // Validate the call's `input` against the capability's declared schema
        // before policy is consulted (ADR-0027 §4): a malformed call fails here,
        // not at the executor.
        if !argus_domain::input_matches(descriptor.input_schema(), &request.arguments) {
            return Err(DispatchError::InvalidInput(request.capability.clone()));
        }

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

    /// Runs one host-health reasoning step end to end: build the request from
    /// evidence, decide, gate on confidence, execute through policy+executor,
    /// and record the outcome as an observation and an audit event.
    ///
    /// Inference stays external: the caller supplies the decision provider (the
    /// Laya `laya-serve` sidecar, or the deterministic fake in tests).
    pub async fn diagnose_once(
        &self,
        provider: &dyn DecisionProvider,
        evidence: ContextBuilder,
        confidence_threshold: f64,
    ) -> Result<Option<Plan>, DecisionError> {
        diagnose_with(
            |request| self.authorize_and_execute(request),
            self.repository.as_ref(),
            provider,
            evidence,
            confidence_threshold,
            &self.dedup,
        )
        .await
    }
}

/// Runs one host-health step through an injected dispatch function and repository.
///
/// This is the loop's wiring seam: [`Daemon::diagnose_once`] passes its own
/// `authorize_and_execute`, and tests pass a dispatch that records calls —
/// neither needs a full `Daemon`.
pub async fn diagnose_with<F>(
    dispatch: F,
    repository: &dyn DomainRepository,
    provider: &dyn DecisionProvider,
    evidence: ContextBuilder,
    confidence_threshold: f64,
    dedup: &dyn DedupPort,
) -> Result<Option<Plan>, DecisionError>
where
    F: Fn(CapabilityRequest) -> Result<Value, DispatchError> + Send + Sync,
{
    let correlation_id = Uuid::new_v4();
    let actions = FnActionPort {
        dispatch,
        correlation_id,
    };
    let recorder = RepoRecordPort {
        repository,
        correlation_id,
    };
    run_host_health(
        provider,
        evidence,
        confidence_threshold,
        &actions,
        &recorder,
        dedup,
    )
    .await
}

/// An [`ActionPort`] over an injected dispatch function.
struct FnActionPort<F> {
    dispatch: F,
    correlation_id: Uuid,
}

#[async_trait::async_trait]
impl<F> ActionPort for FnActionPort<F>
where
    F: Fn(CapabilityRequest) -> Result<Value, DispatchError> + Send + Sync,
{
    async fn execute(&self, action: &Action) -> Result<Value, DecisionError> {
        let principal = Principal::new(None, None);
        let context = RequestContext::new(
            self.correlation_id,
            Version::new(0, 1, 0),
            principal,
            Utc::now(),
        );
        let request = CapabilityRequest::new(
            action.capability.clone(),
            principal,
            action.resource.clone(),
            action.arguments.clone(),
            context,
        );
        (self.dispatch)(request).map_err(|e| match e {
            DispatchError::Denied(outcome) => {
                DecisionError::Validation(format!("capability denied by policy: {outcome:?}"))
            }
            DispatchError::InvalidInput(id) => DecisionError::Validation(format!(
                "capability input violates its declared schema: {id}"
            )),
            DispatchError::Execution(error) => {
                DecisionError::Unavailable(format!("execution failed: {error}"))
            }
        })
    }
}

/// A [`RecordPort`] that writes immutable evidence and an audit event.
struct RepoRecordPort<'a> {
    repository: &'a dyn DomainRepository,
    correlation_id: Uuid,
}

#[async_trait::async_trait]
impl RecordPort for RepoRecordPort<'_> {
    async fn record(
        &self,
        plan: &Plan,
        executed: bool,
        provenance: &DecisionProvenance,
    ) -> Result<(), DecisionError> {
        let now = Utc::now();
        let subject =
            ResourceId::new("host", "local").map_err(|e| DecisionError::Invalid(e.to_string()))?;
        let observation = Observation::new(
            Uuid::new_v4(),
            "argusd",
            subject,
            "brain.plan.executed",
            ObservedValue::Bool(executed),
            1.0,
            Provenance::new("argusd", "brain.host_health", now),
            now,
        )
        .map_err(|e| DecisionError::Invalid(e.to_string()))?;
        self.repository
            .put_observation(&observation)
            .await
            .map_err(|e| DecisionError::Unavailable(e.to_string()))?;

        let event = DomainEvent::new(
            Uuid::new_v4(),
            EventType::new("brain.plan.recorded")
                .map_err(|e| DecisionError::Invalid(e.to_string()))?,
            now,
            "argusd",
            "host:local",
            Severity::Info,
            Some(self.correlation_id),
            None,
            serde_json::json!({
                "objective": plan.objective,
                "executed": executed,
                "confidence": plan.confidence,
                "provenance": provenance,
            }),
        );
        self.repository
            .put_audit_event(&event)
            .await
            .map_err(|e| DecisionError::Unavailable(e.to_string()))?;
        Ok(())
    }

    async fn record_dedup(&self, key: &str) -> Result<(), DecisionError> {
        let event = DomainEvent::new(
            Uuid::new_v4(),
            EventType::new("brain.observation.deduped")
                .map_err(|e| DecisionError::Invalid(e.to_string()))?,
            Utc::now(),
            "argusd",
            "host:local",
            Severity::Info,
            Some(self.correlation_id),
            None,
            serde_json::json!({ "dedup_key": key }),
        );
        self.repository
            .put_audit_event(&event)
            .await
            .map_err(|e| DecisionError::Unavailable(e.to_string()))?;
        Ok(())
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
            let service = is_service_capability(&id);
            let input_schema = if service {
                serde_json::json!({
                    "type": "object",
                    "required": ["unit"],
                    "properties": { "unit": { "type": "string" } },
                    "additionalProperties": false,
                })
            } else {
                serde_json::json!({ "type": "object", "additionalProperties": false })
            };

            let base = CapabilityDescriptor::new(
                id.clone(),
                "argusd",
                id.as_str(),
                if service {
                    RiskClass::LowRisk
                } else {
                    RiskClass::Read
                },
                Version::new(0, 1, 0),
                input_schema,
                serde_json::json!({}),
                if service {
                    Reversibility::Reversible
                } else {
                    Reversibility::None
                },
            );

            if service {
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

/// The default upper bound on remembered dedup keys. When the set reaches this
/// size, it is cleared so no observation is suppressed forever and the set does
/// not grow without bound.
const DEFAULT_MAX_KEYS: usize = 1024;

/// A [`DedupPort`] over process memory: remembers claimed observation keys so a
/// repeated observation spawns no plan or execution (ADR-0028 §5).
///
/// In-memory dedup is a per-process guard, not plan/execution persistence — the
/// latter is out of scope for this milestone (ADR-0028 §6).
pub struct InMemoryDedup {
    seen: std::sync::Mutex<std::collections::HashSet<String>>,
    max_keys: usize,
}

impl InMemoryDedup {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_MAX_KEYS)
    }

    /// A bounded dedup set: once `max_keys` keys are remembered, the set is
    /// cleared rather than growing without bound.
    pub fn with_capacity(max_keys: usize) -> Self {
        Self {
            seen: std::sync::Mutex::new(std::collections::HashSet::new()),
            max_keys: max_keys.max(1),
        }
    }
}

impl Default for InMemoryDedup {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl DedupPort for InMemoryDedup {
    async fn claim(&self, key: &str) -> Result<bool, DecisionError> {
        let mut seen = self
            .seen
            .lock()
            .map_err(|_| DecisionError::Invalid("dedup key set poisoned".to_string()))?;
        if seen.contains(key) {
            return Ok(false);
        }
        if seen.len() >= self.max_keys {
            seen.clear();
        }
        seen.insert(key.to_string());
        Ok(true)
    }

    async fn release(&self, key: &str) -> Result<(), DecisionError> {
        self.seen
            .lock()
            .map_err(|_| DecisionError::Invalid("dedup key set poisoned".to_string()))?
            .remove(key);
        Ok(())
    }
}

/// The daemon's own systemd unit name (matching `deploy/debian/argusd.service`).
const DAEMON_UNIT: &str = "argusd";

/// The execution-time guardrails wired for the bootstrap service capabilities.
///
/// `never_target_argusd` and `valid_service_unit` run on every service effect;
/// `allowed_targets` is registered only when a target set is configured (none in
/// the bootstrap). The daemon owns this wiring (ADR-0028 §1).
fn service_guardrails(daemon_unit: &str) -> argus_executor::GuardrailRegistry {
    let mut registry = argus_executor::GuardrailRegistry::new();
    for capability in [
        CapabilityId::HOST_SERVICE_RESTART,
        CapabilityId::HOST_SERVICE_STOP,
        CapabilityId::HOST_SERVICE_START,
    ] {
        let id = CapabilityId::new(capability).expect("bootstrap capability ids are valid");
        registry.register(
            id.clone(),
            Arc::new(argus_executor::NeverTargetArgusd::new(daemon_unit)),
        );
        registry.register(id, Arc::new(argus_executor::ValidServiceUnit));
    }
    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_guardrails_refuse_the_configured_unit_and_malformed_names() {
        let registry = service_guardrails("customd");
        let restart = CapabilityId::new(CapabilityId::HOST_SERVICE_RESTART).unwrap();

        assert!(registry.check(&restart, "customd").is_err());
        assert!(registry.check(&restart, "customd.service").is_err());
        assert!(
            registry.check(&restart, "argusd").is_ok(),
            "the configured unit, not the default, must be refused"
        );
        assert!(registry.check(&restart, "bad unit").is_err());
        assert!(registry.check(&restart, "nginx.service").is_ok());
    }
}
