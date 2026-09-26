//! Message payloads for agent protocol v1.0.0.
//!
//! Field names and enum spellings mirror the cloud's published schemas exactly.
//! The cloud validates these with its own schema, so a local spelling drift is a
//! rejection, not a cosmetic difference — see the vocabulary rules in
//! `contracts/agent-protocol-conformance.md` §5 and ADR-0016.
//!
//! Bounds from `contracts/agent-protocol-conformance.md` §3 are declared as
//! constants here so the caller enforces them before sending rather than relying
//! on the cloud to reject (FR-019).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

// --- Bounds (contract §3) ---

/// Maximum telemetry payload size.
pub const MAX_TELEMETRY_BYTES: usize = 256 * 1024;
/// Maximum events per report.
pub const MAX_EVENTS_PER_REPORT: usize = 500;
/// Minimum events per report.
pub const MIN_EVENTS_PER_REPORT: usize = 1;
/// Maximum activities per report.
pub const MAX_ACTIVITIES_PER_REPORT: usize = 500;
/// Minimum activities per report.
pub const MIN_ACTIVITIES_PER_REPORT: usize = 1;
/// Maximum capabilities per publication.
pub const MAX_CAPABILITIES_PER_PUBLICATION: usize = 1000;
/// Maximum `agent_version` length.
pub const MAX_AGENT_VERSION_LEN: usize = 50;
/// Maximum `hostname` length.
pub const MAX_HOSTNAME_LEN: usize = 255;
/// Maximum health summary length.
pub const MAX_HEALTH_SUMMARY_LEN: usize = 4000;
/// Maximum acknowledgement reason length.
pub const MAX_ACK_REASON_LEN: usize = 500;
/// Maximum result reason length.
pub const MAX_RESULT_REASON_LEN: usize = 2000;
/// Minimum `public_key` length.
pub const MIN_PUBLIC_KEY_LEN: usize = 32;
/// Maximum `public_key` length.
pub const MAX_PUBLIC_KEY_LEN: usize = 2000;
/// Minimum `challenge_signature` length.
pub const MIN_CHALLENGE_SIGNATURE_LEN: usize = 16;
/// Maximum `challenge_signature` length.
pub const MAX_CHALLENGE_SIGNATURE_LEN: usize = 2000;
/// Minimum pairing code length.
pub const MIN_PAIRING_CODE_LEN: usize = 8;
/// Maximum pairing code length.
pub const MAX_PAIRING_CODE_LEN: usize = 64;
/// Maximum capability id length.
pub const MAX_CAPABILITY_ID_LEN: usize = 200;
/// Maximum schema/policy/field name length used by the cloud's schemas.
pub const MAX_SHORT_TEXT_LEN: usize = 200;

// --- Cloud → installation ---

/// `handshake.hello`: sent by the cloud on connect.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HandshakeHelloPayload {
    pub cloud_id: String,
    pub cloud_instance: String,
    pub server_time: DateTime<Utc>,
    pub supported_protocol_versions: Vec<String>,
    /// Nonce the installation signs to prove identity.
    pub challenge: String,
    pub heartbeat_interval_seconds: u64,
}

/// `handshake.ready`: the cloud accepted the session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandshakeReadyPayload {
    pub negotiated_protocol_version: String,
    pub instance_id: Uuid,
    pub tenant_id: Uuid,
    #[serde(default)]
    pub config_pull_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_schema_version: Option<String>,
}

/// `handshake.reject`: the cloud refused the session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandshakeRejectPayload {
    pub code: String,
    pub message: String,
}

/// `pairing.granted`: enrollment succeeded.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingGrantedPayload {
    pub instance_id: Uuid,
    pub tenant_id: Uuid,
    pub instance_name: String,
    /// Secret material. Never logged, never persisted outside the secret store.
    pub session_token: String,
    pub session_expires_at: DateTime<Utc>,
    pub negotiated_protocol_version: String,
}

/// `pairing.denied`: enrollment was refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingDeniedPayload {
    pub code: super::errors::PairingDenialCode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// `config.apply`: a managed configuration version to make effective.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigApplyPayload {
    pub configuration_id: Uuid,
    pub version_id: Uuid,
    pub version_number: i64,
    pub content_hash: String,
    pub content: Map<String, Value>,
    #[serde(default)]
    pub mode: ConfigApplyMode,
}

/// How a delivered configuration should be treated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfigApplyMode {
    #[default]
    Apply,
    Rollback,
}

/// `command.invoke`: a capability invocation request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandInvokePayload {
    pub command_id: Uuid,
    pub capability_id: String,
    pub input: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
}

/// `config.pull.request`: asks the installation to report applied versions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigPullRequestPayload {}

/// `ping`: liveness. Both directions; `ts` is optional.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PingPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts: Option<DateTime<Utc>>,
}

/// `stream.throttle`: reduce reporting cadence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamThrottlePayload {
    pub interval_ms: u64,
}

/// `session.rotate`: adopt a new credential.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRotatePayload {
    pub rotation_token: String,
    pub expires_at: DateTime<Utc>,
}

/// Any `*.ack`: acknowledgement of a report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IngestAckPayload {
    pub message_id: String,
    pub accepted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

// --- Installation → cloud ---

/// `pairing.redeem`: enroll using an operator-supplied code.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingRedeemPayload {
    pub code: String,
    pub protocol_version: String,
    pub agent_version: String,
    pub hostname: String,
    /// Non-secret; see ADR-0023 for why this must remain non-decodable in v1.
    pub public_key: String,
    pub challenge_signature: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_schema_version: Option<String>,
}

/// `handshake.authenticate`: prove identity on every connection.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandshakeAuthenticatePayload {
    pub instance_id: Uuid,
    pub protocol_version: String,
    pub agent_version: String,
    pub hostname: String,
    pub challenge_signature: String,
    /// The cloud-issued session credential, when the installation still holds one.
    ///
    /// Optional since ADR-0026: identity is proven by `challenge_signature`
    /// against the key stored at enrollment, so this is carried for protocol
    /// compatibility and is not required to authenticate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_proof: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_schema_version: Option<String>,
}

/// `telemetry.report`: bounded metrics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TelemetryReportPayload {
    pub metrics: Map<String, Value>,
    pub reported_at: DateTime<Utc>,
}

/// `health.report`: environment-level assessment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HealthReportPayload {
    pub state: HealthStateWire,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Map<String, Value>>,
    pub reported_at: DateTime<Utc>,
}

/// The cloud's health vocabulary. Not the local one — see ADR-0016 §4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthStateWire {
    Healthy,
    Degraded,
    Critical,
    Unknown,
}

/// The cloud's severity vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SeverityWire {
    Info,
    Warning,
    Error,
}

/// The cloud's risk classification. Note the casing: `LowRisk`, not `low_risk`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum RiskClassWire {
    Read,
    LowRisk,
    Controlled,
    HighRisk,
    Destructive,
}

/// The cloud's reversibility vocabulary, including the `n/a` spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReversibilityWire {
    #[serde(rename = "reversible")]
    Reversible,
    #[serde(rename = "irreversible")]
    Irreversible,
    #[serde(rename = "n/a")]
    NotApplicable,
}

/// A projected local event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReportedEvent {
    /// The local event UUID. The cloud deduplicates on it, so it must be stable
    /// across retries.
    pub source_event_id: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub severity: SeverityWire,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub causation_id: Option<String>,
    pub reported_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Map<String, Value>>,
}

/// `events.report`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventsReportPayload {
    pub events: Vec<ReportedEvent>,
}

/// Outcome vocabulary for activities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityOutcomeWire {
    Succeeded,
    Failed,
    Denied,
    PendingApproval,
}

/// A summary of an attempted or performed operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivityRecordWire {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent_id: Option<String>,
    pub capability_id: String,
    pub risk_class: RiskClassWire,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    pub outcome: ActivityOutcomeWire,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
}

/// `activities.report`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivitiesReportPayload {
    pub activities: Vec<ActivityRecordWire>,
}

/// A capability as published to the cloud.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapabilityDescriptorWire {
    pub id: String,
    pub provider: String,
    pub operation: String,
    pub risk_class: RiskClassWire,
    pub reversibility: ReversibilityWire,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_approval: Option<bool>,
}

/// `capabilities.publish`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapabilitiesPublishPayload {
    pub version: String,
    pub capabilities: Vec<CapabilityDescriptorWire>,
}

/// Outcome of applying a managed configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfigResultStatus {
    Applied,
    Rejected,
    Failed,
}

/// `config.result`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigResultPayload {
    pub version_id: Uuid,
    pub status: ConfigResultStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_at: Option<DateTime<Utc>>,
}

/// Outcome of a capability invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommandResultStatus {
    Acknowledged,
    Failed,
    Refused,
}

/// `command.result`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandResultPayload {
    pub command_id: Uuid,
    pub status: CommandResultStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// One entry in the applied-configuration report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigurationStateEntry {
    pub configuration_id: Uuid,
    /// `None` means nothing is applied, and must be reported as such rather than
    /// omitted.
    pub applied_version_id: Option<Uuid>,
}

/// `config.state`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigStatePayload {
    pub configurations: Vec<ConfigurationStateEntry>,
}

/// `pong`: reply to `ping`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PongPayload {}

/// Four payloads carry secret material (the pairing code, the session credential,
/// the session proof, and the rotation token). They deliberately do not derive
/// `Debug`, so a stray `{:?}` in a log line cannot leak a credential.
const REDACTED: &str = "<redacted>";

impl std::fmt::Debug for PairingRedeemPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairingRedeemPayload")
            .field("code", &REDACTED)
            .field("protocol_version", &self.protocol_version)
            .field("agent_version", &self.agent_version)
            .field("hostname", &self.hostname)
            .field("public_key", &self.public_key)
            .field("challenge_signature", &REDACTED)
            .field("capability_schema_version", &self.capability_schema_version)
            .finish()
    }
}

impl std::fmt::Debug for PairingGrantedPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairingGrantedPayload")
            .field("instance_id", &self.instance_id)
            .field("tenant_id", &self.tenant_id)
            .field("instance_name", &self.instance_name)
            .field("session_token", &REDACTED)
            .field("session_expires_at", &self.session_expires_at)
            .field(
                "negotiated_protocol_version",
                &self.negotiated_protocol_version,
            )
            .finish()
    }
}

impl std::fmt::Debug for HandshakeAuthenticatePayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HandshakeAuthenticatePayload")
            .field("instance_id", &self.instance_id)
            .field("protocol_version", &self.protocol_version)
            .field("agent_version", &self.agent_version)
            .field("hostname", &self.hostname)
            .field("challenge_signature", &REDACTED)
            .field("session_proof", &REDACTED)
            .field("capability_schema_version", &self.capability_schema_version)
            .finish()
    }
}

impl std::fmt::Debug for SessionRotatePayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionRotatePayload")
            .field("rotation_token", &REDACTED)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn risk_class_serialises_pascal_case_not_local_snake_case() {
        // This is the drift ADR-0016 §2 exists to prevent.
        assert_eq!(
            serde_json::to_string(&RiskClassWire::LowRisk).unwrap(),
            "\"LowRisk\""
        );
        assert_eq!(
            serde_json::to_string(&RiskClassWire::HighRisk).unwrap(),
            "\"HighRisk\""
        );
    }

    #[test]
    fn reversibility_serialises_the_slash_spelling() {
        assert_eq!(
            serde_json::to_string(&ReversibilityWire::NotApplicable).unwrap(),
            "\"n/a\""
        );
        assert_eq!(
            serde_json::to_string(&ReversibilityWire::Irreversible).unwrap(),
            "\"irreversible\""
        );
    }

    #[test]
    fn health_and_severity_use_lowercase() {
        assert_eq!(
            serde_json::to_string(&HealthStateWire::Critical).unwrap(),
            "\"critical\""
        );
        assert_eq!(
            serde_json::to_string(&SeverityWire::Warning).unwrap(),
            "\"warning\""
        );
    }

    #[test]
    fn activity_outcome_uses_snake_case() {
        assert_eq!(
            serde_json::to_string(&ActivityOutcomeWire::PendingApproval).unwrap(),
            "\"pending_approval\""
        );
    }

    #[test]
    fn event_type_field_is_named_type_on_the_wire() {
        let event = ReportedEvent {
            source_event_id: "11111111-1111-1111-1111-111111111111".into(),
            event_type: "argus.ready".into(),
            severity: SeverityWire::Info,
            subject: None,
            correlation_id: None,
            causation_id: None,
            reported_at: Utc::now(),
            payload: None,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert!(json.get("type").is_some());
        assert!(json.get("event_type").is_none());
    }

    #[test]
    fn config_apply_mode_defaults_to_apply() {
        let value = serde_json::json!({
            "configuration_id": Uuid::nil(),
            "version_id": Uuid::nil(),
            "version_number": 1,
            "content_hash": "abc",
            "content": {}
        });
        let parsed: ConfigApplyPayload = serde_json::from_value(value).unwrap();
        assert_eq!(parsed.mode, ConfigApplyMode::Apply);
    }

    #[test]
    fn config_state_reports_null_applied_version_explicitly() {
        let payload = ConfigStatePayload {
            configurations: vec![ConfigurationStateEntry {
                configuration_id: Uuid::nil(),
                applied_version_id: None,
            }],
        };
        let json = serde_json::to_value(&payload).unwrap();
        assert!(json["configurations"][0]["applied_version_id"].is_null());
    }
}
