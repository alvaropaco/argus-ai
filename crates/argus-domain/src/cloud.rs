//! Cloud-facing domain value objects.
//!
//! Vocabulary-neutral by design: no protocol, transport, or wire type appears
//! here, so the cloud integration stays removable and the domain model stays
//! infrastructure-agnostic (constitution Principle 6). Wire shapes live in
//! `argus-cloud::protocol`; the translation between the two lives in
//! `argus-cloud::mapping`.
//!
//! The invariants these types encode are the ones the cloud feature depends on:
//! only revocation is terminal, approval is per invocation and expires, and the
//! refusal vocabulary is closed at five reasons so every refusal is diagnosable.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{BlastRadius, CapabilityDescriptor, CapabilityId, EnvironmentId, RiskClass};

/// The installation's own view of its cloud relationship.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloudConnectivityState {
    /// Cloud disabled or never enrolled.
    NotConfigured,
    /// Dialing or handshaking.
    Connecting,
    /// Authenticated and heartbeating.
    Connected,
    /// Authenticated but impaired (throttled, buffer pressure, send failures).
    Degraded,
    /// Connection lost; reconnecting with backoff.
    Disconnected,
    /// The cloud refused the connection; retried conservatively.
    Suspended,
    /// The cloud refused with `REVOKED`. Terminal until re-enrollment.
    Revoked,
}

impl CloudConnectivityState {
    /// Whether the installation may attempt a connection in this state.
    pub fn allows_reconnect(self) -> bool {
        !matches!(self, Self::Revoked | Self::NotConfigured)
    }

    /// Whether this state is terminal without operator action.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Revoked)
    }

    /// Whether the cloud currently sees this installation as reachable.
    pub fn is_online(self) -> bool {
        matches!(self, Self::Connected | Self::Degraded)
    }
}

/// The enrollment's standing, which decides whether a connection is permitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustStatus {
    Active,
    Suspended,
    Revoked,
}

impl TrustStatus {
    /// Revocation is the only terminal trust state.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Revoked)
    }

    /// Whether a connection attempt should be made.
    pub fn permits_connection(self) -> bool {
        matches!(self, Self::Active)
    }
}

/// Trust lifecycle details for the local installation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustState {
    pub status: TrustStatus,
    pub credential_epoch: u64,
    pub last_change_at: DateTime<Utc>,
    pub last_change_reason: Option<String>,
}

impl TrustState {
    /// A fresh, active trust state at epoch zero.
    pub fn active(now: DateTime<Utc>) -> Self {
        Self {
            status: TrustStatus::Active,
            credential_epoch: 0,
            last_change_at: now,
            last_change_reason: None,
        }
    }

    /// Applies a lifecycle change and advances state, recording why.
    pub fn transition(
        &mut self,
        status: TrustStatus,
        reason: impl Into<String>,
        at: DateTime<Utc>,
    ) {
        self.status = status;
        self.last_change_at = at;
        self.last_change_reason = Some(reason.into());
    }

    /// Records a credential rotation by advancing the epoch.
    pub fn rotate_credential(&mut self, at: DateTime<Utc>) {
        self.credential_epoch = self.credential_epoch.saturating_add(1);
        self.last_change_at = at;
        self.last_change_reason = Some("credential rotated".to_string());
    }
}

/// Lifecycle status of a cloud-issued command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloudCommandStatus {
    Received,
    Refused,
    Executing,
    Acknowledged,
    Failed,
}

impl CloudCommandStatus {
    /// Terminal statuses must never be re-executed on re-delivery.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Refused | Self::Acknowledged | Self::Failed)
    }
}

/// Why an invocation was refused.
///
/// Closed at exactly five values, each produced by one step of the validation
/// ladder in `contracts/privileged-execution.md` §2. A sixth reason requires an
/// ADR amendment (ADR-0020 §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefusalReason {
    /// The capability is not in the currently published surface.
    UnknownCapability,
    /// The input does not match the capability's declared schema.
    InvalidInput,
    /// A local operator disabled cloud-issued privileged execution.
    ExecutionDisabled,
    /// Local policy denied the invocation.
    PolicyDenied,
    /// The capability requires approval and none has been granted.
    ApprovalRequired,
}

impl RefusalReason {
    /// Operator-facing explanation, secret-free.
    pub fn describe(self) -> &'static str {
        match self {
            Self::UnknownCapability => "the capability is not published by this installation",
            Self::InvalidInput => "the input does not match the capability's declared schema",
            Self::ExecutionDisabled => "cloud-issued privileged execution is disabled locally",
            Self::PolicyDenied => "local policy denied the invocation",
            Self::ApprovalRequired => "local approval is required and has not been granted",
        }
    }
}

/// A capability invocation requested by the cloud.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CloudCommand {
    /// Also the idempotency key for this invocation.
    pub command_id: Uuid,
    pub environment_id: EnvironmentId,
    pub capability_id: CapabilityId,
    pub input: Value,
    pub correlation_id: Option<String>,
    pub status: CloudCommandStatus,
    pub refusal_reason: Option<RefusalReason>,
    pub result: Option<Value>,
    pub received_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    /// Re-deliveries observed, so duplicate suppression is auditable.
    pub attempt_count: u32,
}

impl CloudCommand {
    /// A newly received invocation, awaiting validation.
    pub fn received(
        command_id: Uuid,
        environment_id: EnvironmentId,
        capability_id: CapabilityId,
        input: Value,
        correlation_id: Option<String>,
        at: DateTime<Utc>,
    ) -> Self {
        Self {
            command_id,
            environment_id,
            capability_id,
            input,
            correlation_id,
            status: CloudCommandStatus::Received,
            refusal_reason: None,
            result: None,
            received_at: at,
            completed_at: None,
            attempt_count: 1,
        }
    }

    /// A duplicate must never be re-executed: a terminal command re-reports its
    /// stored result, and an in-flight command is suppressed because no result
    /// exists yet (ADR-0022 §2, §3).
    pub fn is_duplicate_of(&self, incoming_command_id: Uuid) -> bool {
        self.command_id == incoming_command_id
    }

    /// Marks the command refused and records why.
    pub fn refuse(&mut self, reason: RefusalReason, at: DateTime<Utc>) {
        self.status = CloudCommandStatus::Refused;
        self.refusal_reason = Some(reason);
        self.completed_at = Some(at);
    }
}

/// The installation's own authorization verdict for an invocation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionDecision {
    pub decision_id: Uuid,
    pub command_id: Uuid,
    pub outcome: DecisionOutcome,
    pub risk_class: RiskClass,
    pub blast_radius: BlastRadius,
    pub policy_id: String,
    pub reason: String,
    pub decided_at: DateTime<Utc>,
}

/// Authorization outcome for a cloud-issued invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionOutcome {
    Permitted,
    Denied,
    RequireApproval,
}

impl DecisionOutcome {
    /// Only a permitted decision may reach the executor.
    pub fn permits_execution(self) -> bool {
        matches!(self, Self::Permitted)
    }
}

/// A locally granted authorization for one approval-required invocation.
///
/// Bound to a specific `command_id` rather than a capability: a standing
/// capability-level approval would silently convert a reviewed action into an
/// unreviewed one (ADR-0020 §3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionApproval {
    pub approval_id: Uuid,
    pub command_id: Uuid,
    pub granted_by: String,
    pub granted_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub state: ApprovalState,
}

/// Lifecycle of an execution approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalState {
    Granted,
    Denied,
    Expired,
}

impl ExecutionApproval {
    /// Grants approval for one invocation, bounded in time.
    pub fn grant(
        command_id: Uuid,
        granted_by: impl Into<String>,
        granted_at: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> Self {
        Self {
            approval_id: Uuid::new_v4(),
            command_id,
            granted_by: granted_by.into(),
            granted_at,
            expires_at,
            state: ApprovalState::Granted,
        }
    }

    /// Whether this approval authorizes execution of `command_id` at `now`.
    ///
    /// An expired approval authorizes nothing, even though it was granted.
    pub fn authorizes(&self, command_id: Uuid, now: DateTime<Utc>) -> bool {
        self.state == ApprovalState::Granted
            && self.command_id == command_id
            && now < self.expires_at
    }
}

/// Outcome of applying a managed configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplyStatus {
    None,
    Applied,
    Rejected,
    Failed,
}

/// A cloud-assigned configuration the installation must make effective.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManagedConfiguration {
    pub configuration_id: Uuid,
    /// The cloud-defined kind, e.g. `agent_team` or `orchestration`.
    pub kind: String,
    pub current_version_id: Option<Uuid>,
    pub current_version_number: i64,
    pub content_hash: Option<String>,
    pub applied_at: Option<DateTime<Utc>>,
    pub apply_status: ApplyStatus,
    pub apply_reason: Option<String>,
}

impl ManagedConfiguration {
    /// Whether a delivery should be applied.
    ///
    /// Stale deliveries are ignored rather than re-applied, so the version
    /// reported as applied is always the version actually held (FR-031).
    pub fn accepts_version(&self, incoming_version_number: i64) -> bool {
        incoming_version_number > self.current_version_number
    }

    /// Whether the configuration is currently in effect.
    pub fn is_effective(&self) -> bool {
        self.apply_status == ApplyStatus::Applied
    }
}

/// Where the effective value of a managed setting came from.
///
/// Precedence is `Default < File < Cloud < Flag`; the CLI flag ranks above the
/// cloud deliberately so an operator always retains a local remedy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectiveSource {
    Default,
    File,
    Cloud,
    Flag,
}

impl EffectiveSource {
    /// Numeric rank, higher wins.
    pub fn rank(self) -> u8 {
        match self {
            Self::Default => 0,
            Self::File => 1,
            Self::Cloud => 2,
            Self::Flag => 3,
        }
    }

    /// The source with the higher precedence.
    pub fn higher(self, other: Self) -> Self {
        if self.rank() >= other.rank() {
            self
        } else {
            other
        }
    }
}

/// A configuration field the cloud owns the value of.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManagedSetting {
    /// Dotted key, e.g. `model.provider`.
    pub key: String,
    /// Non-secret value, absent when the value is secret.
    pub value: Option<Value>,
    /// Handle to the secret store when the value is secret.
    pub secret_ref: Option<String>,
    pub configuration_id: Uuid,
    pub effective_source: EffectiveSource,
    pub updated_at: DateTime<Utc>,
}

impl ManagedSetting {
    /// Whether this setting holds secret material rather than a plain value.
    pub fn is_secret(&self) -> bool {
        self.secret_ref.is_some()
    }
}

/// What the installation currently holds for one managed configuration.
///
/// This is the authoritative answer to the cloud's reconciliation request, so it
/// must reflect what is genuinely in effect, never what was merely received.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedConfigurationState {
    pub configuration_id: Uuid,
    /// `None` means nothing is applied, and must be reported as such.
    pub applied_version_id: Option<Uuid>,
    pub applied_version_number: i64,
    pub content_hash: Option<String>,
    pub updated_at: DateTime<Utc>,
}

/// Occupancy of the bounded report buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportBuffer {
    pub capacity: usize,
    pub count: usize,
    pub dropped_total: u64,
    pub last_drain_at: Option<DateTime<Utc>>,
}

impl ReportBuffer {
    /// A buffer with the given record capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity,
            count: 0,
            dropped_total: 0,
            last_drain_at: None,
        }
    }

    /// Whether the buffer holds no undelivered records.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Whether the buffer is at capacity.
    pub fn is_full(&self) -> bool {
        self.count >= self.capacity
    }

    /// Records an enqueue, discarding the oldest entry when full.
    ///
    /// Returns `true` when a record was dropped, so the loss is counted and
    /// surfaced rather than hidden (FR-020).
    pub fn record_enqueue(&mut self) -> bool {
        if self.is_full() {
            self.dropped_total = self.dropped_total.saturating_add(1);
            true
        } else {
            self.count += 1;
            false
        }
    }

    /// Records delivery of `n` records.
    pub fn record_drain(&mut self, n: usize, at: DateTime<Utc>) {
        self.count = self.count.saturating_sub(n);
        self.last_drain_at = Some(at);
    }
}

/// The operator-mediated binding of this installation to one organization.
///
/// An installation holds at most one enrollment for its lifetime; changing
/// organization requires forgetting and re-enrolling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudEnrollment {
    pub installation_id: Uuid,
    pub tenant_id: Uuid,
    pub instance_name: String,
    pub enrolled_at: DateTime<Utc>,
    /// Operator identity as reported by the cloud, for local audit.
    pub enrolled_by: Option<String>,
}

impl CloudEnrollment {
    pub fn new(
        installation_id: Uuid,
        tenant_id: Uuid,
        instance_name: impl Into<String>,
        enrolled_at: DateTime<Utc>,
    ) -> Self {
        Self {
            installation_id,
            tenant_id,
            instance_name: instance_name.into(),
            enrolled_at,
            enrolled_by: None,
        }
    }
}

/// The authenticated relationship with the cloud, as last observed.
///
/// Persisted so an operator can answer "is this host talking to the cloud, and
/// when did it last succeed?" after a restart, without waiting for a reconnect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudConnection {
    pub negotiated_protocol_version: String,
    pub heartbeat_interval_seconds: u64,
    pub last_exchange_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub backoff_attempt: u32,
}

impl CloudConnection {
    pub fn connected(negotiated_protocol_version: impl Into<String>) -> Self {
        Self {
            negotiated_protocol_version: negotiated_protocol_version.into(),
            heartbeat_interval_seconds: 20,
            last_exchange_at: None,
            last_error: None,
            backoff_attempt: 0,
        }
    }

    /// Records a successful exchange and clears the backoff schedule.
    pub fn record_success(&mut self, at: DateTime<Utc>) {
        self.last_exchange_at = Some(at);
        self.last_error = None;
        self.backoff_attempt = 0;
    }

    /// Records a failure and advances the backoff schedule.
    pub fn record_failure(&mut self, error: impl Into<String>) {
        self.last_error = Some(error.into());
        self.backoff_attempt = self.backoff_attempt.saturating_add(1);
    }
}

/// A versioned snapshot of the capability surface published to the cloud.
///
/// Superseded versions are retained rather than replaced, so the cloud keeps
/// history and a no-op re-publish can be detected by content hash.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapabilityPublication {
    pub publication_id: Uuid,
    pub version: String,
    pub capabilities: Vec<CapabilityDescriptor>,
    pub published_at: DateTime<Utc>,
    pub content_hash: String,
}

impl CapabilityPublication {
    /// Builds a publication, deriving its content hash.
    pub fn new(
        version: impl Into<String>,
        capabilities: Vec<CapabilityDescriptor>,
        published_at: DateTime<Utc>,
    ) -> Self {
        let content_hash = Self::hash(&capabilities);
        Self {
            publication_id: Uuid::new_v4(),
            version: version.into(),
            capabilities,
            published_at,
            content_hash,
        }
    }

    /// Stable, dependency-free hash over the canonical serialisation.
    ///
    /// Used only to skip a no-op re-publish, so a fast non-cryptographic hash is
    /// sufficient; it makes no security claim.
    pub fn hash(capabilities: &[CapabilityDescriptor]) -> String {
        let canonical = serde_json::to_string(capabilities).unwrap_or_default();
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in canonical.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        format!("{hash:016x}")
    }

    /// Whether publishing `capabilities` would change nothing.
    pub fn is_noop_for(&self, capabilities: &[CapabilityDescriptor]) -> bool {
        self.content_hash == Self::hash(capabilities)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 21, 12, 0, 0).unwrap()
    }

    #[test]
    fn only_revocation_is_terminal() {
        assert!(CloudConnectivityState::Revoked.is_terminal());
        assert!(!CloudConnectivityState::Suspended.is_terminal());
        assert!(!CloudConnectivityState::Disconnected.is_terminal());
        assert!(TrustStatus::Revoked.is_terminal());
        assert!(!TrustStatus::Suspended.is_terminal());
    }

    #[test]
    fn revoked_and_unconfigured_installations_do_not_reconnect() {
        assert!(!CloudConnectivityState::Revoked.allows_reconnect());
        assert!(!CloudConnectivityState::NotConfigured.allows_reconnect());
        assert!(CloudConnectivityState::Disconnected.allows_reconnect());
        assert!(CloudConnectivityState::Suspended.allows_reconnect());
    }

    #[test]
    fn connectivity_state_serde_is_snake_case() {
        assert_eq!(
            serde_json::to_string(&CloudConnectivityState::NotConfigured).unwrap(),
            "\"not_configured\""
        );
        assert_eq!(
            serde_json::to_string(&CloudConnectivityState::Degraded).unwrap(),
            "\"degraded\""
        );
    }

    #[test]
    fn refusal_vocabulary_is_closed_at_five_reasons() {
        let all = [
            RefusalReason::UnknownCapability,
            RefusalReason::InvalidInput,
            RefusalReason::ExecutionDisabled,
            RefusalReason::PolicyDenied,
            RefusalReason::ApprovalRequired,
        ];
        assert_eq!(all.len(), 5);
        for reason in all {
            assert!(!reason.describe().is_empty(), "{reason:?}");
            serde_json::to_value(reason).expect("serialises");
        }
        // `not_executable` is deliberately absent; it had no producing step.
        assert!(serde_json::from_str::<RefusalReason>("\"not_executable\"").is_err());
    }

    #[test]
    fn only_permitted_reaches_the_executor() {
        assert!(DecisionOutcome::Permitted.permits_execution());
        assert!(!DecisionOutcome::Denied.permits_execution());
        assert!(!DecisionOutcome::RequireApproval.permits_execution());
    }

    #[test]
    fn approval_is_bound_to_one_invocation_and_expires() {
        let command = Uuid::new_v4();
        let other = Uuid::new_v4();
        let approval = ExecutionApproval::grant(
            command,
            "uid=1000",
            ts(),
            ts() + chrono::Duration::minutes(10),
        );

        assert!(approval.authorizes(command, ts()));
        assert!(
            !approval.authorizes(other, ts()),
            "must not leak to another command"
        );
        assert!(
            !approval.authorizes(command, ts() + chrono::Duration::minutes(11)),
            "an expired approval authorizes nothing"
        );
    }

    #[test]
    fn stale_configuration_versions_are_ignored() {
        let managed = ManagedConfiguration {
            configuration_id: Uuid::new_v4(),
            kind: "agent_team".into(),
            current_version_id: Some(Uuid::new_v4()),
            current_version_number: 5,
            content_hash: Some("hash".into()),
            applied_at: Some(ts()),
            apply_status: ApplyStatus::Applied,
            apply_reason: None,
        };
        assert!(!managed.accepts_version(5), "same version is a no-op");
        assert!(!managed.accepts_version(4), "older version is stale");
        assert!(managed.accepts_version(6));
        assert!(managed.is_effective());
    }

    #[test]
    fn cloud_settings_are_overridden_by_flags_but_not_by_files() {
        assert_eq!(
            EffectiveSource::Cloud.higher(EffectiveSource::File),
            EffectiveSource::Cloud
        );
        assert_eq!(
            EffectiveSource::Cloud.higher(EffectiveSource::Flag),
            EffectiveSource::Flag
        );
        assert_eq!(
            EffectiveSource::Default.higher(EffectiveSource::File),
            EffectiveSource::File
        );
    }

    #[test]
    fn secrets_are_flagged_not_inlined() {
        let secret = ManagedSetting {
            key: "model.api_token".into(),
            value: None,
            secret_ref: Some("model-api-token".into()),
            configuration_id: Uuid::new_v4(),
            effective_source: EffectiveSource::Cloud,
            updated_at: ts(),
        };
        assert!(secret.is_secret());

        let plain = ManagedSetting {
            key: "model.provider".into(),
            value: Some(serde_json::json!("openai")),
            secret_ref: None,
            configuration_id: Uuid::new_v4(),
            effective_source: EffectiveSource::Cloud,
            updated_at: ts(),
        };
        assert!(!plain.is_secret());
    }

    #[test]
    fn buffer_discards_oldest_and_counts_the_loss() {
        let mut buffer = ReportBuffer::with_capacity(3);
        assert!(!buffer.record_enqueue());
        assert!(!buffer.record_enqueue());
        assert!(!buffer.record_enqueue());
        assert!(buffer.is_full());

        assert!(
            buffer.record_enqueue(),
            "fourth enqueue must drop the oldest"
        );
        assert_eq!(buffer.dropped_total, 1);
        assert_eq!(buffer.count, 3, "occupancy stays at capacity");

        buffer.record_drain(2, ts());
        assert_eq!(buffer.count, 1);
        assert_eq!(buffer.last_drain_at, Some(ts()));
    }

    #[test]
    fn empty_buffer_reports_empty() {
        let buffer = ReportBuffer::with_capacity(10);
        assert!(buffer.is_empty());
        assert_eq!(buffer.dropped_total, 0);
        assert_eq!(buffer.last_drain_at, None);
    }

    #[test]
    fn trust_lifecycle_transitions_are_recorded() {
        let mut trust = TrustState::active(ts());
        assert!(trust.status.permits_connection());

        trust.transition(TrustStatus::Suspended, "suspended by operator", ts());
        assert!(!trust.status.permits_connection());
        assert_eq!(
            trust.last_change_reason.as_deref(),
            Some("suspended by operator")
        );

        trust.transition(TrustStatus::Active, "resumed", ts());
        assert!(trust.status.permits_connection());

        trust.rotate_credential(ts());
        assert_eq!(trust.credential_epoch, 1);
    }

    #[test]
    fn a_terminal_command_must_not_be_reexecuted() {
        let mut command = CloudCommand::received(
            Uuid::new_v4(),
            EnvironmentId::new(),
            CapabilityId::new("host.service.restart").unwrap(),
            serde_json::json!({"unit": "nginx"}),
            None,
            ts(),
        );
        assert_eq!(command.status, CloudCommandStatus::Received);
        assert!(!command.status.is_terminal());
        assert!(command.is_duplicate_of(command.command_id));

        command.refuse(RefusalReason::ApprovalRequired, ts());
        assert_eq!(command.status, CloudCommandStatus::Refused);
        assert!(command.status.is_terminal());
        assert!(command.is_duplicate_of(command.command_id));
        assert!(!command.is_duplicate_of(Uuid::new_v4()));
        assert_eq!(
            command.refusal_reason,
            Some(RefusalReason::ApprovalRequired)
        );
        assert_eq!(command.completed_at, Some(ts()));
    }

    #[test]
    fn cloud_command_serde_round_trips() {
        let command = CloudCommand::received(
            Uuid::new_v4(),
            EnvironmentId::new(),
            CapabilityId::new("argus.health.read").unwrap(),
            serde_json::json!({}),
            Some("corr-1".into()),
            ts(),
        );
        let json = serde_json::to_string(&command).unwrap();
        let back: CloudCommand = serde_json::from_str(&json).unwrap();
        assert_eq!(back, command);
    }
}
