//! Cloud wiring for the daemon: the secret store and enrollment orchestration.
//!
//! Secret material lives here and nowhere else. It is never written to
//! `argus.toml`, never written to the state store, and never rendered in
//! diagnostics — the guarantees the project already makes for provider
//! credentials (`specs/001-bootstrap/research.md` § 3) extend to cloud
//! credentials unchanged.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use argus_cloud::buffer::{BufferedReport, ReportKind, ReportQueue};
use argus_cloud::client::config::classify_delivery;
use argus_cloud::client::handshake::AuthenticatedSession;
use argus_cloud::client::heartbeat::Heartbeat;
use argus_cloud::client::pairing::{EnrollmentOutcome, enroll};
use argus_cloud::client::reporting::{ReportingSchedule, collect_events, is_report_ack};
use argus_cloud::client::supervisor::{
    ConnectionOutcome, NextAction, ReconnectPolicy, SessionRequest, StopReason, TransportFactory,
    connect_once,
};
use argus_cloud::config::CloudConfig;
use argus_cloud::error::CloudError;
use argus_cloud::mapping::capability as capability_mapping;
use argus_cloud::protocol::errors::PairingDenialCode;
use argus_cloud::protocol::messages::{
    CommandInvokePayload, CommandResultPayload, CommandResultStatus, ConfigApplyPayload,
    ConfigResultPayload, ConfigResultStatus, ConfigStatePayload, ConfigurationStateEntry,
    IngestAckPayload, SessionRotatePayload, StreamThrottlePayload,
};
use argus_cloud::protocol::{Envelope, MessageType};
use argus_cloud::state::{ConnectivityTracker, EnrolledIdentity};
use argus_cloud::transport::{Transport, TransportError};
use argus_domain::{
    AppliedConfigurationState, ApplyStatus, AuthorizationRequest, CapabilityDescriptor,
    CapabilityId, CapabilityPublication, CapabilityRequest, CloudCommand, CloudCommandStatus,
    CloudConnection, CloudConnectivityState, DecisionOutcome, DomainEvent, EnvironmentId,
    EventType, ExecutionDecision, ManagedConfiguration, PolicyDecision, PolicyOutcome, Principal,
    RefusalReason, RequestContext, Severity,
};
use argus_events::LocalEventBus;
use argus_executor::{AuthorizedAction, Executor};
use argus_policy::{ApprovalStore, PolicyEvaluator};
use argus_state::{DomainRepository, RepositoryError};

use crate::config::disposition;
use crate::privileged::{PrivilegedError, PrivilegedLimiter};
use chrono::{DateTime, Utc};
use semver::Version;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use tokio::sync::{Mutex, broadcast, watch};
use uuid::Uuid;

/// File name of the cloud session credential inside the secret directory.
pub const SESSION_CREDENTIAL_FILE: &str = "cloud-session";

/// File name of the AI provider credential a cloud-managed configuration delivers.
pub const PROVIDER_CREDENTIAL_FILE: &str = "model-api-token";

/// Conventional secret directory, root-owned with mode `0600`.
pub const DEFAULT_SECRET_DIR: &str = "/etc/argus/secrets";

/// Errors from the cloud secret store.
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("secret store unavailable: {0}")]
    Unavailable(String),

    #[error("secret store operation failed: {0}")]
    Failed(String),
}

fn unavailable(error: impl std::fmt::Display) -> SecretError {
    SecretError::Unavailable(error.to_string())
}

fn failed(error: impl std::fmt::Display) -> SecretError {
    SecretError::Failed(error.to_string())
}

/// Deliberately does not derive `Debug`, and exposes its value only through
/// [`Self::expose`], so reading it is a visible act at the call site and a stray
/// `{:?}` cannot write a live credential to a log.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The value. Named for what it does: reading a secret.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// The credential issued by the cloud at enrollment.
pub type SessionCredential = Secret;

/// The AI provider credential delivered by a cloud-managed configuration.
pub type ProviderCredential = Secret;

/// Filesystem store for cloud secrets.
///
/// Writes are atomic (temp file plus rename) and always mode `0600`. A
/// pre-existing file is replaced rather than appended to, so a rotation cannot
/// leave the previous credential behind.
#[derive(Debug, Clone)]
pub struct CloudSecretStore {
    root: PathBuf,
}

impl CloudSecretStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The packaged default location.
    pub fn default_location() -> Self {
        Self::new(DEFAULT_SECRET_DIR)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn path_for(&self, file_name: &str) -> PathBuf {
        self.root.join(file_name)
    }

    fn read_secret(&self, file_name: &str) -> Result<Option<Secret>, SecretError> {
        match fs::read_to_string(self.path_for(file_name)) {
            Ok(value) => {
                let trimmed = value.trim_end_matches(['\n', '\r']);
                if trimmed.is_empty() {
                    return Ok(None);
                }
                Ok(Some(Secret::new(trimmed)))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(unavailable(error)),
        }
    }

    fn write_secret(&self, file_name: &str, secret: &Secret) -> Result<(), SecretError> {
        if secret.is_empty() {
            return Err(failed("refusing to store an empty secret"));
        }

        fs::create_dir_all(&self.root).map_err(unavailable)?;

        let temporary = self.root.join(format!(".{file_name}.incoming"));
        // Removed first so the create below cannot inherit wider permissions
        // from a leftover file, and so a previous failed write cannot be read.
        let _ = fs::remove_file(&temporary);

        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(failed)?;
        file.write_all(secret.expose().as_bytes()).map_err(failed)?;
        file.sync_all().map_err(failed)?;
        drop(file);

        fs::rename(&temporary, self.path_for(file_name)).map_err(failed)?;
        Ok(())
    }

    fn remove_secret(&self, file_name: &str) -> Result<(), SecretError> {
        match fs::remove_file(self.path_for(file_name)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(failed(error)),
        }
    }

    /// Reads the stored credential, or `None` when the installation has none.
    pub fn read_session_credential(&self) -> Result<Option<SessionCredential>, SecretError> {
        self.read_secret(SESSION_CREDENTIAL_FILE)
    }

    pub fn store_session_credential(
        &self,
        credential: &SessionCredential,
    ) -> Result<(), SecretError> {
        self.write_secret(SESSION_CREDENTIAL_FILE, credential)
    }

    /// Removes the stored credential. Idempotent: absence is success.
    pub fn remove_session_credential(&self) -> Result<(), SecretError> {
        self.remove_secret(SESSION_CREDENTIAL_FILE)
    }

    /// Reads the credential a cloud-managed configuration delivered, or `None`
    /// when none was delivered.
    pub fn read_provider_credential(&self) -> Result<Option<ProviderCredential>, SecretError> {
        self.read_secret(PROVIDER_CREDENTIAL_FILE)
    }

    pub fn store_provider_credential(
        &self,
        credential: &ProviderCredential,
    ) -> Result<(), SecretError> {
        self.write_secret(PROVIDER_CREDENTIAL_FILE, credential)
    }

    pub fn remove_provider_credential(&self) -> Result<(), SecretError> {
        self.remove_secret(PROVIDER_CREDENTIAL_FILE)
    }
}

/// Packaged location of the cloud-managed settings file.
pub const DEFAULT_SETTINGS_PATH: &str = "/etc/argus/cloud-settings.toml";

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("managed settings unavailable: {0}")]
    Unavailable(String),

    #[error("managed settings operation failed: {0}")]
    Failed(String),

    #[error("managed settings are corrupt: {0}")]
    Corrupt(String),
}

/// Cloud-managed non-secret settings, kept in their own file.
///
/// Separate from the operator's `argus.toml` for two reasons: applying a cloud
/// configuration cannot then clobber local sections the cloud does not own, and
/// "is this value cloud-managed?" is answerable from the file's presence alone,
/// which is what makes `effective_source` cheap to report honestly (FR-033).
#[derive(Debug, Clone)]
pub struct ManagedSettingsStore {
    path: PathBuf,
}

impl ManagedSettingsStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn default_location() -> Self {
        Self::new(DEFAULT_SETTINGS_PATH)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reads the applied settings, or `None` when none are applied.
    pub fn read(&self) -> Result<Option<toml::Table>, SettingsError> {
        match fs::read_to_string(&self.path) {
            Ok(text) => toml::from_str::<toml::Table>(&text)
                .map(Some)
                .map_err(|error| SettingsError::Corrupt(error.to_string())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(SettingsError::Unavailable(error.to_string())),
        }
    }

    /// Replaces the applied settings atomically, so a crash mid-write cannot
    /// leave a half-applied configuration behind.
    pub fn write(&self, settings: &toml::Table) -> Result<(), SettingsError> {
        let text =
            toml::to_string(settings).map_err(|error| SettingsError::Failed(error.to_string()))?;

        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| SettingsError::Unavailable(error.to_string()))?;
        }

        let temporary = self.path.with_extension("incoming");
        let _ = fs::remove_file(&temporary);

        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|error| SettingsError::Failed(error.to_string()))?;
        file.write_all(text.as_bytes())
            .map_err(|error| SettingsError::Failed(error.to_string()))?;
        file.sync_all()
            .map_err(|error| SettingsError::Failed(error.to_string()))?;
        drop(file);

        fs::rename(&temporary, &self.path)
            .map_err(|error| SettingsError::Failed(error.to_string()))?;
        Ok(())
    }

    /// Removes the applied settings. Idempotent: absence is success.
    pub fn clear(&self) -> Result<(), SettingsError> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(SettingsError::Failed(error.to_string())),
        }
    }
}

/// The local view of cloud connectivity an operator asks for.
///
/// Reports that a credential exists and when it was last used, never its value:
/// a diagnostic an operator pastes into a ticket must be safe to paste
/// (FR-056, FR-057).
#[derive(Debug, Clone, Serialize)]
pub struct CloudStatus {
    pub state: CloudConnectivityState,
    pub enabled: bool,
    pub endpoint_configured: bool,
    pub tenant_id: Option<Uuid>,
    pub installation_id: Option<Uuid>,
    pub instance_name: Option<String>,
    pub protocol_version: Option<String>,
    pub last_exchange_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub buffered_reports: usize,
    pub buffer_capacity: usize,
    pub dropped_reports: u64,
    pub privileged_execution_enabled: bool,
    pub remediation: Option<String>,
}

/// Collects what an operator asks for, without contacting the cloud.
pub async fn cloud_status(
    config: &CloudConfig,
    repository: &dyn DomainRepository,
    tracker: &ConnectivityTracker,
    queue: &ReportQueue,
) -> CloudStatus {
    let enrollment = repository.get_cloud_enrollment().await.ok().flatten();
    let connection = repository.get_cloud_connection().await.ok().flatten();
    let buffer = queue.stats();

    let last_exchange_at = tracker
        .last_exchange_at()
        .or_else(|| connection.as_ref().and_then(|held| held.last_exchange_at));

    CloudStatus {
        state: tracker.state(),
        enabled: config.enabled,
        endpoint_configured: config.endpoint.is_some(),
        tenant_id: enrollment.as_ref().map(|found| found.tenant_id),
        installation_id: enrollment.as_ref().map(|found| found.installation_id),
        instance_name: enrollment.as_ref().map(|found| found.instance_name.clone()),
        protocol_version: connection
            .as_ref()
            .map(|held| held.negotiated_protocol_version.clone()),
        last_exchange_at,
        last_error: tracker.last_error().map(str::to_string),
        buffered_reports: buffer.count,
        buffer_capacity: buffer.capacity,
        dropped_reports: buffer.dropped_total,
        privileged_execution_enabled: config.permits_privileged_execution(),
        remediation: tracker.remediation().map(str::to_string),
    }
}

/// Removes this installation's enrollment and every cloud-derived record.
///
/// Idempotent, and deliberately does not contact the cloud: an operator reaches
/// for this exactly when the cloud cannot be reached, or when the enrollment is
/// being discarded rather than announced (FR-006).
pub async fn forget_enrollment(
    repository: &dyn DomainRepository,
    secrets: &CloudSecretStore,
    managed_settings: &ManagedSettingsStore,
) -> Result<(), String> {
    secrets
        .remove_session_credential()
        .map_err(|error| error.to_string())?;
    secrets
        .remove_provider_credential()
        .map_err(|error| error.to_string())?;
    managed_settings
        .clear()
        .map_err(|error| error.to_string())?;
    repository
        .clear_cloud_state()
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EnrollmentError {
    #[error("this installation is already enrolled; forget the current enrollment first")]
    AlreadyEnrolled,

    #[error("cloud connectivity is not configured or is disabled")]
    NotConfigured,

    #[error("enrollment was refused by the cloud: {code}")]
    Denied {
        code: PairingDenialCode,
        remediation: &'static str,
    },

    #[error("cloud request failed: {0}")]
    Cloud(String),

    #[error("could not persist the enrollment: {0}")]
    Persistence(String),

    #[error("could not store the session credential: {0}")]
    Secret(String),
}

impl From<CloudError> for EnrollmentError {
    fn from(error: CloudError) -> Self {
        Self::Cloud(error.to_string())
    }
}

impl From<RepositoryError> for EnrollmentError {
    fn from(error: RepositoryError) -> Self {
        Self::Persistence(error.to_string())
    }
}

impl From<SecretError> for EnrollmentError {
    fn from(error: SecretError) -> Self {
        Self::Secret(error.to_string())
    }
}

/// A successful enrollment, carrying no secret so it is safe to return over IPC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EnrollmentReport {
    pub installation_id: Uuid,
    pub tenant_id: Uuid,
    pub instance_name: String,
    pub protocol_version: String,
}

pub struct EnrollmentRequest<'a> {
    pub code: &'a str,
    pub hostname: &'a str,
    pub agent_version: &'a str,
    pub now: DateTime<Utc>,
}

/// Enrolls the installation with the cloud.
///
/// Refuses when an enrollment already exists instead of rebinding the
/// installation to a different organization: silently moving a host between
/// tenants would be a tenant-isolation hazard. The operator must forget the
/// current enrollment first.
pub async fn enroll_installation(
    config: &CloudConfig,
    secrets: &CloudSecretStore,
    repository: &dyn DomainRepository,
    transport: &mut dyn Transport,
    request: EnrollmentRequest<'_>,
) -> Result<EnrollmentReport, EnrollmentError> {
    let EnrollmentRequest {
        code,
        hostname,
        agent_version,
        now,
    } = request;
    if !config.is_active() {
        return Err(EnrollmentError::NotConfigured);
    }
    if repository.get_cloud_enrollment().await?.is_some() {
        return Err(EnrollmentError::AlreadyEnrolled);
    }

    let outcome = enroll(
        transport,
        code,
        hostname,
        agent_version,
        &config.expected_cloud_id,
    )
    .await?;

    match outcome {
        EnrollmentOutcome::Denied { code, remediation } => {
            Err(EnrollmentError::Denied { code, remediation })
        }
        EnrollmentOutcome::Granted(payload) => {
            let identity = EnrolledIdentity::from_granted(&payload, now);
            repository
                .save_cloud_enrollment(&identity.to_enrollment())
                .await?;
            secrets
                .store_session_credential(&SessionCredential::new(payload.session_token.clone()))?;
            Ok(EnrollmentReport {
                installation_id: identity.installation_id,
                tenant_id: identity.tenant_id,
                instance_name: identity.instance_name.clone(),
                protocol_version: identity.protocol_version.clone(),
            })
        }
    }
}

pub struct SupervisorDeps {
    pub config: CloudConfig,
    pub secrets: CloudSecretStore,
    pub repository: Arc<dyn DomainRepository>,
    pub tracker: Arc<Mutex<ConnectivityTracker>>,
    /// Injected so supervision is exercisable without a real network.
    pub factory: Arc<dyn TransportFactory>,
    /// Drained into the report queue on every session tick (T054).
    pub events: Arc<LocalEventBus>,
    /// Undelivered reports; shared so IPC can surface occupancy (T048).
    pub queue: Arc<Mutex<ReportQueue>>,
    /// The capability surface to publish on connect (T062).
    ///
    /// A snapshot taken at startup: the bootstrap registers no plugins, so the
    /// surface cannot change while running. Live change detection arrives with
    /// the plugin manager.
    pub capabilities: Arc<Vec<CapabilityDescriptor>>,
    /// Where a cloud-managed configuration is applied (T069).
    pub managed_settings: Arc<ManagedSettingsStore>,
    /// The installation identity a cloud-issued command is recorded against.
    pub environment_id: EnvironmentId,
    /// The local authorization boundary every cloud invocation must cross.
    pub policy: Arc<dyn PolicyEvaluator>,
    /// Where a permitted invocation is handed for execution.
    pub executor: Arc<dyn Executor>,
    /// Approvals granted locally for individual invocations.
    pub approvals: Arc<ApprovalStore>,
    /// Bounds concurrency, serialization, and timeout for privileged work.
    pub limiter: Arc<PrivilegedLimiter>,
}

/// Reported as the negotiated version until a session supplies the real one.
const CONNECTION_PROTOCOL_LABEL: &str = "1.0.0";

/// How long to wait before re-checking prerequisites that need operator action.
const RECHECK: Duration = Duration::from_secs(30);

/// How often the session loop wakes to consider a heartbeat or a stop request.
const SESSION_TICK: Duration = Duration::from_secs(1);

/// Runs the cloud connection loop until `stop` is signalled.
///
/// Meant to be spawned as a task: the daemon must never wait on the cloud to
/// start, so an absent, unreachable, or misconfigured cloud delays nothing
/// (FR-013). Every failure path either backs off or stops deliberately; none of
/// them blocks local operation.
pub async fn supervise(deps: SupervisorDeps, mut stop: watch::Receiver<bool>) {
    let mut policy = ReconnectPolicy::new(
        Duration::from_secs(deps.config.reconnect_base_seconds.max(1)),
        Duration::from_secs(deps.config.reconnect_max_seconds.max(1)),
    );
    let mut event_rx = deps.events.subscribe();

    loop {
        if *stop.borrow() {
            return;
        }

        let Some(endpoint) = active_endpoint(&deps).await else {
            if wait_or_stop(&mut stop, RECHECK).await {
                return;
            }
            continue;
        };

        let Some(identity) = enrolled_identity(&deps).await else {
            if wait_or_stop(&mut stop, RECHECK).await {
                return;
            }
            continue;
        };

        let Some(credential) = session_credential(&deps).await else {
            if wait_or_stop(&mut stop, RECHECK).await {
                return;
            }
            continue;
        };

        set_state(&deps, CloudConnectivityState::Connecting, None).await;

        let outcome = match connect_once(
            deps.factory.as_ref(),
            SessionRequest {
                endpoint: &endpoint,
                expected_cloud_id: &deps.config.expected_cloud_id,
                identity: &identity,
                session_proof: credential.expose(),
                hostname: &hostname(),
                agent_version: env!("CARGO_PKG_VERSION"),
                capability_schema_version: None,
            },
        )
        .await
        {
            Ok((mut transport, session)) => {
                policy.note_connected();
                set_state(&deps, CloudConnectivityState::Connected, None).await;
                mark_exchange(&deps).await;
                run_session(
                    &deps,
                    transport.as_mut(),
                    &session,
                    &mut event_rx,
                    &mut stop,
                )
                .await
            }
            Err(outcome) => outcome,
        };

        match policy.after(outcome.clone()) {
            NextAction::Stop(reason) => {
                let (state, detail) = stop_outcome(&reason);
                set_state(&deps, state, Some(detail.to_string())).await;
                return;
            }
            NextAction::Reconnect(delay) => {
                set_state(
                    &deps,
                    CloudConnectivityState::Disconnected,
                    Some(describe(&outcome)),
                )
                .await;
                if wait_or_stop(&mut stop, delay).await {
                    return;
                }
            }
        }
    }
}

async fn active_endpoint(deps: &SupervisorDeps) -> Option<String> {
    match deps.config.endpoint.clone() {
        Some(endpoint) if deps.config.is_active() => Some(endpoint),
        _ => {
            set_state(deps, CloudConnectivityState::NotConfigured, None).await;
            None
        }
    }
}

async fn enrolled_identity(deps: &SupervisorDeps) -> Option<EnrolledIdentity> {
    match deps.repository.get_cloud_enrollment().await {
        Ok(Some(enrollment)) => Some(EnrolledIdentity::from_enrollment(
            &enrollment,
            CONNECTION_PROTOCOL_LABEL,
        )),
        Ok(None) => {
            set_state(deps, CloudConnectivityState::NotConfigured, None).await;
            None
        }
        Err(error) => {
            set_state(
                deps,
                CloudConnectivityState::Degraded,
                Some(error.to_string()),
            )
            .await;
            None
        }
    }
}

async fn session_credential(deps: &SupervisorDeps) -> Option<SessionCredential> {
    match deps.secrets.read_session_credential() {
        Ok(Some(credential)) => Some(credential),
        Ok(None) => {
            set_state(
                deps,
                CloudConnectivityState::Disconnected,
                Some("the session credential is missing; re-enrollment is required".to_string()),
            )
            .await;
            None
        }
        Err(error) => {
            set_state(
                deps,
                CloudConnectivityState::Degraded,
                Some(error.to_string()),
            )
            .await;
            None
        }
    }
}

const MANAGED_PROVIDER_SETTINGS: [&str; 4] = ["provider", "model", "fallback_models", "base_url"];

/// The provider credential key, which is stored as a secret rather than a setting.
const MANAGED_PROVIDER_SECRET: &str = "api_token";

/// Splits a provider configuration into settings and an optional credential.
///
/// Validation happens entirely before anything is written, and an unrecognised
/// field is named in the error, so a rejection explains exactly what could not be
/// honoured instead of silently applying part of a configuration.
fn split_provider_content(
    content: &serde_json::Map<String, serde_json::Value>,
) -> Result<(toml::Table, Option<ProviderCredential>), String> {
    let mut settings = toml::Table::new();
    let mut credential = None;

    for (key, value) in content {
        match key.as_str() {
            // `kind` is envelope metadata; the caller already consumed it.
            "kind" => continue,
            MANAGED_PROVIDER_SECRET => {
                let Some(text) = value.as_str() else {
                    return Err(format!("'{key}' must be a string"));
                };
                credential = Some(ProviderCredential::new(text));
            }
            known if MANAGED_PROVIDER_SETTINGS.contains(&known) => {
                settings.insert(key.clone(), json_to_toml(value)?);
            }
            unknown => return Err(format!("unrecognised configuration field '{unknown}'")),
        }
    }

    Ok((settings, credential))
}

fn json_to_toml(value: &serde_json::Value) -> Result<toml::Value, String> {
    match value {
        serde_json::Value::String(text) => Ok(toml::Value::String(text.clone())),
        serde_json::Value::Bool(flag) => Ok(toml::Value::Boolean(*flag)),
        serde_json::Value::Number(number) => number
            .as_i64()
            .map(toml::Value::Integer)
            .or_else(|| number.as_f64().map(toml::Value::Float))
            .ok_or_else(|| "unsupported number".to_string()),
        serde_json::Value::Array(items) => items
            .iter()
            .map(json_to_toml)
            .collect::<Result<Vec<_>, _>>()
            .map(toml::Value::Array),
        serde_json::Value::Null => Err("null is not a valid setting value".to_string()),
        serde_json::Value::Object(_) => {
            Err("nested tables are not supported for provider settings".to_string())
        }
    }
}

/// Applies a delivered configuration and reports the outcome.
///
/// Disposition, ordering, and content are all decided before anything is
/// written, so a configuration is never partially applied: either every managed
/// field is honoured, or the previously applied configuration stays in effect
/// untouched (FR-028, FR-036).
async fn apply_configuration(deps: &SupervisorDeps, transport: &dyn Transport, frame: &Envelope) {
    let correlation = frame.correlation_id.or(Some(frame.message_id));

    let Ok(payload) = serde_json::from_value::<ConfigApplyPayload>(frame.payload.clone()) else {
        reply_config_result(
            transport,
            correlation,
            Uuid::nil(),
            ConfigResultStatus::Failed,
            Some("the delivered configuration was malformed".to_string()),
        )
        .await;
        return;
    };

    let version_id = payload.version_id;

    let Some(kind) = payload.content.get("kind").and_then(|value| value.as_str()) else {
        reply_config_result(
            transport,
            correlation,
            version_id,
            ConfigResultStatus::Rejected,
            Some("the configuration does not declare its kind".to_string()),
        )
        .await;
        return;
    };

    if let Some(reason) = disposition(kind).refusal_reason() {
        reply_config_result(
            transport,
            correlation,
            version_id,
            ConfigResultStatus::Rejected,
            Some(reason.to_string()),
        )
        .await;
        return;
    }

    let held = deps
        .repository
        .get_managed_configuration(payload.configuration_id)
        .await
        .ok()
        .flatten()
        .map(|held| held.current_version_number)
        .unwrap_or(0);

    if !classify_delivery(held, payload.version_number, payload.mode).changes_held_version() {
        reply_config_result(
            transport,
            correlation,
            version_id,
            ConfigResultStatus::Rejected,
            Some(format!(
                "version {} is not newer than the version held ({held})",
                payload.version_number
            )),
        )
        .await;
        return;
    }

    let (settings, credential) = match split_provider_content(&payload.content) {
        Ok(parts) => parts,
        Err(reason) => {
            reply_config_result(
                transport,
                correlation,
                version_id,
                ConfigResultStatus::Rejected,
                Some(reason),
            )
            .await;
            return;
        }
    };

    // Credential first: managed settings must never point at a credential that
    // is not there yet.
    if let Some(credential) = credential
        && let Err(error) = deps.secrets.store_provider_credential(&credential)
    {
        reply_config_result(
            transport,
            correlation,
            version_id,
            ConfigResultStatus::Failed,
            Some(format!("could not store the provider credential: {error}")),
        )
        .await;
        return;
    }

    if let Err(error) = deps.managed_settings.write(&settings) {
        reply_config_result(
            transport,
            correlation,
            version_id,
            ConfigResultStatus::Failed,
            Some(format!("could not write the managed settings: {error}")),
        )
        .await;
        return;
    }

    let applied_at = Utc::now();

    let _ = deps
        .repository
        .save_managed_configuration(&ManagedConfiguration {
            configuration_id: payload.configuration_id,
            kind: kind.to_string(),
            current_version_id: Some(version_id),
            current_version_number: payload.version_number,
            content_hash: Some(payload.content_hash.clone()),
            applied_at: Some(applied_at),
            apply_status: ApplyStatus::Applied,
            apply_reason: None,
        })
        .await;

    let _ = deps
        .repository
        .save_applied_configuration(&AppliedConfigurationState {
            configuration_id: payload.configuration_id,
            applied_version_id: Some(version_id),
            applied_version_number: payload.version_number,
            content_hash: Some(payload.content_hash.clone()),
            updated_at: applied_at,
        })
        .await;

    reply_config_result(
        transport,
        correlation,
        version_id,
        ConfigResultStatus::Applied,
        None,
    )
    .await;
}

async fn reply_config_result(
    transport: &dyn Transport,
    correlation_id: Option<Uuid>,
    version_id: Uuid,
    status: ConfigResultStatus,
    reason: Option<String>,
) {
    let payload = ConfigResultPayload {
        version_id,
        status,
        reason,
        applied_at: (status == ConfigResultStatus::Applied).then(Utc::now),
    };
    let Ok(value) = serde_json::to_value(&payload) else {
        return;
    };
    let envelope = Envelope::new(MessageType::ConfigResult, value, correlation_id);
    let _ = transport.send(&envelope).await;
}

/// Adopts a credential the cloud rotated.
///
/// The store replaces the previous credential atomically and never appends, so
/// there is no window where both the retired and the new credential are usable
/// (FR-046, FR-053). An already-expired rotation is ignored rather than
/// installed: adopting it would leave the installation holding a credential the
/// cloud no longer honours.
async fn rotate_session_credential(deps: &SupervisorDeps, frame: &Envelope) {
    let Ok(payload) = serde_json::from_value::<SessionRotatePayload>(frame.payload.clone()) else {
        return;
    };

    if payload.expires_at <= Utc::now() {
        return;
    }

    let _ = deps
        .secrets
        .store_session_credential(&SessionCredential::new(payload.rotation_token));
}

/// Schema version reported alongside the published capability surface.
const CAPABILITY_SCHEMA_VERSION: &str = "0.1.0";

/// Publishes the capability surface, unless the cloud already holds this exact
/// one.
///
/// The no-op check is by content hash, so a reconnection does not re-send a
/// surface the cloud already has. An empty surface is still published: the cloud
/// must be able to tell "this installation can do nothing" from "this
/// installation has not said".
async fn publish_capabilities(
    deps: &SupervisorDeps,
    transport: &dyn Transport,
) -> Result<(), TransportError> {
    let descriptors = deps.capabilities.as_ref();

    if let Ok(Some(previous)) = deps.repository.get_capability_publication().await
        && previous.is_noop_for(descriptors)
    {
        return Ok(());
    }

    let publication =
        CapabilityPublication::new(CAPABILITY_SCHEMA_VERSION, descriptors.clone(), Utc::now());

    let payload = capability_mapping::publication(&publication.version, descriptors, |_| false)
        .map_err(|error| TransportError::Send(error.to_string()))?;

    let value =
        serde_json::to_value(&payload).map_err(|error| TransportError::Send(error.to_string()))?;
    let envelope = Envelope::new(MessageType::CapabilitiesPublish, value, None);
    transport.send(&envelope).await?;

    let _ = deps
        .repository
        .save_capability_publication(&publication)
        .await;
    Ok(())
}

/// Answers `config.pull.request` with what this installation genuinely holds.
///
/// This is what makes an interrupted deployment determinate: the cloud asks, and
/// the answer is the applied state, never the state that was merely delivered
/// (FR-029, FR-032). A configuration with nothing applied is reported with a
/// null version rather than omitted, so "nothing" is distinguishable from
/// "unknown".
async fn send_config_state(
    deps: &SupervisorDeps,
    transport: &dyn Transport,
    correlation_id: Option<Uuid>,
) {
    let applied = deps
        .repository
        .list_applied_configurations()
        .await
        .unwrap_or_default();

    let configurations = applied
        .into_iter()
        .map(|state| ConfigurationStateEntry {
            configuration_id: state.configuration_id,
            applied_version_id: state.applied_version_id,
        })
        .collect();

    let payload = ConfigStatePayload { configurations };
    let Ok(value) = serde_json::to_value(&payload) else {
        return;
    };

    let envelope = Envelope::new(MessageType::ConfigState, value, correlation_id);
    let _ = transport.send(&envelope).await;
}

/// One cloud-issued invocation, from validation to reporting.
///
/// The checks run in the fixed, short-circuiting order of
/// `contracts/privileged-execution.md` §2. Checks 2–4 precede any authorization
/// work, so a malformed or non-executable request never consumes a policy
/// evaluation, and check 1 precedes everything, so a re-delivered command cannot
/// re-trigger a side effect (ADR-0022 §2, §3).
async fn handle_command_invoke(deps: &SupervisorDeps, transport: &dyn Transport, frame: &Envelope) {
    let correlation = frame.correlation_id.or(Some(frame.message_id));

    let Ok(payload) = serde_json::from_value::<CommandInvokePayload>(frame.payload.clone()) else {
        return;
    };
    let command_id = payload.command_id;

    // Check 1: idempotency keys on `command_id`. A terminal command re-reports the
    // result it already holds; a duplicate that arrives while the first attempt is
    // still running is suppressed, because no result exists yet and emitting one
    // would be a fabrication.
    if let Ok(Some(existing)) = deps.repository.get_cloud_command(command_id).await {
        if existing.status.is_terminal() {
            report_stored_result(transport, &existing, correlation).await;
        }
        return;
    }

    // Check 2: the capability must be part of the published surface.
    let Some(descriptor) = deps
        .capabilities
        .iter()
        .find(|candidate| candidate.id().as_str() == payload.capability_id)
    else {
        refuse_command(
            deps,
            transport,
            &payload,
            correlation,
            RefusalReason::UnknownCapability,
        )
        .await;
        return;
    };

    // Check 3: the input must match the capability's declared schema.
    let input = Value::Object(payload.input.clone());
    if !input_matches(descriptor.input_schema(), &input) {
        refuse_command(
            deps,
            transport,
            &payload,
            correlation,
            RefusalReason::InvalidInput,
        )
        .await;
        return;
    }

    // Check 4: the local kill switch. It is read from local configuration only, so
    // no cloud message can reach it.
    if descriptor.changes_the_host() && !deps.config.permits_privileged_execution() {
        refuse_command(
            deps,
            transport,
            &payload,
            correlation,
            RefusalReason::ExecutionDisabled,
        )
        .await;
        return;
    }

    // Check 5: authorization, derived from the capability and evaluated locally.
    // The principal is the unattributed cloud caller, which carries no authority.
    let authorization = AuthorizationRequest::new(
        CapabilityRequest::new(
            descriptor.id().clone(),
            Principal::cloud(),
            None,
            input,
            RequestContext::new(
                command_id,
                Version::new(0, 1, 0),
                Principal::cloud(),
                Utc::now(),
            ),
        ),
        descriptor.risk_class(),
        descriptor.effective_blast_radius(),
    )
    .requiring_approval(descriptor.requires_approval());

    let decision = deps.policy.evaluate(&authorization);
    record_execution_decision(deps, command_id, descriptor, &decision).await;

    if decision.outcome == PolicyOutcome::Deny {
        refuse_command(
            deps,
            transport,
            &payload,
            correlation,
            RefusalReason::PolicyDenied,
        )
        .await;
        return;
    }

    // Check 6: an approval-requiring invocation needs a valid, unexpired approval.
    let decision = if decision.outcome == PolicyOutcome::RequireApproval {
        if !deps.approvals.authorizes(command_id, Utc::now()) {
            refuse_command(
                deps,
                transport,
                &payload,
                correlation,
                RefusalReason::ApprovalRequired,
            )
            .await;
            return;
        }

        // The approval is exactly what the policy was waiting for, so the
        // invocation is authorized now. The executor still only ever sees `Allow`,
        // so the structural gate is preserved rather than bypassed.
        PolicyDecision::allow(
            decision.policy_id.clone(),
            format!("{} (approved locally)", decision.reason),
        )
    } else {
        decision
    };

    // Check 7: execute under the concurrency cap, the resource lock, and a timeout.
    let Ok(action) = AuthorizedAction::new(authorization.capability_request, decision) else {
        refuse_command(
            deps,
            transport,
            &payload,
            correlation,
            RefusalReason::PolicyDenied,
        )
        .await;
        return;
    };

    let resource = action
        .request()
        .resource
        .as_ref()
        .map(|resource| resource.as_str().to_string())
        .or_else(|| {
            action
                .request()
                .arguments
                .get("unit")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| descriptor.id().as_str().to_string());

    let mut command = CloudCommand::received(
        command_id,
        deps.environment_id,
        descriptor.id().clone(),
        Value::Object(payload.input.clone()),
        payload.correlation_id.clone(),
        Utc::now(),
    );

    // The row is committed as `executing` before the operation begins. A crash
    // between "operation started" and "row written" would otherwise erase all
    // trace of a privileged change that may already have taken effect (ADR-0022 §1).
    command.status = CloudCommandStatus::Executing;
    let _ = deps.repository.put_cloud_command(&command).await;

    let executor = Arc::clone(&deps.executor);
    let timeout_budget = Duration::from_secs(
        descriptor
            .timeout_seconds()
            .unwrap_or(deps.config.request_timeout_seconds),
    );
    let queue_budget = Duration::from_secs(deps.config.request_timeout_seconds);

    let outcome = deps
        .limiter
        .run(&resource, queue_budget, timeout_budget, async move {
            executor.execute(&action)
        })
        .await;

    let (status, result, reason) = match outcome {
        Ok(Ok(execution)) => (
            CommandResultStatus::Acknowledged,
            Some(execution.evidence),
            None,
        ),
        Ok(Err(error)) => (CommandResultStatus::Failed, None, Some(error.to_string())),
        Err(PrivilegedError::TimedOut) => (
            CommandResultStatus::Failed,
            None,
            Some(
                "the operation exceeded its timeout and its outcome is unknown; verify it manually"
                    .to_string(),
            ),
        ),
        Err(PrivilegedError::QueueBudgetExpired) => (
            CommandResultStatus::Failed,
            None,
            Some("the operation waited past its queue budget and never ran".to_string()),
        ),
    };

    command.status = match status {
        CommandResultStatus::Acknowledged => CloudCommandStatus::Acknowledged,
        CommandResultStatus::Refused => CloudCommandStatus::Refused,
        CommandResultStatus::Failed => CloudCommandStatus::Failed,
    };
    command.result = result.clone();
    command.completed_at = Some(Utc::now());
    let _ = deps.repository.put_cloud_command(&command).await;

    record_command_audit(
        deps,
        command_id,
        descriptor.id().as_str(),
        match status {
            CommandResultStatus::Acknowledged => "acknowledged",
            CommandResultStatus::Refused => "refused",
            CommandResultStatus::Failed => "failed",
        },
        reason.as_deref(),
        command.result.as_ref(),
    )
    .await;

    send_command_result(transport, command_id, status, result, reason, correlation).await;
}

/// Records a refusal, persists the command in its terminal refused state, and
/// reports it to the cloud with a secret-free reason.
async fn refuse_command(
    deps: &SupervisorDeps,
    transport: &dyn Transport,
    payload: &CommandInvokePayload,
    correlation: Option<Uuid>,
    reason: RefusalReason,
) {
    let Ok(capability) = CapabilityId::new(&payload.capability_id) else {
        send_command_result(
            transport,
            payload.command_id,
            CommandResultStatus::Refused,
            None,
            Some(RefusalReason::UnknownCapability.describe().to_string()),
            correlation,
        )
        .await;
        return;
    };

    let mut command = CloudCommand::received(
        payload.command_id,
        deps.environment_id,
        capability,
        Value::Object(payload.input.clone()),
        payload.correlation_id.clone(),
        Utc::now(),
    );
    command.refuse(reason, Utc::now());
    let _ = deps.repository.put_cloud_command(&command).await;

    record_command_audit(
        deps,
        payload.command_id,
        &payload.capability_id,
        "refused",
        Some(reason.describe()),
        None,
    )
    .await;

    send_command_result(
        transport,
        payload.command_id,
        CommandResultStatus::Refused,
        None,
        Some(reason.describe().to_string()),
        correlation,
    )
    .await;
}

/// Re-reports the stored result of a command that already reached a terminal
/// state, without executing it again.
async fn report_stored_result(
    transport: &dyn Transport,
    command: &CloudCommand,
    correlation: Option<Uuid>,
) {
    let status = match command.status {
        CloudCommandStatus::Acknowledged => CommandResultStatus::Acknowledged,
        CloudCommandStatus::Refused => CommandResultStatus::Refused,
        CloudCommandStatus::Failed => CommandResultStatus::Failed,
        CloudCommandStatus::Received | CloudCommandStatus::Executing => return,
    };

    let reason = command
        .refusal_reason
        .map(|reason| reason.describe().to_string());

    send_command_result(
        transport,
        command.command_id,
        status,
        command.result.clone(),
        reason,
        correlation,
    )
    .await;
}

async fn send_command_result(
    transport: &dyn Transport,
    command_id: Uuid,
    status: CommandResultStatus,
    result: Option<Value>,
    reason: Option<String>,
    correlation: Option<Uuid>,
) {
    let payload = CommandResultPayload {
        command_id,
        status,
        result: result.and_then(|value| value.as_object().cloned()),
        reason,
    };
    let Ok(value) = serde_json::to_value(&payload) else {
        return;
    };
    let envelope = Envelope::new(MessageType::CommandResult, value, correlation);
    let _ = transport.send(&envelope).await;
}

/// Persists the installation's authorization verdict for an invocation.
///
/// The decision is recorded whether it allowed or denied, so a denied invocation
/// is as auditable as an executed one (FR-042, SC-016).
async fn record_execution_decision(
    deps: &SupervisorDeps,
    command_id: Uuid,
    descriptor: &CapabilityDescriptor,
    decision: &PolicyDecision,
) {
    let outcome = match decision.outcome {
        PolicyOutcome::Allow => DecisionOutcome::Permitted,
        PolicyOutcome::Deny => DecisionOutcome::Denied,
        PolicyOutcome::RequireApproval => DecisionOutcome::RequireApproval,
    };

    let _ = deps
        .repository
        .put_execution_decision(&ExecutionDecision {
            decision_id: Uuid::new_v4(),
            command_id,
            outcome,
            risk_class: descriptor.risk_class(),
            blast_radius: descriptor.effective_blast_radius(),
            policy_id: decision.policy_id.clone(),
            reason: decision.reason.clone(),
            decided_at: decision.decided_at,
        })
        .await;
}

/// Records an invocation in the audit trail on the same basis as a local
/// operation, so cloud-issued and local actions are auditable on identical terms
/// (FR-042, SC-016).
async fn record_command_audit(
    deps: &SupervisorDeps,
    command_id: Uuid,
    capability: &str,
    outcome: &str,
    detail: Option<&str>,
    evidence: Option<&Value>,
) {
    let severity = if outcome == "acknowledged" {
        Severity::Info
    } else {
        Severity::Warning
    };

    let event = DomainEvent::new(
        Uuid::new_v4(),
        EventType::new("cloud.command.outcome").expect("valid event type"),
        Utc::now(),
        "argusd",
        capability,
        severity,
        Some(command_id),
        None,
        serde_json::json!({
            "command_id": command_id.to_string(),
            "outcome": outcome,
            "detail": detail,
            "evidence": evidence,
        }),
    );

    let _ = deps.repository.put_audit_event(&event).await;
}

/// Settles commands left unfinished by a previous run.
///
/// A row found in `executing` has an unknown real-world outcome, so it becomes
/// `failed` with an explicit unknown-state reason rather than being dropped or
/// silently retried: the operator is told a privileged change may or may not have
/// happened (ADR-0022 §2). A row still `received` never started and is likewise
/// settled, so a re-delivery is not mistaken for an in-flight duplicate.
async fn resolve_interrupted_commands(deps: &SupervisorDeps, transport: &dyn Transport) {
    let Ok(unfinished) = deps.repository.list_unfinished_cloud_commands().await else {
        return;
    };

    for mut command in unfinished {
        let reason = match command.status {
            CloudCommandStatus::Executing => {
                "the daemon restarted while this operation was executing; its outcome is unknown and must be verified manually"
            }
            _ => "the daemon restarted before this operation was executed",
        };

        command.status = CloudCommandStatus::Failed;
        command.completed_at = Some(Utc::now());
        let _ = deps.repository.put_cloud_command(&command).await;

        send_command_result(
            transport,
            command.command_id,
            CommandResultStatus::Failed,
            None,
            Some(reason.to_string()),
            None,
        )
        .await;
    }
}

/// Whether `input` satisfies the subset of JSON Schema the capability declarations
/// in this project use.
///
/// An unknown or absent constraint is treated as satisfied, and the
/// `additionalProperties: false` rule is respected so a typo in an invocation's
/// arguments cannot slip through as an accepted input.
fn input_matches(schema: &Value, input: &Value) -> bool {
    let Some(schema) = schema.as_object() else {
        return true;
    };

    if let Some(kind) = schema.get("type").and_then(Value::as_str) {
        let matches_kind = match kind {
            "object" => input.is_object(),
            "array" => input.is_array(),
            "string" => input.is_string(),
            "number" | "integer" => input.is_number(),
            "boolean" => input.is_boolean(),
            "null" => input.is_null(),
            _ => true,
        };
        if !matches_kind {
            return false;
        }
    }

    let Some(input) = input.as_object() else {
        return true;
    };

    if let Some(required) = schema.get("required").and_then(Value::as_array)
        && !required
            .iter()
            .filter_map(Value::as_str)
            .all(|key| input.contains_key(key))
    {
        return false;
    }

    if schema.get("additionalProperties").and_then(Value::as_bool) == Some(false) {
        let properties = schema.get("properties").and_then(Value::as_object);
        let permitted =
            |key: &String| properties.is_some_and(|declared| declared.contains_key(key));
        if !input.keys().all(permitted) {
            return false;
        }
    }

    true
}

async fn run_session(
    deps: &SupervisorDeps,
    transport: &mut dyn Transport,
    session: &AuthenticatedSession,
    event_rx: &mut broadcast::Receiver<DomainEvent>,
    stop: &mut watch::Receiver<bool>,
) -> ConnectionOutcome {
    let mut heartbeat = Heartbeat::new(session.heartbeat_interval_seconds, Instant::now());
    let mut schedule =
        ReportingSchedule::new(deps.config.telemetry_interval_seconds, Instant::now());
    let mut in_flight: HashMap<Uuid, BufferedReport> = HashMap::new();
    let mut ticker = tokio::time::interval(SESSION_TICK);

    if let Err(error) = publish_capabilities(deps, transport).await {
        return ConnectionOutcome::TransportFailed(error.to_string());
    }

    // Settle anything a previous run left unfinished before reporting anything
    // else, so a privileged change of unknown outcome is never silently dropped.
    resolve_interrupted_commands(deps, transport).await;

    // The cloud asks for reconciliation at handshake when it wants it, so an
    // interrupted deployment is settled without the operator doing anything.
    if session.config_pull_required {
        send_config_state(deps, transport, None).await;
    }

    loop {
        tokio::select! {
            _ = stop.changed() => {
                requeue_in_flight(deps, &mut in_flight).await;
                return ConnectionOutcome::Stopped;
            }
            _ = ticker.tick() => {
                let now = Instant::now();

                if heartbeat.is_overdue(now) {
                    requeue_in_flight(deps, &mut in_flight).await;
                    return ConnectionOutcome::TransportFailed(
                        "the cloud stopped answering heartbeats".to_string(),
                    );
                }

                collect_bus_events(deps, event_rx).await;

                if schedule.telemetry_due(now) {
                    if let Err(error) = flush_reports(deps, transport, &mut in_flight).await {
                        requeue_in_flight(deps, &mut in_flight).await;
                        return ConnectionOutcome::TransportFailed(error.to_string());
                    }
                    schedule.note_telemetry_sent(now);
                }

                if heartbeat.ping_due(now) {
                    let ping = Envelope::new(MessageType::Ping, serde_json::json!({}), None);
                    if let Err(error) = transport.send(&ping).await {
                        requeue_in_flight(deps, &mut in_flight).await;
                        return ConnectionOutcome::TransportFailed(error.to_string());
                    }
                    heartbeat.note_seen(now);
                }
            }
            received = transport.recv() => match received {
                Ok(Some(frame)) => {
                    heartbeat.note_seen(Instant::now());
                    handle_inbound(deps, transport, frame, &mut in_flight, &mut schedule).await;
                    mark_exchange(deps).await;
                }
                Ok(None) => {
                    requeue_in_flight(deps, &mut in_flight).await;
                    return ConnectionOutcome::SessionEnded;
                }
                Err(error) => {
                    requeue_in_flight(deps, &mut in_flight).await;
                    return ConnectionOutcome::TransportFailed(error.to_string());
                }
            },
        }
    }
}

async fn collect_bus_events(
    deps: &SupervisorDeps,
    event_rx: &mut broadcast::Receiver<DomainEvent>,
) {
    let mut queue = deps.queue.lock().await;
    collect_events(event_rx, &mut queue, Utc::now());
}

fn message_type_for(kind: ReportKind) -> MessageType {
    match kind {
        ReportKind::Telemetry => MessageType::TelemetryReport,
        ReportKind::Health => MessageType::HealthReport,
        ReportKind::Events => MessageType::EventsReport,
        ReportKind::Activities => MessageType::ActivitiesReport,
    }
}

/// Sends every queued report, holding each one in flight until the cloud
/// acknowledges it.
///
/// Nothing is dropped from local custody on send alone: a connection that dies
/// straight after a write would otherwise lose the report silently.
async fn flush_reports(
    deps: &SupervisorDeps,
    transport: &dyn Transport,
    in_flight: &mut HashMap<Uuid, BufferedReport>,
) -> Result<(), TransportError> {
    let entries = {
        let mut queue = deps.queue.lock().await;
        queue.drain(usize::MAX, Utc::now())
    };

    for entry in entries {
        let envelope = Envelope::new(message_type_for(entry.kind), entry.payload.clone(), None);
        let message_id = envelope.message_id;
        transport.send(&envelope).await?;
        in_flight.insert(message_id, entry);
    }

    Ok(())
}

/// Returns in-flight reports to the queue, oldest first, so a failed or ended
/// session loses nothing.
async fn requeue_in_flight(deps: &SupervisorDeps, in_flight: &mut HashMap<Uuid, BufferedReport>) {
    if in_flight.is_empty() {
        return;
    }

    let mut entries: Vec<BufferedReport> = in_flight.drain().map(|(_, entry)| entry).collect();
    entries.sort_by_key(|entry| entry.enqueued_at);
    deps.queue.lock().await.requeue_front(entries);
}

async fn handle_inbound(
    deps: &SupervisorDeps,
    transport: &dyn Transport,
    frame: Envelope,
    in_flight: &mut HashMap<Uuid, BufferedReport>,
    schedule: &mut ReportingSchedule,
) {
    match frame.kind() {
        Some(MessageType::Ping) => {
            let pong = Envelope::new(
                MessageType::Pong,
                serde_json::json!({}),
                frame.correlation_id.or(Some(frame.message_id)),
            );
            let _ = transport.send(&pong).await;
        }
        Some(MessageType::StreamThrottle) => {
            if let Ok(payload) =
                serde_json::from_value::<StreamThrottlePayload>(frame.payload.clone())
            {
                schedule.apply_throttle(payload.interval_ms, Instant::now());
            }
        }
        Some(MessageType::SessionRotate) => {
            rotate_session_credential(deps, &frame).await;
        }
        Some(MessageType::ConfigApply) => {
            apply_configuration(deps, transport, &frame).await;
        }
        Some(MessageType::ConfigPullRequest) => {
            send_config_state(
                deps,
                transport,
                frame.correlation_id.or(Some(frame.message_id)),
            )
            .await;
        }
        Some(MessageType::CommandInvoke) => {
            handle_command_invoke(deps, transport, &frame).await;
        }
        Some(ty) if is_report_ack(ty) => {
            let Ok(ack) = serde_json::from_value::<IngestAckPayload>(frame.payload.clone()) else {
                return;
            };
            let Ok(message_id) = Uuid::parse_str(&ack.message_id) else {
                return;
            };
            let entry = in_flight.remove(&message_id);
            if !ack.accepted
                && let Some(entry) = entry
            {
                deps.queue.lock().await.requeue_front(vec![entry]);
            }
        }
        _ => {}
    }
}

/// Updates the in-memory tracker and persists what an operator needs to see.
///
/// Persisting the last exchange makes "is this host talking to the cloud, and
/// when did it last succeed?" answerable after a restart, without waiting for a
/// reconnection (FR-022, FR-046).
async fn set_state(deps: &SupervisorDeps, state: CloudConnectivityState, error: Option<String>) {
    let last_exchange = {
        let mut tracker = deps.tracker.lock().await;
        tracker.transition(state);
        if let Some(error) = error.clone() {
            tracker.record_error(error);
        }
        tracker.last_exchange_at()
    };

    let mut connection = CloudConnection::connected(CONNECTION_PROTOCOL_LABEL);
    connection.last_exchange_at = last_exchange;
    connection.last_error = error;
    let _ = deps.repository.save_cloud_connection(&connection).await;
}

async fn mark_exchange(deps: &SupervisorDeps) {
    let at = Utc::now();
    {
        let mut tracker = deps.tracker.lock().await;
        tracker.record_exchange(at);
    }

    let mut connection = CloudConnection::connected(CONNECTION_PROTOCOL_LABEL);
    connection.last_exchange_at = Some(at);
    let _ = deps.repository.save_cloud_connection(&connection).await;
}

fn stop_outcome(reason: &StopReason) -> (CloudConnectivityState, &'static str) {
    match reason {
        StopReason::Revoked => (
            CloudConnectivityState::Revoked,
            "this installation was revoked; re-enrollment is required",
        ),
        StopReason::UnsupportedVersion => (
            CloudConnectivityState::Disconnected,
            "no mutually supported protocol version; an upgrade is required",
        ),
        StopReason::Requested => (
            CloudConnectivityState::NotConfigured,
            "cloud supervision was stopped",
        ),
        StopReason::NotEnrolled => (
            CloudConnectivityState::NotConfigured,
            "this installation is not enrolled",
        ),
    }
}

fn describe(outcome: &ConnectionOutcome) -> String {
    match outcome {
        ConnectionOutcome::HandshakeFailed(failure) => failure.remediation(),
        ConnectionOutcome::TransportFailed(reason) => format!("connection failed: {reason}"),
        ConnectionOutcome::SessionEnded => "the cloud closed the session".to_string(),
        ConnectionOutcome::Stopped => "cloud supervision was stopped".to_string(),
    }
}

/// Read from the kernel rather than by shelling out, per the Linux-first rule.
pub(crate) fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|value| value.trim().to_string())
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Waits for `delay`, returning `true` if a stop was requested meanwhile.
async fn wait_or_stop(stop: &mut watch::Receiver<bool>, delay: Duration) -> bool {
    tokio::select! {
        _ = stop.changed() => true,
        _ = tokio::time::sleep(delay) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn store() -> (CloudSecretStore, PathBuf) {
        let dir = std::env::temp_dir().join(format!("argus-secrets-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("temp dir");
        (CloudSecretStore::new(&dir), dir)
    }

    fn mode_of(path: &Path) -> u32 {
        fs::metadata(path).expect("metadata").permissions().mode() & 0o777
    }

    fn credential_path(dir: &Path) -> PathBuf {
        dir.join(SESSION_CREDENTIAL_FILE)
    }

    #[test]
    fn an_absent_credential_reads_as_none() {
        let (store, dir) = store();
        assert_eq!(store.read_session_credential().unwrap(), None);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_credential_round_trips() {
        let (store, dir) = store();
        let credential = SessionCredential::new("session-credential-value");

        store.store_session_credential(&credential).unwrap();
        assert_eq!(store.read_session_credential().unwrap(), Some(credential));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_stored_credential_is_mode_0600() {
        let (store, dir) = store();
        store
            .store_session_credential(&SessionCredential::new("value"))
            .unwrap();

        assert_eq!(
            mode_of(&credential_path(&dir)),
            0o600,
            "a group- or world-readable credential is a defect"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn storing_replaces_rather_than_appends() {
        let (store, dir) = store();
        store
            .store_session_credential(&SessionCredential::new("first-credential"))
            .unwrap();
        store
            .store_session_credential(&SessionCredential::new("second"))
            .unwrap();

        let stored = store.read_session_credential().unwrap().unwrap();
        assert_eq!(stored.expose(), "second", "a rotation must not concatenate");
        assert!(!stored.expose().contains("first"));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn replacing_preserves_the_restrictive_mode() {
        let (store, dir) = store();
        store
            .store_session_credential(&SessionCredential::new("first"))
            .unwrap();
        store
            .store_session_credential(&SessionCredential::new("second"))
            .unwrap();

        assert_eq!(mode_of(&credential_path(&dir)), 0o600);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_credential_created_by_hand_with_a_trailing_newline_is_read_cleanly() {
        let (store, dir) = store();
        fs::write(credential_path(&dir), "hand-written\n").unwrap();

        let stored = store.read_session_credential().unwrap().unwrap();
        assert_eq!(stored.expose(), "hand-written");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_empty_file_reads_as_none_rather_than_an_empty_credential() {
        let (store, dir) = store();
        fs::write(credential_path(&dir), "\n").unwrap();
        assert_eq!(store.read_session_credential().unwrap(), None);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn removing_is_idempotent() {
        let (store, dir) = store();
        store
            .remove_session_credential()
            .expect("absence is success");

        store
            .store_session_credential(&SessionCredential::new("value"))
            .unwrap();
        store.remove_session_credential().unwrap();
        store
            .remove_session_credential()
            .expect("second removal is fine");

        assert_eq!(store.read_session_credential().unwrap(), None);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_empty_credential_is_refused() {
        let (store, dir) = store();
        let err = store
            .store_session_credential(&SessionCredential::new(""))
            .expect_err("an empty credential is a bug, not a value");
        assert!(matches!(err, SecretError::Failed(_)), "{err:?}");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_credential_never_appears_in_debug_output() {
        let credential = SessionCredential::new("super-secret-value");
        let rendered = format!("{credential:?}");
        assert!(!rendered.contains("super-secret-value"), "{rendered}");
        assert!(rendered.contains("<redacted>"), "{rendered}");
    }

    #[test]
    fn a_leftover_temporary_file_does_not_block_a_store() {
        let (store, dir) = store();
        fs::write(dir.join(".cloud-session.incoming"), "stale").unwrap();

        store
            .store_session_credential(&SessionCredential::new("fresh"))
            .expect("a stale temp file must not block the write");
        assert_eq!(
            store.read_session_credential().unwrap().unwrap().expose(),
            "fresh"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_default_location_matches_the_packaged_convention() {
        assert_eq!(
            CloudSecretStore::default_location().root(),
            Path::new(DEFAULT_SECRET_DIR)
        );
        assert!(SESSION_CREDENTIAL_FILE.ends_with("cloud-session"));
    }

    use argus_cloud::protocol::{Envelope, MessageType};
    use argus_cloud::transport::fake::FakeTransport;
    use argus_state::SqliteRepository;

    const CLOUD_ID: &str = "argus-cloud";
    const CODE: &str = "ARGUS-7F3K-9Q2M-4XZ8";
    const SESSION_TOKEN: &str = "granted-session-credential";

    fn now() -> DateTime<Utc> {
        chrono::TimeZone::with_ymd_and_hms(&Utc, 2026, 9, 21, 12, 0, 0).unwrap()
    }

    fn active_config() -> CloudConfig {
        CloudConfig {
            enabled: true,
            endpoint: Some("wss://cloud.example.com/agent".into()),
            ..CloudConfig::default()
        }
    }

    fn hello() -> Envelope {
        Envelope::new(
            MessageType::HandshakeHello,
            serde_json::json!({
                "cloud_id": CLOUD_ID,
                "cloud_instance": "gateway-7",
                "server_time": now().to_rfc3339(),
                "supported_protocol_versions": ["1.0.0"],
                "challenge": "nonce",
                "heartbeat_interval_seconds": 20
            }),
            None,
        )
    }

    fn granted() -> Envelope {
        Envelope::new(
            MessageType::PairingGranted,
            serde_json::json!({
                "instance_id": uuid::Uuid::new_v4().to_string(),
                "tenant_id": uuid::Uuid::new_v4().to_string(),
                "instance_name": "web-01",
                "session_token": SESSION_TOKEN,
                "session_expires_at": (now() + chrono::Duration::hours(1)).to_rfc3339(),
                "negotiated_protocol_version": "1.0.0"
            }),
            None,
        )
    }

    fn ready() -> Envelope {
        Envelope::new(
            MessageType::HandshakeReady,
            serde_json::json!({
                "negotiated_protocol_version": "1.0.0",
                "instance_id": uuid::Uuid::new_v4().to_string(),
                "tenant_id": uuid::Uuid::new_v4().to_string(),
                "config_pull_required": true
            }),
            None,
        )
    }

    fn denied(code: &str) -> Envelope {
        Envelope::new(
            MessageType::PairingDenied,
            serde_json::json!({ "code": code }),
            None,
        )
    }

    async fn enroll_with(
        repository: &dyn DomainRepository,
        secrets: &CloudSecretStore,
        config: &CloudConfig,
        frames: Vec<Envelope>,
    ) -> (Result<EnrollmentReport, EnrollmentError>, FakeTransport) {
        let mut transport = FakeTransport::with_inbound(frames);
        let outcome = enroll_installation(
            config,
            secrets,
            repository,
            &mut transport,
            EnrollmentRequest {
                code: CODE,
                hostname: "web-01",
                agent_version: "0.1.7",
                now: now(),
            },
        )
        .await;
        (outcome, transport)
    }

    #[tokio::test]
    async fn a_granted_enrollment_persists_the_binding_and_stores_the_credential() {
        let repository = SqliteRepository::open_in_memory().unwrap();
        let (secrets, dir) = store();

        let (outcome, transport) = enroll_with(
            &repository,
            &secrets,
            &active_config(),
            vec![hello(), granted()],
        )
        .await;

        let report = outcome.expect("enrolled");
        assert_eq!(report.instance_name, "web-01");
        assert_eq!(report.protocol_version, "1.0.0");

        let enrollment = repository
            .get_cloud_enrollment()
            .await
            .unwrap()
            .expect("persisted");
        assert_eq!(enrollment.installation_id, report.installation_id);
        assert_eq!(enrollment.tenant_id, report.tenant_id);

        let stored = secrets.read_session_credential().unwrap().expect("stored");
        assert_eq!(stored.expose(), SESSION_TOKEN);

        assert_eq!(transport.count_sent(MessageType::PairingRedeem), 1);
        fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn the_enrollment_report_carries_no_secret() {
        let repository = SqliteRepository::open_in_memory().unwrap();
        let (secrets, dir) = store();

        let (outcome, _) = enroll_with(
            &repository,
            &secrets,
            &active_config(),
            vec![hello(), granted()],
        )
        .await;
        let rendered = format!("{:?}", outcome.expect("enrolled"));

        assert!(
            !rendered.contains(SESSION_TOKEN),
            "IPC result leaked a credential: {rendered}"
        );
        assert!(
            !rendered.contains(CODE),
            "IPC result leaked the pairing code"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn enrolling_when_already_enrolled_is_refused_and_sends_nothing() {
        let repository = SqliteRepository::open_in_memory().unwrap();
        let (secrets, dir) = store();

        repository
            .save_cloud_enrollment(&argus_domain::CloudEnrollment::new(
                uuid::Uuid::new_v4(),
                uuid::Uuid::new_v4(),
                "existing",
                now(),
            ))
            .await
            .unwrap();

        let (outcome, transport) = enroll_with(
            &repository,
            &secrets,
            &active_config(),
            vec![hello(), granted()],
        )
        .await;

        assert_eq!(outcome, Err(EnrollmentError::AlreadyEnrolled));
        assert!(
            transport.sent().is_empty(),
            "an already-enrolled installation must not contact the cloud"
        );
        assert_eq!(secrets.read_session_credential().unwrap(), None);
        fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_denied_enrollment_persists_nothing() {
        let repository = SqliteRepository::open_in_memory().unwrap();
        let (secrets, dir) = store();

        let (outcome, _) = enroll_with(
            &repository,
            &secrets,
            &active_config(),
            vec![hello(), denied("USED")],
        )
        .await;

        match outcome {
            Err(EnrollmentError::Denied { code, remediation }) => {
                assert_eq!(code, PairingDenialCode::Used);
                assert!(!remediation.is_empty());
            }
            other => panic!("expected Denied, got {other:?}"),
        }
        assert_eq!(repository.get_cloud_enrollment().await.unwrap(), None);
        assert_eq!(secrets.read_session_credential().unwrap(), None);
        fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_cloud_client_failure_persists_nothing() {
        let repository = SqliteRepository::open_in_memory().unwrap();
        let (secrets, dir) = store();

        let (outcome, _) = enroll_with(&repository, &secrets, &active_config(), vec![]).await;
        assert!(
            matches!(outcome, Err(EnrollmentError::Cloud(_))),
            "{outcome:?}"
        );
        assert_eq!(repository.get_cloud_enrollment().await.unwrap(), None);
        assert_eq!(secrets.read_session_credential().unwrap(), None);
        fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn an_unconfigured_installation_refuses_enrollment_without_touching_anything() {
        let repository = SqliteRepository::open_in_memory().unwrap();
        let (secrets, dir) = store();
        let disabled = CloudConfig::default();

        let (outcome, transport) =
            enroll_with(&repository, &secrets, &disabled, vec![hello(), granted()]).await;

        assert_eq!(outcome, Err(EnrollmentError::NotConfigured));
        assert!(transport.sent().is_empty());
        assert_eq!(repository.get_cloud_enrollment().await.unwrap(), None);
        assert_eq!(secrets.read_session_credential().unwrap(), None);
        fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn an_unenrolled_installation_still_has_working_local_state() {
        // FR-007 / SC-008: cloud absence must not degrade local operation.
        let repository = SqliteRepository::open_in_memory().unwrap();
        let environment = argus_domain::EnvironmentId::new();
        let health = argus_domain::HealthStatus::ready(now());
        repository.save_environment(&environment).await.unwrap();
        repository.save_health(&health).await.unwrap();

        assert_eq!(repository.get_cloud_enrollment().await.unwrap(), None);
        assert_eq!(
            repository.get_environment().await.unwrap(),
            Some(environment)
        );
        assert_eq!(repository.get_health().await.unwrap(), Some(health));
    }

    use argus_cloud::state::ConnectivityTracker;

    async fn wait_for_state(
        tracker: &Arc<Mutex<ConnectivityTracker>>,
        expected: CloudConnectivityState,
        within: Duration,
    ) -> bool {
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            if tracker.lock().await.state() == expected {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        false
    }

    async fn wait_for_exchange(
        tracker: &Arc<Mutex<ConnectivityTracker>>,
        within: Duration,
    ) -> bool {
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            if tracker.lock().await.last_exchange_at().is_some() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        false
    }

    fn supervisor_deps(
        config: CloudConfig,
        secrets: CloudSecretStore,
        repository: Arc<SqliteRepository>,
        tracker: Arc<Mutex<ConnectivityTracker>>,
        factory: Arc<argus_cloud::transport::fake::FakeFactory>,
    ) -> SupervisorDeps {
        SupervisorDeps {
            config,
            secrets,
            repository,
            tracker,
            factory,
            events: Arc::new(LocalEventBus::new(16)),
            queue: Arc::new(Mutex::new(ReportQueue::new(256))),
            capabilities: Arc::new(Vec::new()),
            managed_settings: Arc::new(ManagedSettingsStore::new(std::env::temp_dir().join(
                format!("argus-cloud-settings-{}.toml", uuid::Uuid::new_v4()),
            ))),
            environment_id: EnvironmentId::new(),
            policy: Arc::new(argus_policy::BootstrapPolicyEvaluator::new()),
            executor: Arc::new(argus_executor::PrivilegedExecutor::new(Arc::new(
                argus_executor::MockServiceController::new(),
            ))),
            approvals: Arc::new(ApprovalStore::new()),
            limiter: Arc::new(PrivilegedLimiter::new(2)),
        }
    }

    #[tokio::test]
    async fn a_disabled_installation_never_dials() {
        let (secrets, dir) = store();
        let repository = Arc::new(SqliteRepository::open_in_memory().unwrap());
        let tracker = Arc::new(Mutex::new(ConnectivityTracker::new()));
        let factory = Arc::new(argus_cloud::transport::fake::FakeFactory::new());

        let deps = supervisor_deps(
            CloudConfig::default(),
            secrets,
            repository,
            Arc::clone(&tracker),
            Arc::clone(&factory),
        );
        let (stop, stop_rx) = watch::channel(false);
        let handle = tokio::spawn(supervise(deps, stop_rx));

        assert!(
            wait_for_state(
                &tracker,
                CloudConnectivityState::NotConfigured,
                Duration::from_secs(2)
            )
            .await
        );
        let _ = stop.send(true);
        let _ = handle.await;

        assert_eq!(
            factory.connect_attempts(),
            0,
            "a disabled cloud must not produce a single connection attempt"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn an_enrolled_installation_connects_and_records_the_exchange() {
        let (secrets, dir) = store();
        secrets
            .store_session_credential(&SessionCredential::new(SESSION_TOKEN))
            .unwrap();
        assert_eq!(
            secrets.read_session_credential().unwrap().unwrap().expose(),
            SESSION_TOKEN
        );

        let repository = Arc::new(SqliteRepository::open_in_memory().unwrap());
        repository
            .save_cloud_enrollment(&argus_domain::CloudEnrollment::new(
                uuid::Uuid::new_v4(),
                uuid::Uuid::new_v4(),
                "web-01",
                now(),
            ))
            .await
            .unwrap();

        let tracker = Arc::new(Mutex::new(ConnectivityTracker::new()));
        let factory = Arc::new(argus_cloud::transport::fake::FakeFactory::new());
        factory.script_connection(vec![hello(), ready()]);

        let deps = supervisor_deps(
            active_config(),
            secrets,
            Arc::clone(&repository),
            Arc::clone(&tracker),
            Arc::clone(&factory),
        );
        let (stop, stop_rx) = watch::channel(false);
        let handle = tokio::spawn(supervise(deps, stop_rx));

        assert!(
            wait_for_exchange(&tracker, Duration::from_secs(2)).await,
            "no exchange was recorded. The scripted connection ends as soon as its \
             frames run out, so the state legitimately returns to disconnected; the \
             recorded exchange is what proves the session was established. State: {:?}",
            tracker.lock().await.state()
        );

        let _ = stop.send(true);
        let _ = handle.await;

        assert_eq!(factory.connect_attempts(), 1);
        let connection = repository.get_cloud_connection().await.unwrap().unwrap();
        assert!(
            connection.last_exchange_at.is_some(),
            "the last exchange must survive a restart (FR-022)"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_revocation_ends_supervision_without_a_stop_signal() {
        let (secrets, dir) = store();
        secrets
            .store_session_credential(&SessionCredential::new(SESSION_TOKEN))
            .unwrap();

        let repository = Arc::new(SqliteRepository::open_in_memory().unwrap());
        repository
            .save_cloud_enrollment(&argus_domain::CloudEnrollment::new(
                uuid::Uuid::new_v4(),
                uuid::Uuid::new_v4(),
                "web-01",
                now(),
            ))
            .await
            .unwrap();

        let tracker = Arc::new(Mutex::new(ConnectivityTracker::new()));
        let factory = Arc::new(argus_cloud::transport::fake::FakeFactory::new());
        factory.script_connection(vec![
            hello(),
            Envelope::new(
                MessageType::HandshakeReject,
                serde_json::json!({ "code": "REVOKED", "message": "instance revoked" }),
                None,
            ),
        ]);

        let deps = supervisor_deps(
            active_config(),
            secrets,
            Arc::clone(&repository),
            Arc::clone(&tracker),
            factory,
        );
        let (_stop, stop_rx) = watch::channel(false);
        let handle = tokio::spawn(supervise(deps, stop_rx));

        let finished = tokio::time::timeout(Duration::from_secs(5), handle).await;
        assert!(finished.is_ok(), "supervision must return once revoked");

        assert_eq!(
            tracker.lock().await.state(),
            CloudConnectivityState::Revoked
        );
        assert!(
            !tracker.lock().await.allows_reconnect(),
            "a revoked installation must not keep trying"
        );
        fs::remove_dir_all(&dir).ok();
    }

    mod config_apply {
        use super::*;

        fn frame(configuration_id: Uuid, version: i64, content: serde_json::Value) -> Envelope {
            Envelope::new(
                MessageType::ConfigApply,
                serde_json::json!({
                    "configuration_id": configuration_id.to_string(),
                    "version_id": Uuid::new_v4().to_string(),
                    "version_number": version,
                    "content_hash": format!("hash-{version}"),
                    "content": content,
                    "mode": "apply"
                }),
                None,
            )
        }

        fn provider_content(extra: serde_json::Value) -> serde_json::Value {
            let mut content = serde_json::json!({
                "kind": "provider",
                "provider": "openai",
                "model": "gpt-4.1"
            });
            if let (Some(base), Some(extra)) = (content.as_object_mut(), extra.as_object()) {
                for (key, value) in extra {
                    base.insert(key.clone(), value.clone());
                }
            }
            content
        }

        async fn deps_and_transport() -> (SupervisorDeps, PathBuf) {
            let (secrets, dir) = store();
            let repository = Arc::new(SqliteRepository::open_in_memory().unwrap());
            let deps = supervisor_deps(
                active_config(),
                secrets,
                repository,
                Arc::new(Mutex::new(ConnectivityTracker::new())),
                Arc::new(argus_cloud::transport::fake::FakeFactory::new()),
            );
            (deps, dir)
        }

        /// An executor for capabilities that do not change the host.
        struct StubReadOnly;

        impl Executor for StubReadOnly {
            fn execute(
                &self,
                action: &AuthorizedAction,
            ) -> Result<argus_executor::ExecutionResult, argus_executor::ExecutionError>
            {
                Ok(argus_executor::ExecutionResult {
                    capability: action.capability().clone(),
                    evidence: serde_json::json!({ "stub": true }),
                    started_at: Utc::now(),
                    finished_at: Utc::now(),
                })
            }
        }

        /// A command-capable supervisor whose service controller is observable.
        async fn command_deps(
            config: CloudConfig,
            capabilities: Vec<CapabilityDescriptor>,
        ) -> (
            SupervisorDeps,
            Arc<argus_executor::MockServiceController>,
            PathBuf,
        ) {
            let (secrets, dir) = store();
            let repository = Arc::new(SqliteRepository::open_in_memory().unwrap());
            let deps = supervisor_deps(
                config,
                secrets,
                repository,
                Arc::new(Mutex::new(ConnectivityTracker::new())),
                Arc::new(argus_cloud::transport::fake::FakeFactory::new()),
            );

            let services = Arc::new(argus_executor::MockServiceController::new());
            let service_controller: Arc<dyn argus_executor::ServiceController> =
                Arc::clone(&services) as Arc<dyn argus_executor::ServiceController>;
            let executor = Arc::new(argus_executor::CompositeExecutor::new(
                Arc::new(StubReadOnly),
                Arc::new(argus_executor::PrivilegedExecutor::new(service_controller)),
            ));

            (
                SupervisorDeps {
                    capabilities: Arc::new(capabilities),
                    executor,
                    ..deps
                },
                services,
                dir,
            )
        }

        /// The published surface a command test invokes against.
        fn published_capabilities() -> Vec<CapabilityDescriptor> {
            vec![
                CapabilityDescriptor::new(
                    CapabilityId::new(CapabilityId::HOST_SERVICE_RESTART).unwrap(),
                    "argusd",
                    CapabilityId::HOST_SERVICE_RESTART,
                    argus_domain::RiskClass::LowRisk,
                    Version::new(0, 1, 0),
                    serde_json::json!({}),
                    serde_json::json!({}),
                    argus_domain::Reversibility::Reversible,
                )
                .with_blast_radius(argus_domain::BlastRadius::Host)
                .requiring_approval(),
                CapabilityDescriptor::new(
                    CapabilityId::new(CapabilityId::ARGUS_HEALTH_READ).unwrap(),
                    "argusd",
                    CapabilityId::ARGUS_HEALTH_READ,
                    argus_domain::RiskClass::Read,
                    Version::new(0, 1, 0),
                    serde_json::json!({ "type": "object", "additionalProperties": false }),
                    serde_json::json!({}),
                    argus_domain::Reversibility::None,
                )
                .with_blast_radius(argus_domain::BlastRadius::None),
            ]
        }

        fn command_frame(command_id: Uuid, capability: &str, input: serde_json::Value) -> Envelope {
            Envelope::new(
                MessageType::CommandInvoke,
                serde_json::json!({
                    "command_id": command_id.to_string(),
                    "capability_id": capability,
                    "input": input,
                }),
                None,
            )
        }

        async fn invoke(deps: &SupervisorDeps, frame: &Envelope) -> FakeTransport {
            let transport = FakeTransport::new();
            handle_command_invoke(deps, &transport, frame).await;
            transport
        }

        fn result_for(transport: &FakeTransport, command_id: Uuid) -> serde_json::Value {
            let sent = transport.sent_of_type(MessageType::CommandResult);
            assert_eq!(sent.len(), 1, "exactly one command.result per delivery");
            assert_eq!(sent[0].payload["command_id"], command_id.to_string());
            sent[0].payload.clone()
        }

        #[tokio::test]
        async fn a_command_for_an_unpublished_capability_is_refused() {
            let (deps, _services, dir) =
                command_deps(active_config(), published_capabilities()).await;
            let id = Uuid::new_v4();

            let transport = invoke(
                &deps,
                &command_frame(id, "container.restart", serde_json::json!({})),
            )
            .await;

            let result = result_for(&transport, id);
            assert_eq!(result["status"], "refused");
            let stored = deps
                .repository
                .get_cloud_command(id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                stored.refusal_reason,
                Some(RefusalReason::UnknownCapability)
            );

            fs::remove_dir_all(&dir).ok();
        }

        #[tokio::test]
        async fn a_command_whose_input_mismatches_the_schema_is_refused() {
            let (deps, _services, dir) =
                command_deps(active_config(), published_capabilities()).await;
            let id = Uuid::new_v4();

            let transport = invoke(
                &deps,
                &command_frame(
                    id,
                    CapabilityId::ARGUS_HEALTH_READ,
                    serde_json::json!({ "unexpected": true }),
                ),
            )
            .await;

            result_for(&transport, id);
            let stored = deps
                .repository
                .get_cloud_command(id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(stored.refusal_reason, Some(RefusalReason::InvalidInput));

            fs::remove_dir_all(&dir).ok();
        }

        #[tokio::test]
        async fn a_host_changing_command_is_refused_when_the_kill_switch_is_off() {
            let mut config = active_config();
            config.allow_privileged_execution = false;
            let (deps, services, dir) = command_deps(config, published_capabilities()).await;
            let id = Uuid::new_v4();

            let transport = invoke(
                &deps,
                &command_frame(
                    id,
                    CapabilityId::HOST_SERVICE_RESTART,
                    serde_json::json!({ "unit": "nginx.service" }),
                ),
            )
            .await;

            let result = result_for(&transport, id);
            assert_eq!(result["status"], "refused");
            let stored = deps
                .repository
                .get_cloud_command(id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                stored.refusal_reason,
                Some(RefusalReason::ExecutionDisabled)
            );
            assert!(
                services.recorded_calls().is_empty(),
                "a disabled installation must not touch the host"
            );

            fs::remove_dir_all(&dir).ok();
        }

        #[tokio::test]
        async fn a_read_only_command_still_works_with_privileged_execution_disabled() {
            let mut config = active_config();
            config.allow_privileged_execution = false;
            let (deps, _services, dir) = command_deps(config, published_capabilities()).await;
            let id = Uuid::new_v4();

            let transport = invoke(
                &deps,
                &command_frame(id, CapabilityId::ARGUS_HEALTH_READ, serde_json::json!({})),
            )
            .await;

            let result = result_for(&transport, id);
            assert_eq!(
                result["status"], "acknowledged",
                "the kill switch must not disable read-only invocations"
            );

            fs::remove_dir_all(&dir).ok();
        }

        #[tokio::test]
        async fn an_approval_requiring_command_is_refused_without_a_grant() {
            let (deps, services, dir) =
                command_deps(active_config(), published_capabilities()).await;
            let id = Uuid::new_v4();

            let transport = invoke(
                &deps,
                &command_frame(
                    id,
                    CapabilityId::HOST_SERVICE_RESTART,
                    serde_json::json!({ "unit": "nginx.service" }),
                ),
            )
            .await;

            result_for(&transport, id);
            let stored = deps
                .repository
                .get_cloud_command(id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(stored.refusal_reason, Some(RefusalReason::ApprovalRequired));
            assert!(services.recorded_calls().is_empty());

            fs::remove_dir_all(&dir).ok();
        }

        #[tokio::test]
        async fn a_granted_approval_lets_the_command_execute() {
            let (deps, services, dir) =
                command_deps(active_config(), published_capabilities()).await;
            let id = Uuid::new_v4();
            deps.approvals
                .grant_for_a_while(id, "root", Utc::now(), chrono::Duration::minutes(5));

            let transport = invoke(
                &deps,
                &command_frame(
                    id,
                    CapabilityId::HOST_SERVICE_RESTART,
                    serde_json::json!({ "unit": "nginx.service" }),
                ),
            )
            .await;

            let result = result_for(&transport, id);
            assert_eq!(result["status"], "acknowledged");
            assert_eq!(
                services.recorded_calls(),
                vec![("restart".to_string(), "nginx.service".to_string())]
            );

            fs::remove_dir_all(&dir).ok();
        }

        #[tokio::test]
        async fn every_refusal_reason_is_distinct_and_reports_a_reason() {
            let published = published_capabilities();

            let (deps, _s, d1) = command_deps(active_config(), published.clone()).await;
            let unknown = invoke(
                &deps,
                &command_frame(Uuid::new_v4(), "container.restart", serde_json::json!({})),
            )
            .await;

            let (deps, _s, d2) = command_deps(active_config(), published.clone()).await;
            let invalid = invoke(
                &deps,
                &command_frame(
                    Uuid::new_v4(),
                    CapabilityId::ARGUS_HEALTH_READ,
                    serde_json::json!({ "bad": 1 }),
                ),
            )
            .await;

            let mut config = active_config();
            config.allow_privileged_execution = false;
            let (deps, _s, d3) = command_deps(config, published.clone()).await;
            let disabled = invoke(
                &deps,
                &command_frame(
                    Uuid::new_v4(),
                    CapabilityId::HOST_SERVICE_RESTART,
                    serde_json::json!({ "unit": "x" }),
                ),
            )
            .await;

            let (deps, _s, d4) = command_deps(active_config(), published).await;
            let pending = invoke(
                &deps,
                &command_frame(
                    Uuid::new_v4(),
                    CapabilityId::HOST_SERVICE_RESTART,
                    serde_json::json!({ "unit": "x" }),
                ),
            )
            .await;

            let reasons: Vec<String> = [&unknown, &invalid, &disabled, &pending]
                .iter()
                .filter_map(|transport| {
                    transport
                        .sent_of_type(MessageType::CommandResult)
                        .first()
                        .and_then(|frame| frame.payload["reason"].as_str().map(str::to_string))
                })
                .collect();

            assert_eq!(reasons.len(), 4, "every refusal must carry a reason");
            let unique: std::collections::HashSet<&String> = reasons.iter().collect();
            assert_eq!(
                unique.len(),
                4,
                "each refusal reason must be distinct: {reasons:?}"
            );

            for dir in [d1, d2, d3, d4] {
                fs::remove_dir_all(&dir).ok();
            }
        }

        #[tokio::test]
        async fn a_redelivered_terminal_command_re_reports_without_re_executing() {
            let (deps, services, dir) =
                command_deps(active_config(), published_capabilities()).await;
            let id = Uuid::new_v4();
            deps.approvals
                .grant_for_a_while(id, "root", Utc::now(), chrono::Duration::minutes(5));

            let frame = command_frame(
                id,
                CapabilityId::HOST_SERVICE_RESTART,
                serde_json::json!({ "unit": "nginx.service" }),
            );

            let first = invoke(&deps, &frame).await;
            assert_eq!(result_for(&first, id)["status"], "acknowledged");
            assert_eq!(
                services.recorded_calls().len(),
                1,
                "the first delivery executes"
            );

            let second = invoke(&deps, &frame).await;
            assert_eq!(
                result_for(&second, id)["status"],
                "acknowledged",
                "a re-delivery re-reports the stored result"
            );
            assert_eq!(
                services.recorded_calls().len(),
                1,
                "a re-delivered command must never execute twice"
            );

            fs::remove_dir_all(&dir).ok();
        }

        #[tokio::test]
        async fn a_duplicate_while_executing_is_suppressed_without_a_fabricated_result() {
            let (deps, services, dir) =
                command_deps(active_config(), published_capabilities()).await;
            let id = Uuid::new_v4();

            let mut in_flight = CloudCommand::received(
                id,
                deps.environment_id,
                CapabilityId::new(CapabilityId::HOST_SERVICE_RESTART).unwrap(),
                serde_json::json!({ "unit": "nginx.service" }),
                None,
                Utc::now(),
            );
            in_flight.status = CloudCommandStatus::Executing;
            deps.repository.put_cloud_command(&in_flight).await.unwrap();

            let transport = invoke(
                &deps,
                &command_frame(
                    id,
                    CapabilityId::HOST_SERVICE_RESTART,
                    serde_json::json!({ "unit": "nginx.service" }),
                ),
            )
            .await;

            assert!(
                transport
                    .sent_of_type(MessageType::CommandResult)
                    .is_empty(),
                "no result exists yet, so none may be reported"
            );
            assert!(
                services.recorded_calls().is_empty(),
                "and nothing may re-execute"
            );

            fs::remove_dir_all(&dir).ok();
        }

        #[tokio::test]
        async fn a_command_interrupted_by_a_restart_is_reported_failed_with_an_unknown_state() {
            let (deps, _services, dir) =
                command_deps(active_config(), published_capabilities()).await;
            let id = Uuid::new_v4();

            let mut interrupted = CloudCommand::received(
                id,
                deps.environment_id,
                CapabilityId::new(CapabilityId::HOST_SERVICE_RESTART).unwrap(),
                serde_json::json!({ "unit": "nginx.service" }),
                None,
                Utc::now(),
            );
            interrupted.status = CloudCommandStatus::Executing;
            deps.repository
                .put_cloud_command(&interrupted)
                .await
                .unwrap();

            let transport = FakeTransport::new();
            resolve_interrupted_commands(&deps, &transport).await;

            let sent = transport.sent_of_type(MessageType::CommandResult);
            assert_eq!(sent.len(), 1, "the interrupted command must be settled");
            assert_eq!(sent[0].payload["status"], "failed");
            assert!(
                sent[0].payload["reason"]
                    .as_str()
                    .unwrap()
                    .contains("unknown"),
                "the operator must be told the outcome is unknown: {}",
                sent[0].payload["reason"]
            );

            let settled = deps
                .repository
                .get_cloud_command(id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(settled.status, CloudCommandStatus::Failed);

            fs::remove_dir_all(&dir).ok();
        }

        async fn apply_and_read_result(
            deps: &SupervisorDeps,
            frame: Envelope,
        ) -> serde_json::Value {
            let transport = FakeTransport::new();
            apply_configuration(deps, &transport, &frame).await;
            let sent = transport.sent_of_type(MessageType::ConfigResult);
            assert_eq!(sent.len(), 1, "exactly one config.result per delivery");
            sent[0].payload.clone()
        }

        #[tokio::test]
        async fn a_provider_configuration_is_applied_and_reported() {
            let (deps, dir) = deps_and_transport().await;
            let id = Uuid::new_v4();

            let result =
                apply_and_read_result(&deps, frame(id, 1, provider_content(serde_json::json!({}))))
                    .await;

            assert_eq!(result["status"], "applied");
            assert!(result.get("reason").is_none(), "a success needs no reason");

            let settings = deps.managed_settings.read().unwrap().expect("written");
            assert_eq!(settings["provider"].as_str(), Some("openai"));
            assert_eq!(settings["model"].as_str(), Some("gpt-4.1"));

            fs::remove_dir_all(&dir).ok();
        }

        #[tokio::test]
        async fn the_applied_version_is_recorded_for_reconciliation() {
            let (deps, dir) = deps_and_transport().await;
            let id = Uuid::new_v4();

            apply_and_read_result(&deps, frame(id, 3, provider_content(serde_json::json!({}))))
                .await;

            let held = deps
                .repository
                .get_managed_configuration(id)
                .await
                .unwrap()
                .expect("recorded");
            assert_eq!(held.current_version_number, 3);
            assert_eq!(held.apply_status, ApplyStatus::Applied);

            let applied = deps.repository.list_applied_configurations().await.unwrap();
            assert_eq!(applied.len(), 1);
            assert_eq!(applied[0].applied_version_number, 3);

            fs::remove_dir_all(&dir).ok();
        }

        #[tokio::test]
        async fn an_unrecognised_field_is_rejected_and_nothing_is_written() {
            let (deps, dir) = deps_and_transport().await;
            let content = provider_content(serde_json::json!({ "something_else": true }));

            let result = apply_and_read_result(&deps, frame(Uuid::new_v4(), 1, content)).await;

            assert_eq!(result["status"], "rejected");
            assert!(
                result["reason"]
                    .as_str()
                    .unwrap()
                    .contains("something_else"),
                "the rejection must name the offending field: {result}"
            );
            assert!(
                deps.managed_settings.read().unwrap().is_none(),
                "nothing may be written when any field is rejected"
            );

            fs::remove_dir_all(&dir).ok();
        }

        #[tokio::test]
        async fn the_agent_team_kind_is_rejected_with_an_explicit_reason() {
            let (deps, dir) = deps_and_transport().await;
            let content = serde_json::json!({ "kind": "agent_team", "agents": ["sentinel"] });

            let result = apply_and_read_result(&deps, frame(Uuid::new_v4(), 1, content)).await;

            assert_eq!(result["status"], "rejected");
            let reason = result["reason"].as_str().unwrap();
            assert!(
                reason.contains("no local runtime"),
                "the operator must be told why, not merely that it failed: {reason}"
            );
            assert!(deps.managed_settings.read().unwrap().is_none());

            fs::remove_dir_all(&dir).ok();
        }

        #[tokio::test]
        async fn a_configuration_without_a_kind_is_rejected() {
            let (deps, dir) = deps_and_transport().await;
            let content = serde_json::json!({ "provider": "openai" });

            let result = apply_and_read_result(&deps, frame(Uuid::new_v4(), 1, content)).await;

            assert_eq!(result["status"], "rejected");
            assert!(result["reason"].as_str().unwrap().contains("kind"));

            fs::remove_dir_all(&dir).ok();
        }

        #[tokio::test]
        async fn a_stale_version_is_rejected_rather_than_reapplied() {
            let (deps, dir) = deps_and_transport().await;
            let id = Uuid::new_v4();

            apply_and_read_result(&deps, frame(id, 5, provider_content(serde_json::json!({}))))
                .await;
            let result =
                apply_and_read_result(&deps, frame(id, 4, provider_content(serde_json::json!({}))))
                    .await;

            assert_eq!(result["status"], "rejected");
            assert!(
                result["reason"].as_str().unwrap().contains("not newer"),
                "an out-of-order delivery must say why it was ignored: {result}"
            );

            fs::remove_dir_all(&dir).ok();
        }

        #[tokio::test]
        async fn a_delivered_credential_never_reaches_the_non_secret_settings() {
            // FR-035/FR-051: the credential goes to the secret store; the settings
            // file and the report must not contain it.
            let (deps, dir) = deps_and_transport().await;
            let content =
                provider_content(serde_json::json!({ "api_token": "sk-super-secret-value" }));

            let result = apply_and_read_result(&deps, frame(Uuid::new_v4(), 1, content)).await;

            assert_eq!(result["status"], "applied");
            assert!(
                !serde_json::to_string(&result)
                    .unwrap()
                    .contains("sk-super-secret-value"),
                "the report must never echo a credential"
            );

            let settings_text =
                std::fs::read_to_string(deps.managed_settings.path()).expect("written");
            assert!(
                !settings_text.contains("sk-super-secret-value"),
                "the credential must not be in the settings file: {settings_text}"
            );

            let stored = deps
                .secrets
                .read_provider_credential()
                .unwrap()
                .expect("stored");
            assert_eq!(stored.expose(), "sk-super-secret-value");

            fs::remove_dir_all(&dir).ok();
        }

        #[tokio::test]
        async fn a_malformed_configuration_is_reported_as_failed() {
            let (deps, dir) = deps_and_transport().await;
            let malformed = Envelope::new(
                MessageType::ConfigApply,
                serde_json::json!({ "not": "a configuration" }),
                None,
            );

            let result = apply_and_read_result(&deps, malformed).await;

            assert_eq!(result["status"], "failed");

            fs::remove_dir_all(&dir).ok();
        }
    }

    mod trust_lifecycle {
        use super::*;

        fn rotate_frame(token: &str, expires_in: chrono::Duration) -> Envelope {
            Envelope::new(
                MessageType::SessionRotate,
                serde_json::json!({
                    "rotation_token": token,
                    "expires_at": (Utc::now() + expires_in).to_rfc3339()
                }),
                None,
            )
        }

        fn deps_with_credential(value: &str) -> (SupervisorDeps, PathBuf) {
            let (secrets, dir) = store();
            secrets
                .store_session_credential(&SessionCredential::new(value))
                .unwrap();
            let repository = Arc::new(SqliteRepository::open_in_memory().unwrap());
            let deps = supervisor_deps(
                active_config(),
                secrets,
                repository,
                Arc::new(Mutex::new(ConnectivityTracker::new())),
                Arc::new(argus_cloud::transport::fake::FakeFactory::new()),
            );
            (deps, dir)
        }

        #[tokio::test]
        async fn a_valid_rotation_replaces_the_credential() {
            let (deps, dir) = deps_with_credential("original-credential");

            rotate_session_credential(
                &deps,
                &rotate_frame("rotated-credential", chrono::Duration::hours(1)),
            )
            .await;

            let stored = deps.secrets.read_session_credential().unwrap().unwrap();
            assert_eq!(stored.expose(), "rotated-credential");

            fs::remove_dir_all(&dir).ok();
        }

        #[tokio::test]
        async fn the_previous_credential_is_retired_rather_than_kept_alongside() {
            // Two usable credentials at once would widen the window an attacker
            // could exploit after a rotation.
            let (deps, dir) = deps_with_credential("original-credential");

            rotate_session_credential(
                &deps,
                &rotate_frame("rotated-credential", chrono::Duration::hours(1)),
            )
            .await;

            let raw = std::fs::read_to_string(deps.secrets.root().join(SESSION_CREDENTIAL_FILE))
                .expect("credential file");
            assert!(
                !raw.contains("original-credential"),
                "the retired credential must be gone: {raw}"
            );

            fs::remove_dir_all(&dir).ok();
        }

        #[tokio::test]
        async fn an_expired_rotation_is_ignored() {
            let (deps, dir) = deps_with_credential("original-credential");

            rotate_session_credential(
                &deps,
                &rotate_frame("stale-token", chrono::Duration::hours(-1)),
            )
            .await;

            let stored = deps.secrets.read_session_credential().unwrap().unwrap();
            assert_eq!(
                stored.expose(),
                "original-credential",
                "a credential the cloud no longer honours must not be adopted"
            );

            fs::remove_dir_all(&dir).ok();
        }

        #[tokio::test]
        async fn a_malformed_rotation_changes_nothing() {
            let (deps, dir) = deps_with_credential("original-credential");
            let malformed = Envelope::new(
                MessageType::SessionRotate,
                serde_json::json!({ "not": "a rotation" }),
                None,
            );

            rotate_session_credential(&deps, &malformed).await;

            let stored = deps.secrets.read_session_credential().unwrap().unwrap();
            assert_eq!(stored.expose(), "original-credential");

            fs::remove_dir_all(&dir).ok();
        }

        #[tokio::test]
        async fn work_in_flight_is_returned_when_a_session_ends() {
            // T095: a lifecycle change arriving mid-session must not lose the
            // outcome of work already underway.
            let (deps, dir) = deps_with_credential("credential");
            let mut in_flight: HashMap<Uuid, BufferedReport> = HashMap::new();

            let mid_session = BufferedReport {
                entry_id: Uuid::new_v4(),
                kind: ReportKind::Telemetry,
                payload: serde_json::json!({ "tag": "mid-session" }),
                enqueued_at: Utc::now(),
                attempts: 1,
            };
            in_flight.insert(Uuid::new_v4(), mid_session);

            requeue_in_flight(&deps, &mut in_flight).await;

            assert!(in_flight.is_empty(), "nothing may stay in flight");
            let recovered = deps.queue.lock().await.drain(8, Utc::now());
            assert_eq!(recovered.len(), 1, "the report was returned, not dropped");
            assert_eq!(
                recovered[0].payload,
                serde_json::json!({ "tag": "mid-session" })
            );

            fs::remove_dir_all(&dir).ok();
        }
    }

    mod local_inspection {
        use super::*;

        #[tokio::test]
        async fn a_never_enrolled_installation_reports_not_configured() {
            // T102: the operator is asking a question, not seeking an error.
            let (_secrets, dir) = store();
            let repository = SqliteRepository::open_in_memory().unwrap();

            let status = cloud_status(
                &CloudConfig::default(),
                &repository,
                &ConnectivityTracker::new(),
                &ReportQueue::new(256),
            )
            .await;

            assert_eq!(status.state, CloudConnectivityState::NotConfigured);
            assert!(!status.enabled);
            assert!(status.tenant_id.is_none());
            assert!(status.installation_id.is_none());
            assert!(
                status.remediation.is_some(),
                "an operator with no enrollment needs a next step"
            );

            fs::remove_dir_all(&dir).ok();
        }

        #[tokio::test]
        async fn status_reports_the_binding_and_the_last_exchange() {
            let (_secrets, dir) = store();
            let repository = SqliteRepository::open_in_memory().unwrap();
            let enrollment =
                argus_domain::CloudEnrollment::new(Uuid::new_v4(), Uuid::new_v4(), "web-01", now());
            repository.save_cloud_enrollment(&enrollment).await.unwrap();

            let mut tracker = ConnectivityTracker::new();
            tracker.transition(CloudConnectivityState::Connected);
            tracker.record_exchange(now());

            let mut queue = ReportQueue::new(256);
            queue.enqueue(ReportKind::Telemetry, serde_json::json!({}), now());

            let status = cloud_status(&active_config(), &repository, &tracker, &queue).await;

            assert_eq!(status.state, CloudConnectivityState::Connected);
            assert_eq!(status.tenant_id, Some(enrollment.tenant_id));
            assert_eq!(status.instance_name.as_deref(), Some("web-01"));
            assert_eq!(status.last_exchange_at, Some(now()));
            assert_eq!(status.buffered_reports, 1);
            assert!(status.privileged_execution_enabled);
            assert!(
                status.remediation.is_none(),
                "a healthy installation needs no guidance"
            );

            fs::remove_dir_all(&dir).ok();
        }

        #[tokio::test]
        async fn status_never_carries_a_credential() {
            // A diagnostic an operator pastes into a ticket must be safe to paste.
            let (secrets, dir) = store();
            secrets
                .store_session_credential(&SessionCredential::new("secret-session-value"))
                .unwrap();
            secrets
                .store_provider_credential(&ProviderCredential::new("secret-provider-value"))
                .unwrap();
            let repository = SqliteRepository::open_in_memory().unwrap();

            let status = cloud_status(
                &active_config(),
                &repository,
                &ConnectivityTracker::new(),
                &ReportQueue::new(8),
            )
            .await;
            let rendered = serde_json::to_string(&status).unwrap();

            assert!(
                !rendered.contains("secret-session-value")
                    && !rendered.contains("secret-provider-value"),
                "status must not leak a credential: {rendered}"
            );

            fs::remove_dir_all(&dir).ok();
        }

        #[tokio::test]
        async fn every_connectivity_state_is_reported_with_guidance_where_needed() {
            // T097: whatever the status says, the operator must know whether to act.
            let (_secrets, dir) = store();
            let repository = SqliteRepository::open_in_memory().unwrap();

            let states = [
                CloudConnectivityState::NotConfigured,
                CloudConnectivityState::Connecting,
                CloudConnectivityState::Connected,
                CloudConnectivityState::Degraded,
                CloudConnectivityState::Disconnected,
                CloudConnectivityState::Suspended,
                CloudConnectivityState::Revoked,
            ];

            for state in states {
                let mut tracker = ConnectivityTracker::new();
                tracker.transition(state);

                let status = cloud_status(
                    &active_config(),
                    &repository,
                    &tracker,
                    &ReportQueue::new(8),
                )
                .await;
                assert_eq!(status.state, state, "{state:?} must round-trip");

                let needs_a_human = matches!(
                    state,
                    CloudConnectivityState::NotConfigured
                        | CloudConnectivityState::Disconnected
                        | CloudConnectivityState::Suspended
                        | CloudConnectivityState::Revoked
                );
                assert_eq!(
                    status.remediation.is_some(),
                    needs_a_human,
                    "{state:?}: guidance must appear exactly when a human is needed"
                );
            }

            fs::remove_dir_all(&dir).ok();
        }

        #[tokio::test]
        async fn forgetting_removes_every_cloud_record() {
            let (secrets, dir) = store();
            secrets
                .store_session_credential(&SessionCredential::new("session"))
                .unwrap();
            secrets
                .store_provider_credential(&ProviderCredential::new("provider"))
                .unwrap();

            let repository = SqliteRepository::open_in_memory().unwrap();
            repository
                .save_cloud_enrollment(&argus_domain::CloudEnrollment::new(
                    Uuid::new_v4(),
                    Uuid::new_v4(),
                    "web-01",
                    now(),
                ))
                .await
                .unwrap();

            let settings = ManagedSettingsStore::new(dir.join("cloud-settings.toml"));
            settings.write(&toml::Table::new()).unwrap();

            forget_enrollment(&repository, &secrets, &settings)
                .await
                .expect("forgotten");

            assert_eq!(repository.get_cloud_enrollment().await.unwrap(), None);
            assert_eq!(secrets.read_session_credential().unwrap(), None);
            assert_eq!(secrets.read_provider_credential().unwrap(), None);
            assert!(settings.read().unwrap().is_none());

            fs::remove_dir_all(&dir).ok();
        }

        #[tokio::test]
        async fn forgetting_twice_is_not_an_error() {
            // T100: idempotent, so a second run after a partial failure is safe.
            let (secrets, dir) = store();
            let repository = SqliteRepository::open_in_memory().unwrap();
            let settings = ManagedSettingsStore::new(dir.join("cloud-settings.toml"));

            forget_enrollment(&repository, &secrets, &settings)
                .await
                .expect("first");
            forget_enrollment(&repository, &secrets, &settings)
                .await
                .expect("second");

            fs::remove_dir_all(&dir).ok();
        }
    }

    mod trust_lifecycle_guidance {
        use super::*;

        #[tokio::test]
        async fn revocation_and_suspension_both_tell_the_operator_what_to_do() {
            // T096: a condition needing human action must come with guidance.
            let mut tracker = ConnectivityTracker::new();

            tracker.transition(CloudConnectivityState::Revoked);
            let revoked = tracker.remediation().expect("revocation needs guidance");
            assert!(revoked.contains("re-enrollment"), "{revoked}");
            assert!(!tracker.allows_reconnect(), "revocation is terminal");

            tracker.transition(CloudConnectivityState::Suspended);
            let suspended = tracker.remediation().expect("suspension needs guidance");
            assert!(suspended.contains("resumed"), "{suspended}");
            assert!(
                tracker.allows_reconnect(),
                "suspension must stay recoverable"
            );
        }
    }
}
