//! In-memory runtime state for the bootstrap daemon.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use argus_ai_core::decision::context::ContextBuilder;
use argus_ai_core::decision::error::DecisionError;
use argus_ai_core::decision::host_health::{
    ActionPort, DedupPort, RecordPort, ValidationPort, run_host_health,
};
use argus_ai_core::decision::provenance::DecisionProvenance;
use argus_ai_core::decision::provider::DecisionProvider;
use argus_domain::{
    Action, AuthorizationRequest, AutonomyMode, BlastRadius, CapabilityDescriptor, CapabilityId,
    CapabilityRegistry, CapabilityRequest, DomainEvent, EnvironmentId, EventType, Execution,
    ExecutionStatus, HealthStatus, Hypothesis, HypothesisStatus, Observation, ObservedValue, Plan,
    PlanStatus, PluginManifest, PolicyOutcome, Principal, PrivilegeDeclaration, Provenance,
    RequestContext, ResourceId, Reversibility, RiskClass, Severity, plan_context_hash,
};
use argus_events::EventBus;
use argus_executor::{
    BootstrapExecutor, CapabilityProvider, CgroupController, CgroupV2Controller,
    ContainerController, DockerContainerController, ExecutionError, Executor, ProcessController,
    RemediationExecutor, ServiceController, UnixProcessController, remediation_guardrails,
};
use argus_observability::SelfObservability;
use argus_policy::{ApprovalStore, AutopilotGovernor, BootstrapPolicyEvaluator, PolicyEvaluator};
use argus_state::{DomainRepository, RepositoryError, SqliteRepository};
use argus_validate::{desired_state, read_failure_event, validation_event};
use chrono::Utc;
use semver::Version;
use serde_json::{Map, Value};
use uuid::Uuid;

use argus_cloud::buffer::ReportQueue;
use argus_cloud::state::ConnectivityTracker;

use crate::cloud::{CloudSecretStore, ManagedSettingsStore as CloudSettingsStore};
use crate::config::DaemonConfig;
use crate::control::{ExecutionOutcome, LoopPorts, PendingPlan, ResumeOutcome, RunOutcome};

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
    /// The live-state reader for post-execution validation (ADR-0031 §3): the
    /// daemon re-observes a unit's active state through `ServiceController` and
    /// records it as evidence.
    service: Arc<dyn ServiceController>,
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
    /// Plans paused at an approval-requiring step, awaiting an operator decision.
    /// In-memory only (ADR-0028 §6); a restart drops them, which is fail-closed.
    pending: PendingApprovals,
    /// Operator grants and denials for the local plan path, bound to a token and
    /// a context hash and consumed exactly once (ADR-0030 §4).
    approvals: ApprovalStore,
    /// Decision-provider health (FR-009): flips to degraded when a reasoning
    /// step fails, back to ready when one succeeds.
    provider_health: Arc<std::sync::Mutex<ProviderHealth>>,
    /// Remediation controllers and the executor boundary for the spec-003 M3
    /// capabilities (FR-016/FR-017). Guardrails are wired into the executor.
    processes: Arc<dyn ProcessController>,
    containers: Arc<dyn ContainerController>,
    cgroups: Arc<dyn CgroupController>,
    remediation: Arc<RemediationExecutor>,
    /// The resource-autopilot governor: the policy-layer gate every governed
    /// adjustment passes before policy (FR-017).
    governor: Arc<AutopilotGovernor>,
    /// Self-observability counters (CAP-24, FR-025), recorded at the daemon
    /// boundary — one hop coarse: run outcomes, not control-loop internals.
    selfobs: Arc<SelfObservability>,
    /// The AI decision provider built from `[model]` + the secret store at
    /// startup (spec 005 FR-002); `None` = observe-only. Swappable for tests
    /// and future reconfiguration.
    provider:
        std::sync::RwLock<Option<Arc<dyn argus_ai_core::decision::provider::DecisionProvider>>>,
    /// Runbooks loaded at startup from the configured directory (spec 005
    /// FR-005); empty when no directory is configured.
    runbooks: argus_runbooks::RunbookLibrary,
    /// The live Kubernetes cluster bridge and its typed executor, when the
    /// `kubernetes` feature is compiled in AND `[kubernetes] enabled = true`
    /// AND the kubeconfig loads (ADR-0037: degrade, never require).
    cluster: Option<Arc<dyn argus_executor::ClusterController>>,
    kubernetes_executor: Option<Arc<argus_executor::KubernetesExecutor>>,
    /// The brain's shared state and live control levers (spec 006): the loop
    /// writes cycles / reads levers; the cloud report and config channel use
    /// the same handles.
    brain_state: Arc<crate::brain_state::BrainState>,
    brain_control: Arc<crate::brain_state::BrainControlHandle>,
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

/// Errors from the operator approval surface.
#[derive(Debug, thiserror::Error)]
pub enum ApprovalError {
    #[error("no pending approval with token '{0}'")]
    UnknownToken(Uuid),
}

/// The availability of the decision provider (FR-009).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderStatus {
    /// The last reasoning step completed.
    Ready,
    /// The last reasoning step failed; no plan was produced from it.
    Degraded,
}

/// A snapshot of the decision provider's health (FR-009): reported through
/// `status.get`, never used to fabricate a plan — a degraded provider simply
/// yields no plan, and the condition is what the operator sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderHealth {
    pub status: ProviderStatus,
    /// Why the provider is degraded; cleared on recovery.
    pub last_error: Option<String>,
    pub updated_at: chrono::DateTime<Utc>,
}

impl ProviderHealth {
    fn ready(now: chrono::DateTime<Utc>) -> Self {
        Self {
            status: ProviderStatus::Ready,
            last_error: None,
            updated_at: now,
        }
    }

    fn degraded(error: String, now: chrono::DateTime<Utc>) -> Self {
        Self {
            status: ProviderStatus::Degraded,
            last_error: Some(error),
            updated_at: now,
        }
    }

    fn to_json(&self) -> Value {
        serde_json::json!({
            "status": match self.status {
                ProviderStatus::Ready => "ready",
                ProviderStatus::Degraded => "degraded",
            },
            "last_error": self.last_error,
            "updated_at": self.updated_at.to_rfc3339(),
        })
    }
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

/// Connect the live Kubernetes bridge when compiled in and configured
/// (ADR-0037 §2). Without the feature or the configuration this returns
/// `None` and every k8s capability degrades honestly.
fn connect_cluster(
    config: &DaemonConfig,
) -> (
    Option<Arc<dyn argus_executor::ClusterController>>,
    Option<Arc<argus_executor::KubernetesExecutor>>,
) {
    #[cfg(feature = "kubernetes")]
    {
        if !config.kubernetes.enabled {
            return (None, None);
        }
        let connected = argus_kubernetes::KubeClusterController::from_kubeconfig(
            config.kubernetes.kubeconfig.as_deref(),
            config.kubernetes.context.as_deref(),
        );
        match tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(connected))
        {
            Ok(controller) => {
                let cluster: Arc<dyn argus_executor::ClusterController> = Arc::new(controller);
                tracing::info!("kubernetes cluster connected; typed k8s capabilities live");
                // kube-system and kube-public are never autopilot targets;
                // the replica quota bounds k8s.workload.scale at the executor.
                let executor = argus_executor::KubernetesExecutor::new(
                    cluster.clone(),
                    argus_executor::kubernetes_guardrails(&[
                        "kube-system".to_string(),
                        "kube-public".to_string(),
                    ]),
                    10,
                );
                (Some(cluster), Some(Arc::new(executor)))
            }
            Err(reason) => {
                tracing::warn!(
                    reason = %reason,
                    "kubernetes enabled but unreachable; k8s capabilities degrade"
                );
                (None, None)
            }
        }
    }
    #[cfg(not(feature = "kubernetes"))]
    {
        if config.kubernetes.enabled {
            tracing::warn!(
                "[kubernetes] enabled but this build lacks the kubernetes feature;                  k8s capabilities degrade"
            );
        }
        (None, None)
    }
}

/// Load the runbook library from the configured directory (spec 005
/// FR-005); no directory or an unreadable one yields an empty library with
/// the reasons logged — runbooks are never load-bearing for startup.
fn load_runbooks(config: &DaemonConfig) -> argus_runbooks::RunbookLibrary {
    match &config.brain.runbooks_dir {
        Some(dir) => {
            let result = argus_runbooks::load_runbooks(std::path::Path::new(dir));
            tracing::info!("{}", result.summary());
            result.library
        }
        None => argus_runbooks::RunbookLibrary::default(),
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

        // The remediation surface (spec 003 M3, FR-016/FR-017): typed
        // controllers behind their ports, guardrails wired at the executor
        // boundary, and the autopilot governor that gates every governed
        // adjustment. The default classification set is empty — a subject the
        // operator has not classified is refused (deny-by-default).
        let processes: Arc<dyn ProcessController> = Arc::new(UnixProcessController::new());
        let containers: Arc<dyn ContainerController> = Arc::new(DockerContainerController::new());
        let cgroups: Arc<dyn CgroupController> = Arc::new(CgroupV2Controller::default());
        let remediation_executor = Arc::new(RemediationExecutor::new(
            processes.clone(),
            containers.clone(),
            cgroups.clone(),
            remediation_guardrails(&[], &[]),
        ));
        let governor = Arc::new(AutopilotGovernor::default());

        let buffer_capacity = config.cloud.report_buffer_max_records;
        let privileged_execution =
            Arc::new(AtomicBool::new(config.cloud.allow_privileged_execution));

        let secret_store = CloudSecretStore::default_location();
        let (cluster, kubernetes_executor) = connect_cluster(&config);
        let decision_provider = crate::brain::build_provider(&config, &secret_store);
        let runbooks = load_runbooks(&config);
        let (brain_state, brain_control) = {
            use crate::brain_state::{BrainControl, BrainControlHandle, BrainState};
            let control = BrainControlHandle::new(BrainControl {
                autonomy: config.brain.autonomy,
                confidence_threshold: config.brain.confidence_threshold,
                interval_seconds: config.brain.interval_seconds,
            });
            // A deployed `brain` configuration outlives a restart: the
            // persisted managed settings re-seed the levers (spec 006 FR-002).
            if let Ok(Some(settings)) = CloudSettingsStore::default_location().read() {
                for (key, value) in &settings {
                    if key.starts_with("brain.")
                        && let Err(reason) = control
                            .apply_setting(key, &serde_json::to_value(value).unwrap_or_default())
                    {
                        tracing::warn!(key = %key, reason = %reason, "persisted brain setting ignored");
                    }
                }
            }
            (Arc::new(BrainState::default()), Arc::new(control))
        };
        Ok(Self {
            started_at: Instant::now(),
            environment_id,
            config,
            repository,
            secrets: secret_store.clone(),
            tracker: Arc::new(tokio::sync::Mutex::new(ConnectivityTracker::new())),
            queue: Arc::new(tokio::sync::Mutex::new(ReportQueue::new(buffer_capacity))),
            managed_settings: CloudSettingsStore::default_location(),
            policy: BootstrapPolicyEvaluator::new(),
            executor: BootstrapExecutor::new(provider),
            service: Arc::new(argus_executor::SystemdServiceController::new()),
            registry,
            plugins: Vec::new(),
            privileged_execution,
            dedup: InMemoryDedup::new(),
            cloud_wake: Arc::new(tokio::sync::Notify::new()),
            pending: PendingApprovals::new(),
            approvals: ApprovalStore::new(),
            provider_health: Arc::new(std::sync::Mutex::new(ProviderHealth::ready(Utc::now()))),
            processes: processes.clone(),
            containers: containers.clone(),
            cgroups: cgroups.clone(),
            remediation: remediation_executor.clone(),
            governor: governor.clone(),
            selfobs: Arc::new(SelfObservability::new()),
            provider: std::sync::RwLock::new(decision_provider),
            runbooks,
            cluster,
            kubernetes_executor,
            brain_state,
            brain_control,
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

    /// The recorded reasoning history, keyed by correlation id (FR-008):
    /// the hypotheses, plans, and executions behind the audit trail.
    pub async fn list_hypotheses(&self) -> Result<Vec<(Uuid, Hypothesis)>, RepositoryError> {
        self.repository.list_hypotheses().await
    }

    pub async fn list_plans(&self) -> Result<Vec<(Uuid, Plan)>, RepositoryError> {
        self.repository.list_plans().await
    }

    pub async fn list_executions(&self) -> Result<Vec<(Uuid, Execution)>, RepositoryError> {
        self.repository.list_executions().await
    }

    /// The audit events recorded so far (FR-008).
    pub async fn list_audit_events(&self) -> Result<Vec<DomainEvent>, RepositoryError> {
        self.repository.list_audit_events().await
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
            "provider": self.provider_health().to_json(),
        })
    }

    /// A snapshot of the decision provider's health (FR-009).
    pub fn provider_health(&self) -> ProviderHealth {
        self.provider_health
            .lock()
            .expect("provider health is not poisoned")
            .clone()
    }

    /// Records that a reasoning step succeeded, ending any degraded condition.
    fn mark_provider_ready(&self) {
        let mut health = self
            .provider_health
            .lock()
            .expect("provider health is not poisoned");
        if health.status != ProviderStatus::Ready {
            tracing::info!("decision provider recovered");
        }
        *health = ProviderHealth::ready(Utc::now());
    }

    /// Records that a reasoning step failed (FR-009): the provider is degraded
    /// and the condition is reported; no plan was produced from the failed call.
    /// Returns `true` when this is a `Ready → Degraded` transition, so the
    /// caller can publish `provider.degraded` once, not per failure.
    fn mark_provider_degraded(&self, error: &str) -> bool {
        let mut health = self
            .provider_health
            .lock()
            .expect("provider health is not poisoned");
        let transitioned = health.status != ProviderStatus::Degraded;
        *health = ProviderHealth::degraded(error.to_string(), Utc::now());
        tracing::warn!(error = %error, "decision provider degraded");
        transitioned
    }

    /// The `provider.degraded` event for a transition, published by the caller.
    fn degraded_transition_event(error: &str) -> DomainEvent {
        DomainEvent::new(
            Uuid::new_v4(),
            EventType::new(argus_events::types::PROVIDER_DEGRADED)
                .expect("provider.degraded is a valid event type"),
            Utc::now(),
            "argusd",
            "argusd",
            Severity::Warning,
            None,
            None,
            serde_json::json!({ "error": error }),
        )
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

    /// Stores a paused plan awaiting operator approval.
    ///
    /// The plan loop pauses here when a step's policy returns `RequireApproval`;
    /// the single-use token is what the operator later grants against.
    pub fn store_pending(&self, pending: PendingPlan) {
        self.pending.store(pending);
    }

    /// Lists plans currently paused awaiting an operator decision.
    pub fn list_pending_approvals(&self) -> Vec<PendingPlan> {
        self.pending.list()
    }

    /// Grants approval for a pending plan's token, bound to its context hash.
    ///
    /// The grant expires after a fixed window; a later grant replaces it, and a
    /// resume consumes it exactly once (ADR-0030 §4).
    /// Grants the approval and hands back the paused plan it releases —
    /// the caller resumes it (`resume_remediation`), completing the
    /// round-trip. The grant is timed (15 minutes) and consumed exactly
    /// once by the resume.
    pub fn grant_approval(
        &self,
        token: Uuid,
        granted_by: &str,
    ) -> Result<PendingPlan, ApprovalError> {
        let pending = self
            .pending
            .remove(token)
            .ok_or(ApprovalError::UnknownToken(token))?;
        self.approvals.grant_for_a_while(
            token,
            pending.context_hash.clone(),
            granted_by,
            Utc::now(),
            chrono::Duration::minutes(15),
        );
        Ok(pending)
    }

    /// Denies a pending plan's token; a denied approval never executes.
    pub fn deny_approval(&self, token: Uuid, granted_by: &str) -> Result<(), ApprovalError> {
        let pending = self
            .pending
            .remove(token)
            .ok_or(ApprovalError::UnknownToken(token))?;
        self.approvals
            .deny(token, pending.context_hash, granted_by, Utc::now());
        Ok(())
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
        authorize_and_execute_with(&self.registry, &self.policy, &self.executor, request)
    }

    /// The autopilot governor gating governed adjustments (FR-017).
    pub fn governor(&self) -> Arc<AutopilotGovernor> {
        Arc::clone(&self.governor)
    }

    /// The process-signal controller behind the remediation executor, for
    /// health checks and configuration surfaces (CAP-24, later milestones).
    pub fn processes(&self) -> Arc<dyn ProcessController> {
        Arc::clone(&self.processes)
    }

    /// Runs a proposed remediation plan through the full safety boundary
    /// (spec 003 M3, T021): autopilot governor → policy → autonomy → executor
    /// → validation, with fail-stop declarative rollback. The policy used here
    /// permits the remediation capabilities; the cloud channel's does not, so
    /// a cloud-issued plan cannot reach this loop's executors.
    pub async fn run_remediation(
        &self,
        plan: &Plan,
        autonomy: AutonomyMode,
        events: &dyn EventBus,
    ) -> RunOutcome {
        let policy = BootstrapPolicyEvaluator::with_local_remediation();
        let ports = self.remediation_ports();
        let outcome = crate::control::authorize_and_run_with_ports(
            plan,
            &self.registry,
            &policy,
            self.service.as_ref(),
            events,
            autonomy,
            &ports,
        )
        .await;
        // A paused plan is stored, so the operator approval surface
        // (`approval.grant`/`deny`) can act on its token — a plan the daemon
        // forgot would be unapprovable and stuck forever.
        if let RunOutcome::Pending(pending) = &outcome {
            self.pending.store(pending.clone());
        }
        if let RunOutcome::Finished(report) = &outcome {
            self.record_run_outcome(report);
        }
        outcome
    }

    /// Record one finished run into the self-observability counters
    /// (CAP-24). Deliberately coarse — one hop, at the daemon boundary:
    /// policy denials and per-step executor failures come from the report,
    /// and a plan that failed with no failed step failed *after* execution,
    /// which is the validation/rollback path.
    fn record_run_outcome(&self, report: &ExecutionOutcome) {
        self.selfobs.record_decision_completed();
        self.selfobs.record_event_processed();
        for _ in &report.denied {
            self.selfobs.record_policy_denial();
        }
        for execution in &report.executions {
            if execution.status == ExecutionStatus::Failed {
                self.selfobs.record_executor_error();
            }
        }
        let succeeded = report.status == PlanStatus::Completed;
        if !succeeded
            && report.status == PlanStatus::Failed
            && report
                .executions
                .iter()
                .all(|e| e.status != ExecutionStatus::Failed)
        {
            // Failed with every step executed: the failure was detected
            // after execution — a failed validation.
            self.selfobs.record_failed_validation();
        }
        self.selfobs.record_remediation(succeeded);
        self.selfobs.record_autonomous_action(succeeded);
    }

    /// Resumes a paused remediation plan with a previously granted approval.
    pub async fn resume_remediation(
        &self,
        pending: &PendingPlan,
        events: &dyn EventBus,
    ) -> ResumeOutcome {
        let policy = BootstrapPolicyEvaluator::with_local_remediation();
        let ports = self.remediation_ports();
        let outcome = crate::control::resume_and_run_with_ports(
            pending,
            &self.approvals,
            &self.registry,
            &policy,
            self.service.as_ref(),
            events,
            // A resumed plan is authorized; Propose would block the
            // already-approved step again.
            AutonomyMode::L3Assisted,
            &ports,
        )
        .await;
        if let ResumeOutcome::Finished(report) = &outcome {
            self.record_run_outcome(report);
        }
        outcome
    }

    /// The live sentinel view (spec-004 FR-003): built from real daemon state
    /// only. Risks, predictions, and incidents have no live source wired into
    /// the daemon yet and render empty — never invented.
    pub async fn sentinel_snapshot(&self) -> crate::sentinel::SentinelView {
        let executions = self.list_executions().await.unwrap_or_default();
        let inputs = crate::sentinel::SentinelInputs {
            autonomy: AutonomyMode::default(),
            environment: crate::sentinel::Environment::Production,
            provider_ready: self.provider_health().status == ProviderStatus::Ready,
            open_incidents: 0,
            risks: Vec::new(),
            predictions: Vec::new(),
            pending_approvals: self.list_pending_approvals().len() as u32,
            recent_actions: executions.len() as u32,
            self_health: self.selfobs.snapshot(),
            situation: None,
        };
        crate::sentinel::sentinel_evaluate(&inputs, Utc::now()).view
    }

    /// The self-observability counters, for status/telemetry surfaces.
    pub fn self_observability(&self) -> &SelfObservability {
        &self.selfobs
    }

    /// The AI decision provider, when one is configured (spec 005 FR-002);
    /// `None` means the brain runs observe-only.
    pub fn provider(&self) -> Option<Arc<dyn argus_ai_core::decision::provider::DecisionProvider>> {
        self.provider
            .read()
            .expect("provider lock is not poisoned")
            .clone()
    }

    /// Replaces the decision provider (tests, future reconfiguration).
    pub fn set_provider(
        &self,
        provider: Option<Arc<dyn argus_ai_core::decision::provider::DecisionProvider>>,
    ) {
        *self
            .provider
            .write()
            .expect("provider lock is not poisoned") = provider;
    }

    /// The runbooks loaded at startup (spec 005 FR-005).
    pub fn runbooks(&self) -> &argus_runbooks::RunbookLibrary {
        &self.runbooks
    }

    /// The brain's shared cycle record (spec 006 FR-001).
    pub fn brain_state(&self) -> Arc<crate::brain_state::BrainState> {
        Arc::clone(&self.brain_state)
    }

    /// The brain's live control levers (spec 006 FR-002).
    pub fn brain_control(&self) -> Arc<crate::brain_state::BrainControlHandle> {
        Arc::clone(&self.brain_control)
    }

    /// The live Kubernetes cluster bridge, when connected (ADR-0037);
    /// `None` means every k8s capability degrades honestly.
    pub fn cluster(&self) -> Option<Arc<dyn argus_executor::ClusterController>> {
        self.cluster.clone()
    }

    /// The remediation ports over the daemon's own controllers and executor.
    ///
    /// The Kubernetes port ships degraded (no cluster configured) until the
    /// `kube` feature's cluster configuration lands; effects then fail with a
    /// clear `Unavailable` rather than silently succeeding (ADR-0037 §2).
    fn remediation_ports(&self) -> LoopPorts<'_> {
        static UNAVAILABLE_CLUSTER: argus_executor::UnavailableClusterController =
            argus_executor::UnavailableClusterController;
        static NO_KUBERNETES: NoKubernetesExecutor = NoKubernetesExecutor;
        LoopPorts {
            governor: self.governor.as_ref(),
            containers: self.containers.as_ref(),
            cgroups: self.cgroups.as_ref(),
            cluster: self.cluster.as_deref().unwrap_or(&UNAVAILABLE_CLUSTER),
            remediation: self.remediation.as_ref(),
            kubernetes: self
                .kubernetes_executor
                .as_deref()
                .map(|executor| executor as &dyn argus_executor::Executor)
                .unwrap_or(&NO_KUBERNETES),
        }
    }

    /// The decide-only half of the reasoning loop (spec 005): dedup, pose
    /// the structured decisions, gate on confidence, and return the typed
    /// plan **unexecuted** with its dedup key — the brain executes through
    /// `run_remediation`, the modern boundary, not through the dispatch
    /// port. Provider health transitions mirror `diagnose_once`.
    pub async fn propose_once(
        &self,
        provider: &dyn DecisionProvider,
        evidence: ContextBuilder,
        threshold: f64,
        events: &dyn EventBus,
    ) -> Result<Option<(Plan, String)>, DecisionError> {
        use argus_ai_core::decision::host_health::{EvidenceDecision, plan_from_evidence};

        let result = plan_from_evidence(provider, evidence, threshold, &self.dedup).await;
        match &result {
            Ok(_) => self.mark_provider_ready(),
            Err(error) => {
                if self.mark_provider_degraded(&error.to_string()) {
                    let _ = events
                        .publish(&Self::degraded_transition_event(&error.to_string()))
                        .await;
                }
            }
        }
        match result? {
            EvidenceDecision::Plan(plan, _provenance, key) => Ok(Some((*plan, key))),
            EvidenceDecision::Deduplicated(_) | EvidenceDecision::Nothing => Ok(None),
        }
    }

    /// Resumes the paused plan bound to `token` after an operator grant —
    /// the second half of the approval round-trip (ADR-0030 §5): the grant
    /// is consumed exactly once, the stored plan re-enters as-is, and the
    /// outcome is what the operator needs to see. `None` when no pending
    /// plan carries the token.
    pub async fn resume_pending(
        &self,
        token: Uuid,
        events: &dyn EventBus,
    ) -> Option<ResumeOutcome> {
        let pending = self.pending.remove(token)?;
        Some(self.resume_remediation(&pending, events).await)
    }

    /// Release a dedup key so the same evidence can be reasoned about again
    /// (the brain releases when a remediation did not resolve the situation).
    pub async fn release_dedup(&self, key: &str) {
        let _: Result<(), _> = self.dedup.release(key).await;
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
        events: &dyn EventBus,
    ) -> Result<Option<Plan>, DecisionError> {
        let result = diagnose_with(
            |request| self.authorize_and_execute(request),
            DiagnoseContext {
                repository: self.repository.as_ref(),
                provider,
                dedup: &self.dedup,
                service: self.service.as_ref(),
                events,
            },
            evidence,
            confidence_threshold,
        )
        .await;

        // FR-009: the provider's availability is what the run just proved. A
        // failure marks it degraded (publishing the condition once per
        // transition); success marks it ready again. Either way the failed
        // call itself fabricated nothing — the result is passed through.
        match &result {
            Ok(_) => self.mark_provider_ready(),
            Err(error) => {
                if self.mark_provider_degraded(&error.to_string()) {
                    let _ = events
                        .publish(&Self::degraded_transition_event(&error.to_string()))
                        .await;
                }
            }
        }

        result
    }
}

/// Routes a capability request through an injected registry, policy, and
/// executor.
///
/// This is the pure dispatch boundary behind [`Daemon::authorize_and_execute`],
/// extracted so the validation-before-policy ordering is testable without a full
/// [`Daemon`]: the input is validated against the descriptor's `input_schema`
/// *before* the policy is consulted, so a malformed call is refused before any
/// policy side effect (ADR-0027 §4).
pub(crate) fn authorize_and_execute_with(
    registry: &CapabilityRegistry,
    policy: &dyn PolicyEvaluator,
    executor: &dyn Executor,
    request: CapabilityRequest,
) -> Result<Value, DispatchError> {
    let Some(descriptor) = registry.get(&request.capability) else {
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
    let decision = policy.evaluate(&authz);

    if !decision.is_allowed() {
        return Err(DispatchError::Denied(decision.outcome));
    }

    let action = argus_executor::AuthorizedAction::new(authz.capability_request, decision)?;
    let result = executor.execute(&action)?;
    Ok(result.evidence)
}

/// The injected loop dependencies shared by [`diagnose_with`]'s internal ports.
///
/// They are grouped into one value so the loop seam does not take a long
/// positional argument list; the daemon assembles it from its own repository,
/// provider, dedup, live-state reader, and event bus. Each field is an
/// independent dependency, injected individually so the loop stays testable
/// without a host or a full daemon.
pub struct DiagnoseContext<'a> {
    pub repository: &'a dyn DomainRepository,
    pub provider: &'a dyn DecisionProvider,
    pub dedup: &'a dyn DedupPort,
    pub service: &'a dyn ServiceController,
    pub events: &'a dyn EventBus,
}

/// Runs one host-health step through an injected dispatch function and
/// [`DiagnoseContext`].
///
/// This is the loop's wiring seam: [`Daemon::diagnose_once`] passes its own
/// `authorize_and_execute`, and tests pass a dispatch that records calls —
/// neither needs a full `Daemon`.
pub async fn diagnose_with<F>(
    dispatch: F,
    context: DiagnoseContext<'_>,
    evidence: ContextBuilder,
    confidence_threshold: f64,
) -> Result<Option<Plan>, DecisionError>
where
    F: Fn(CapabilityRequest) -> Result<Value, DispatchError> + Send + Sync,
{
    let correlation_id = Uuid::new_v4();
    tracing::info!(
        correlation_id = %correlation_id,
        threshold = confidence_threshold,
        "host-health reasoning step started"
    );
    let actions = FnActionPort {
        dispatch,
        repository: context.repository,
        correlation_id,
    };
    let recorder = RepoRecordPort {
        repository: context.repository,
        correlation_id,
    };
    let validator = DaemonValidationPort {
        service: context.service,
        repository: context.repository,
        events: context.events,
        correlation_id,
    };
    let outcome = run_host_health(
        context.provider,
        evidence,
        confidence_threshold,
        &actions,
        &recorder,
        context.dedup,
        &validator,
    )
    .await;
    match &outcome {
        Ok(Some(plan)) => tracing::info!(
            correlation_id = %correlation_id,
            objective = %plan.objective,
            confidence = plan.confidence,
            "reasoning step produced a plan"
        ),
        Ok(None) => tracing::info!(
            correlation_id = %correlation_id,
            "reasoning step produced no plan"
        ),
        Err(error) => tracing::warn!(
            correlation_id = %correlation_id,
            error = %error,
            "reasoning step failed; no plan was fabricated"
        ),
    }
    outcome
}

/// An [`ActionPort`] over an injected dispatch function.
///
/// Every attempt — completed or failed — is persisted as an [`Execution`]
/// behind the repository, keyed by the step's correlation id (FR-008). A
/// persistence failure never masks the execution result itself.
struct FnActionPort<'a, F> {
    dispatch: F,
    repository: &'a dyn DomainRepository,
    correlation_id: Uuid,
}

#[async_trait::async_trait]
impl<'a, F> ActionPort for FnActionPort<'a, F>
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
        match (self.dispatch)(request) {
            Ok(evidence) => {
                self.persist_execution(action, ExecutionStatus::Completed, evidence.clone())
                    .await;
                Ok(evidence)
            }
            Err(e) => {
                let (message, kind) = match &e {
                    DispatchError::Denied(outcome) => (
                        format!("capability denied by policy: {outcome:?}"),
                        "denied",
                    ),
                    DispatchError::InvalidInput(id) => (
                        format!("capability input violates its declared schema: {id}"),
                        "invalid_input",
                    ),
                    DispatchError::Execution(error) => {
                        (format!("execution failed: {error}"), "execution")
                    }
                };
                self.persist_execution(
                    action,
                    ExecutionStatus::Failed,
                    serde_json::json!({ "error": message, "kind": kind }),
                )
                .await;
                Err(match e {
                    DispatchError::Denied(_) => DecisionError::Validation(message),
                    DispatchError::InvalidInput(_) => DecisionError::Validation(message),
                    DispatchError::Execution(_) => DecisionError::Unavailable(message),
                })
            }
        }
    }
}

impl<F> FnActionPort<'_, F> {
    async fn persist_execution(&self, action: &Action, status: ExecutionStatus, evidence: Value) {
        let execution = Execution {
            action: action.clone(),
            status,
            evidence,
        };
        if let Err(error) = self
            .repository
            .put_execution(self.correlation_id, &execution)
            .await
        {
            tracing::warn!(
                correlation_id = %self.correlation_id,
                error = %error,
                "could not persist the execution record"
            );
        }
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

        // The accepted hypothesis that produced this plan, keyed by the same
        // correlation id: for the bootstrap host-health loop the plan carries
        // the hypothesis verbatim (objective, confidence), so persisting the
        // derivation keeps the reasoning history queryable. The evidence ids
        // are not carried into `record`, so the field stays empty rather than
        // invented (FR-008).
        let hypothesis = Hypothesis {
            statement: plan.objective.clone(),
            confidence: plan.confidence,
            supporting_evidence: Vec::new(),
            status: HypothesisStatus::Confirmed,
        };
        self.repository
            .put_hypothesis(self.correlation_id, &hypothesis)
            .await
            .map_err(|e| DecisionError::Unavailable(e.to_string()))?;
        self.repository
            .put_plan(self.correlation_id, plan)
            .await
            .map_err(|e| DecisionError::Unavailable(e.to_string()))?;

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

/// A [`ValidationPort`] that re-observes live state through a
/// [`ServiceController`], records the re-observation as an [`Observation`], and
/// publishes the validation outcome (ADR-0031 §3, §4).
///
/// A live-state read failure fails closed: no `VALIDATION_PASSED` is published
/// and the failure is recorded as an audit event.
struct DaemonValidationPort<'a> {
    service: &'a dyn ServiceController,
    repository: &'a dyn DomainRepository,
    events: &'a dyn EventBus,
    correlation_id: Uuid,
}

#[async_trait::async_trait]
impl ValidationPort for DaemonValidationPort<'_> {
    async fn validate(&self, plan: &Plan) -> Result<(), DecisionError> {
        let context_hash = plan_context_hash(plan);
        for step in &plan.steps {
            let action = &step.action;
            let Some(expected) = desired_state(&action.capability) else {
                continue;
            };
            let Some(unit) = action.arguments.get("unit").and_then(Value::as_str) else {
                continue;
            };

            match self.service.is_active(unit) {
                Ok(observed) => {
                    self.record_observation(unit, observed).await?;
                    self.publish_outcome(&context_hash, action, unit, expected, observed)
                        .await?;
                }
                Err(error) => {
                    // Fail closed: no pass is fabricated when the read fails.
                    self.record_read_failure(&context_hash, action, unit, &error)
                        .await?;
                }
            }
        }
        Ok(())
    }
}

impl DaemonValidationPort<'_> {
    /// Records the re-observed active state as immutable evidence.
    async fn record_observation(&self, unit: &str, observed: bool) -> Result<(), DecisionError> {
        let now = Utc::now();
        let subject =
            ResourceId::new("service", unit).map_err(|e| DecisionError::Invalid(e.to_string()))?;
        let observation = Observation::new(
            Uuid::new_v4(),
            "argusd",
            subject,
            "service.active",
            ObservedValue::Bool(observed),
            1.0,
            Provenance::new("systemd", "is_active", now),
            now,
        )
        .map_err(|e| DecisionError::Invalid(e.to_string()))?;
        self.repository
            .put_observation(&observation)
            .await
            .map_err(|e| DecisionError::Unavailable(e.to_string()))
    }

    /// Publishes and persists the validation outcome, or does nothing when it is
    /// inconclusive. The payload is built by the shared `argus_validate`
    /// constructor so the two daemon hooks cannot drift (ADR-0031 §6).
    async fn publish_outcome(
        &self,
        context_hash: &str,
        action: &Action,
        unit: &str,
        expected: bool,
        observed: bool,
    ) -> Result<(), DecisionError> {
        let Some((event_type, payload)) =
            validation_event(&action.capability, unit, expected, observed, context_hash)
        else {
            return Ok(());
        };
        let event = DomainEvent::new(
            Uuid::new_v4(),
            EventType::new(event_type).map_err(|e| DecisionError::Invalid(e.to_string()))?,
            Utc::now(),
            "argusd",
            "argusd",
            Severity::Info,
            Some(self.correlation_id),
            None,
            payload,
        );
        self.events
            .publish(&event)
            .await
            .map_err(|e| DecisionError::Unavailable(e.to_string()))?;
        // Persist the outcome too: the read-only learning pass reads repository
        // audit events, not the bus (ADR-0031 §5).
        self.repository
            .put_audit_event(&event)
            .await
            .map_err(|e| DecisionError::Unavailable(e.to_string()))
    }

    /// Records a failed live-state read as an audit event (no validation event).
    async fn record_read_failure(
        &self,
        context_hash: &str,
        action: &Action,
        unit: &str,
        error: &argus_executor::ServiceError,
    ) -> Result<(), DecisionError> {
        let payload =
            read_failure_event(&action.capability, unit, context_hash, &error.to_string());
        let event = DomainEvent::new(
            Uuid::new_v4(),
            EventType::new(argus_validate::VALIDATION_READ_FAILED)
                .map_err(|e| DecisionError::Invalid(e.to_string()))?,
            Utc::now(),
            "argusd",
            "argusd",
            Severity::Warning,
            Some(self.correlation_id),
            None,
            payload,
        );
        self.repository
            .put_audit_event(&event)
            .await
            .map_err(|e| DecisionError::Unavailable(e.to_string()))
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
        // Spec 003 M3 remediation capabilities (FR-016, FR-017). Registration
        // publishes and gates them; the cloud policy does not permit them.
        CapabilityId::HOST_PROCESS_SIGNAL,
        CapabilityId::CONTAINER_RESTART,
        CapabilityId::HOST_CGROUP_FREEZE,
        CapabilityId::HOST_CGROUP_THAW,
        // Spec 003 M4 Kubernetes surface (ADR-0037 §4). Registration
        // publishes and gates them; the deferred drain/reschedule stay
        // unregistered — they have no executor and no policy path.
        CapabilityId::K8S_CLUSTER_READ,
        CapabilityId::K8S_NODE_READ,
        CapabilityId::K8S_POD_READ,
        CapabilityId::K8S_DEPLOYMENT_READ,
        CapabilityId::K8S_POD_RESTART,
        CapabilityId::K8S_POD_DELETE,
        CapabilityId::K8S_DEPLOYMENT_RESTART,
        CapabilityId::K8S_DEPLOYMENT_ROLLBACK,
        CapabilityId::K8S_WORKLOAD_SCALE,
        CapabilityId::K8S_NODE_CORDON,
        CapabilityId::K8S_NODE_UNCORDON,
        CapabilityId::K8S_JOB_CLEANUP,
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
            let remediation = is_remediation_capability(&id);
            let kubernetes = is_kubernetes_capability(&id);
            let input_schema = if service {
                serde_json::json!({
                    "type": "object",
                    "required": ["unit"],
                    "properties": { "unit": { "type": "string" } },
                    "additionalProperties": false,
                })
            } else if id.as_str() == CapabilityId::HOST_PROCESS_SIGNAL {
                serde_json::json!({
                    "type": "object",
                    "required": ["pid", "signal"],
                    "properties": {
                        "pid": { "type": "integer" },
                        "signal": { "type": "string", "enum": ["term", "kill", "stop", "cont"] },
                    },
                    "additionalProperties": false,
                })
            } else if id.as_str() == CapabilityId::CONTAINER_RESTART {
                serde_json::json!({
                    "type": "object",
                    "required": ["container"],
                    "properties": { "container": { "type": "string" } },
                    "additionalProperties": false,
                })
            } else if remediation {
                serde_json::json!({
                    "type": "object",
                    "required": ["path"],
                    "properties": { "path": { "type": "string" } },
                    "additionalProperties": false,
                })
            } else if kubernetes {
                if id.as_str().ends_with(".read") {
                    serde_json::json!({
                        "type": "object",
                        "properties": {
                            "namespace": { "type": "string" },
                            "name": { "type": "string" },
                        },
                        "additionalProperties": false,
                    })
                } else if id.as_str() == CapabilityId::K8S_WORKLOAD_SCALE {
                    serde_json::json!({
                        "type": "object",
                        "required": ["name", "replicas"],
                        "properties": {
                            "namespace": { "type": "string" },
                            "name": { "type": "string" },
                            "replicas": { "type": "integer", "minimum": 0 },
                        },
                        "additionalProperties": false,
                    })
                } else {
                    serde_json::json!({
                        "type": "object",
                        "required": ["name"],
                        "properties": {
                            "namespace": { "type": "string" },
                            "name": { "type": "string" },
                            "revision": { "type": "integer" },
                        },
                        "additionalProperties": false,
                    })
                }
            } else {
                serde_json::json!({ "type": "object", "additionalProperties": false })
            };

            let (risk, reversibility, blast_radius, approval) = if service {
                (
                    RiskClass::LowRisk,
                    Reversibility::Reversible,
                    BlastRadius::Host,
                    true,
                )
            } else if id.as_str() == CapabilityId::HOST_PROCESS_SIGNAL {
                // Terminating a process is irreversible; an operator approves
                // every signal (FR-016).
                (
                    RiskClass::HighRisk,
                    Reversibility::None,
                    BlastRadius::Host,
                    true,
                )
            } else if id.as_str() == CapabilityId::CONTAINER_RESTART {
                (
                    RiskClass::Controlled,
                    Reversibility::Reversible,
                    BlastRadius::Host,
                    true,
                )
            } else if remediation {
                // The freezer is reversible (a thaw undoes it) and is the
                // autopilot's policy-approved adjustment (FR-017); the governor
                // is the gate, so no per-invocation operator approval.
                (
                    RiskClass::LowRisk,
                    Reversibility::Reversible,
                    BlastRadius::Host,
                    false,
                )
            } else if kubernetes {
                // Per contracts/capabilities.md: reads are READ with no
                // approval, the self-healing effects are CONTROLLED with a
                // per-invocation approval, and `k8s.pod.delete` is HIGH_RISK.
                // Blast radius is the cluster environment.
                if id.as_str() == CapabilityId::K8S_POD_DELETE {
                    (
                        RiskClass::HighRisk,
                        Reversibility::None,
                        BlastRadius::Environment,
                        true,
                    )
                } else if id.as_str().ends_with(".read") {
                    (
                        RiskClass::Read,
                        Reversibility::None,
                        BlastRadius::None,
                        false,
                    )
                } else {
                    (
                        RiskClass::Controlled,
                        Reversibility::Reversible,
                        BlastRadius::Environment,
                        true,
                    )
                }
            } else {
                (
                    RiskClass::Read,
                    Reversibility::None,
                    BlastRadius::None,
                    false,
                )
            };

            let base = CapabilityDescriptor::new(
                id.clone(),
                "argusd",
                id.as_str(),
                risk,
                Version::new(0, 1, 0),
                input_schema,
                serde_json::json!({}),
                reversibility,
            );

            base.with_blast_radius(blast_radius)
                .requiring_approval_if(approval)
        })
        .collect()
}

/// A refusing stand-in for the k8s executor slot until a cluster is wired;
/// every k8s effect reports Unsupported rather than reaching any API.
#[derive(Debug, Default)]
struct NoKubernetesExecutor;

impl argus_executor::Executor for NoKubernetesExecutor {
    fn execute(
        &self,
        action: &argus_executor::AuthorizedAction,
    ) -> Result<argus_executor::ExecutionResult, ExecutionError> {
        Err(ExecutionError::Unsupported(action.capability().clone()))
    }
}

fn is_service_capability(id: &CapabilityId) -> bool {
    matches!(
        id.as_str(),
        CapabilityId::HOST_SERVICE_RESTART
            | CapabilityId::HOST_SERVICE_STOP
            | CapabilityId::HOST_SERVICE_START
    )
}

/// Whether the capability belongs to the spec-003 M3 remediation surface.
fn is_remediation_capability(id: &CapabilityId) -> bool {
    matches!(
        id.as_str(),
        CapabilityId::HOST_PROCESS_SIGNAL
            | CapabilityId::CONTAINER_RESTART
            | CapabilityId::HOST_CGROUP_FREEZE
            | CapabilityId::HOST_CGROUP_THAW
    )
}

/// Whether the capability belongs to the spec-003 M4 Kubernetes surface.
fn is_kubernetes_capability(id: &CapabilityId) -> bool {
    id.as_str().starts_with("k8s.")
}

/// In-memory pending-approval store: plans paused at an approval-requiring step,
/// keyed by their single-use token.
///
/// Not persisted (ADR-0028 §6); a daemon restart drops pending plans, which is
/// fail-closed — nothing executes without its approval.
#[derive(Debug, Default)]
struct PendingApprovals {
    by_token: std::sync::Mutex<std::collections::HashMap<Uuid, PendingPlan>>,
}

impl PendingApprovals {
    fn new() -> Self {
        Self::default()
    }

    fn store(&self, pending: PendingPlan) {
        self.by_token
            .lock()
            .expect("pending store is not poisoned")
            .insert(pending.token, pending);
    }

    fn list(&self) -> Vec<PendingPlan> {
        self.by_token
            .lock()
            .expect("pending store is not poisoned")
            .values()
            .cloned()
            .collect()
    }

    fn remove(&self, token: Uuid) -> Option<PendingPlan> {
        self.by_token
            .lock()
            .expect("pending store is not poisoned")
            .remove(&token)
    }
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
impl crate::cloud::SentinelSource for Daemon {
    fn snapshot<'a>(
        &'a self,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<argus_cloud::protocol::messages::SentinelReportPayload, String>,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            let view = self.sentinel_snapshot().await;
            let view = serde_json::to_value(&view)
                .map_err(|e| format!("serialize the sentinel view: {e}"))?;

            let plans: Vec<Map<String, Value>> = self
                .list_plans()
                .await
                .unwrap_or_default()
                .into_iter()
                .rev()
                .take(10)
                .map(|(id, plan)| {
                    serde_json::json!({
                        "id": id.to_string(),
                        "objective": plan.objective,
                        "status": format!("{:?}", plan.status),
                        "confidence": plan.confidence,
                        "step_count": plan.steps.len(),
                    })
                    .as_object()
                    .cloned()
                    .unwrap_or_default()
                })
                .collect();

            let pending_approvals: Vec<Map<String, Value>> = self
                .list_pending_approvals()
                .into_iter()
                .map(|pending| {
                    serde_json::json!({
                        "token": pending.token.to_string(),
                        "objective": pending.plan.objective,
                        "context_hash": pending.context_hash,
                        "step_count": pending.plan.steps.len(),
                    })
                    .as_object()
                    .cloned()
                    .unwrap_or_default()
                })
                .collect();

            let last_cycle = self.brain_state.last_cycle().map(|record| {
                serde_json::json!({
                    "evidence": record.evidence,
                    "provider_available": record.provider_available,
                    "decision": record.decision,
                    "plan": record.plan,
                    "outcome": record.outcome,
                    "at": record.at.to_rfc3339(),
                })
                .as_object()
                .cloned()
                .unwrap_or_default()
            });

            Ok(argus_cloud::protocol::messages::SentinelReportPayload {
                view: view.as_object().cloned().unwrap_or_default(),
                last_cycle,
                plans,
                pending_approvals,
                reported_at: Utc::now(),
            })
        })
    }
}

impl crate::cloud::ApprovalSink for Daemon {
    fn decide<'a>(
        &'a self,
        token: uuid::Uuid,
        grant: bool,
        decided_by: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>
    {
        Box::pin(async move {
            if grant {
                let pending = self
                    .grant_approval(token, decided_by)
                    .map_err(|e| e.to_string())?;
                let events = argus_events::LocalEventBus::new(16);
                let outcome = self.resume_remediation(&pending, &events).await;
                Ok(match outcome {
                    ResumeOutcome::Finished(report) => format!("{:?}", report.status),
                    ResumeOutcome::Refused(why) => format!("refused: {why:?}"),
                })
            } else {
                self.deny_approval(token, decided_by)
                    .map_err(|e| e.to_string())?;
                Ok("denied".to_string())
            }
        })
    }
}

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
    fn provider_health_starts_ready_and_tracks_transitions() {
        let daemon = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async { Daemon::init(test_config()).await.unwrap() });

        let health = daemon.provider_health();
        assert_eq!(health.status, ProviderStatus::Ready);
        assert!(health.last_error.is_none());

        // A failed reasoning step degrades the provider and is reported.
        assert!(daemon.mark_provider_degraded("provider unreachable"));
        let health = daemon.provider_health();
        assert_eq!(health.status, ProviderStatus::Degraded);
        assert_eq!(health.last_error.as_deref(), Some("provider unreachable"));

        // The transition flag fires once: a repeat failure is not a new event.
        assert!(!daemon.mark_provider_degraded("still unreachable"));

        // Recovery clears the error and reports ready again.
        daemon.mark_provider_ready();
        let health = daemon.provider_health();
        assert_eq!(health.status, ProviderStatus::Ready);
        assert!(health.last_error.is_none());
    }

    #[test]
    fn provider_health_is_reported_in_status_output() {
        let daemon = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async { Daemon::init(test_config()).await.unwrap() });

        assert_eq!(daemon.status()["provider"]["status"], "ready");

        daemon.mark_provider_degraded("provider unreachable");
        let status = daemon.status();
        assert_eq!(status["provider"]["status"], "degraded");
        assert_eq!(status["provider"]["last_error"], "provider unreachable");
        assert!(
            status["provider"]["updated_at"].is_string(),
            "the condition carries a timestamp"
        );
    }

    /// A daemon against throwaway paths; no sockets are bound.
    fn test_config() -> DaemonConfig {
        DaemonConfig {
            state_path: std::env::temp_dir()
                .join(format!("argus-test-{}.db", Uuid::new_v4()))
                .display()
                .to_string(),
            ..DaemonConfig::default()
        }
    }

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

    #[test]
    fn pending_approvals_are_keyed_by_token() {
        let store = PendingApprovals::new();
        let token = Uuid::new_v4();
        let pending = PendingPlan {
            plan: Plan {
                objective: "restore nginx".into(),
                steps: vec![],
                preconditions: vec![],
                expected_outcomes: vec![],
                blast_radius: BlastRadius::Host,
                confidence: 0.9,
                status: argus_domain::PlanStatus::AwaitingApproval,
            },
            token,
            context_hash: "hash-a".into(),
            executed: vec![],
        };

        store.store(pending);
        assert_eq!(store.list().len(), 1);
        assert_eq!(
            store.remove(token).map(|p| p.context_hash),
            Some("hash-a".to_string())
        );
        assert_eq!(store.list().len(), 0, "removal drops the pending plan");
        assert!(
            store.remove(Uuid::new_v4()).is_none(),
            "an unknown token finds nothing"
        );
    }

    /// A policy that records whether it was consulted, for the ordering test.
    struct SpyPolicy {
        consulted: AtomicBool,
    }

    impl PolicyEvaluator for SpyPolicy {
        fn evaluate(&self, _request: &AuthorizationRequest) -> argus_domain::PolicyDecision {
            self.consulted.store(true, Ordering::SeqCst);
            argus_domain::PolicyDecision::allow("spy", "allow")
        }
    }

    /// An executor that records whether it was reached.
    struct SpyExecutor {
        reached: AtomicBool,
    }

    impl Executor for SpyExecutor {
        fn execute(
            &self,
            _action: &argus_executor::AuthorizedAction,
        ) -> Result<argus_executor::ExecutionResult, ExecutionError> {
            self.reached.store(true, Ordering::SeqCst);
            Ok(argus_executor::ExecutionResult {
                capability: CapabilityId::new(CapabilityId::HOST_SERVICE_RESTART).unwrap(),
                evidence: serde_json::json!({}),
                started_at: Utc::now(),
                finished_at: Utc::now(),
            })
        }
    }

    #[test]
    fn invalid_input_is_refused_before_policy_is_consulted() {
        let mut registry = CapabilityRegistry::new();
        registry
            .register(CapabilityDescriptor::new(
                CapabilityId::new(CapabilityId::HOST_SERVICE_RESTART).unwrap(),
                "argusd",
                CapabilityId::HOST_SERVICE_RESTART,
                RiskClass::LowRisk,
                Version::new(0, 1, 0),
                serde_json::json!({
                    "type": "object",
                    "required": ["unit"],
                    "properties": { "unit": { "type": "string" } },
                    "additionalProperties": false,
                }),
                serde_json::json!({}),
                Reversibility::Reversible,
            ))
            .unwrap();

        let policy = SpyPolicy {
            consulted: AtomicBool::new(false),
        };
        let executor = SpyExecutor {
            reached: AtomicBool::new(false),
        };

        // `unit` is required, so empty arguments violate the schema. The gate
        // must refuse before consulting policy or reaching the executor.
        let request = CapabilityRequest::new(
            CapabilityId::new(CapabilityId::HOST_SERVICE_RESTART).unwrap(),
            Principal::new(Some(1000), Some(1000)),
            None,
            serde_json::json!({}),
            RequestContext::new(
                Uuid::new_v4(),
                Version::new(0, 1, 0),
                Principal::new(Some(1000), Some(1000)),
                Utc::now(),
            ),
        );

        let err = authorize_and_execute_with(&registry, &policy, &executor, request).unwrap_err();
        assert!(
            matches!(err, DispatchError::InvalidInput(_)),
            "an input violating the schema must be refused before policy: {err:?}"
        );
        assert!(
            !policy.consulted.load(Ordering::SeqCst),
            "policy is not consulted for an invalid input"
        );
        assert!(
            !executor.reached.load(Ordering::SeqCst),
            "the executor is not reached for an invalid input"
        );
    }
}
