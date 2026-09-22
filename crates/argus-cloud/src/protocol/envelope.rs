//! Versioned envelope wrapping every installation ↔ cloud message.
//!
//! One JSON object per frame. Unknown fields MUST be ignored (additive
//! evolution), so the wire-faithful [`Envelope`] keeps the message type as a
//! string and [`MessageType`] is a routing helper rather than a hard parse gate.
//! An unrecognised type is handled by the caller as `UNKNOWN_TYPE` and ignored,
//! never treated as a fatal decode failure.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// A single frame on the agent channel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    /// Semantic protocol version, negotiated at handshake.
    pub protocol_version: String,
    /// Unique id for this message.
    pub message_id: Uuid,
    /// Groups a request with its response. Absent and `null` are both valid.
    #[serde(default)]
    pub correlation_id: Option<Uuid>,
    /// When the sender created the message.
    pub sent_at: DateTime<Utc>,
    /// Dotted, namespaced message type.
    #[serde(rename = "type")]
    pub message_type: String,
    /// Type-specific payload.
    pub payload: Value,
}

impl Envelope {
    /// Builds an envelope with a fresh `message_id` and the current timestamp.
    pub fn new(message_type: MessageType, payload: Value, correlation_id: Option<Uuid>) -> Self {
        Self {
            protocol_version: crate::protocol::version::PROTOCOL_VERSION.to_string(),
            message_id: Uuid::new_v4(),
            correlation_id,
            sent_at: Utc::now(),
            message_type: message_type.as_str().to_string(),
            payload,
        }
    }

    /// The routing view of [`Self::message_type`], if it is a known type.
    pub fn kind(&self) -> Option<MessageType> {
        self.message_type.parse().ok()
    }
}

/// Every message type in agent protocol v1.0.0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MessageType {
    // handshake
    HandshakeHello,
    HandshakeAuthenticate,
    HandshakeReady,
    HandshakeReject,
    // pairing
    PairingRedeem,
    PairingGranted,
    PairingDenied,
    // ingestion (installation → cloud)
    TelemetryReport,
    HealthReport,
    EventsReport,
    ActivitiesReport,
    CapabilitiesPublish,
    // acknowledgements (cloud → installation)
    TelemetryAck,
    HealthAck,
    EventsAck,
    ActivitiesAck,
    CapabilitiesAck,
    IngestAck,
    // results and state (installation → cloud)
    ConfigResult,
    CommandResult,
    ConfigState,
    // cloud-initiated
    ConfigApply,
    CommandInvoke,
    ConfigPullRequest,
    // connection management
    Ping,
    Pong,
    StreamThrottle,
    SessionRotate,
}

impl MessageType {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::HandshakeHello => "handshake.hello",
            Self::HandshakeAuthenticate => "handshake.authenticate",
            Self::HandshakeReady => "handshake.ready",
            Self::HandshakeReject => "handshake.reject",
            Self::PairingRedeem => "pairing.redeem",
            Self::PairingGranted => "pairing.granted",
            Self::PairingDenied => "pairing.denied",
            Self::TelemetryReport => "telemetry.report",
            Self::HealthReport => "health.report",
            Self::EventsReport => "events.report",
            Self::ActivitiesReport => "activities.report",
            Self::CapabilitiesPublish => "capabilities.publish",
            Self::TelemetryAck => "telemetry.ack",
            Self::HealthAck => "health.ack",
            Self::EventsAck => "events.ack",
            Self::ActivitiesAck => "activities.ack",
            Self::CapabilitiesAck => "capabilities.ack",
            Self::IngestAck => "ingest.ack",
            Self::ConfigResult => "config.result",
            Self::CommandResult => "command.result",
            Self::ConfigState => "config.state",
            Self::ConfigApply => "config.apply",
            Self::CommandInvoke => "command.invoke",
            Self::ConfigPullRequest => "config.pull.request",
            Self::Ping => "ping",
            Self::Pong => "pong",
            Self::StreamThrottle => "stream.throttle",
            Self::SessionRotate => "session.rotate",
        }
    }
}

/// Returned when a message type is not part of protocol v1.0.0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnknownMessageType;

impl std::fmt::Display for UnknownMessageType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("unknown message type")
    }
}

impl std::error::Error for UnknownMessageType {}

impl std::str::FromStr for MessageType {
    type Err = UnknownMessageType;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let ty = match s {
            "handshake.hello" => Self::HandshakeHello,
            "handshake.authenticate" => Self::HandshakeAuthenticate,
            "handshake.ready" => Self::HandshakeReady,
            "handshake.reject" => Self::HandshakeReject,
            "pairing.redeem" => Self::PairingRedeem,
            "pairing.granted" => Self::PairingGranted,
            "pairing.denied" => Self::PairingDenied,
            "telemetry.report" => Self::TelemetryReport,
            "health.report" => Self::HealthReport,
            "events.report" => Self::EventsReport,
            "activities.report" => Self::ActivitiesReport,
            "capabilities.publish" => Self::CapabilitiesPublish,
            "telemetry.ack" => Self::TelemetryAck,
            "health.ack" => Self::HealthAck,
            "events.ack" => Self::EventsAck,
            "activities.ack" => Self::ActivitiesAck,
            "capabilities.ack" => Self::CapabilitiesAck,
            "ingest.ack" => Self::IngestAck,
            "config.result" => Self::ConfigResult,
            "command.result" => Self::CommandResult,
            "config.state" => Self::ConfigState,
            "config.apply" => Self::ConfigApply,
            "command.invoke" => Self::CommandInvoke,
            "config.pull.request" => Self::ConfigPullRequest,
            "ping" => Self::Ping,
            "pong" => Self::Pong,
            "stream.throttle" => Self::StreamThrottle,
            "session.rotate" => Self::SessionRotate,
            _ => return Err(UnknownMessageType),
        };
        Ok(ty)
    }
}

/// Which peer is permitted to send a type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Cloud → installation.
    CloudToInstallation,
    /// Installation → cloud.
    InstallationToCloud,
}

impl MessageType {
    /// The direction this type travels in protocol v1.0.0.
    ///
    /// `ping` and `pong` are bidirectional; they report as cloud-to-installation
    /// and are accepted in both directions by the dispatcher.
    pub fn direction(self) -> Direction {
        match self {
            Self::HandshakeHello
            | Self::HandshakeReady
            | Self::HandshakeReject
            | Self::PairingGranted
            | Self::PairingDenied
            | Self::TelemetryAck
            | Self::HealthAck
            | Self::EventsAck
            | Self::ActivitiesAck
            | Self::CapabilitiesAck
            | Self::IngestAck
            | Self::ConfigApply
            | Self::CommandInvoke
            | Self::ConfigPullRequest
            | Self::Ping
            | Self::Pong
            | Self::StreamThrottle
            | Self::SessionRotate => Direction::CloudToInstallation,
            Self::HandshakeAuthenticate
            | Self::PairingRedeem
            | Self::TelemetryReport
            | Self::HealthReport
            | Self::EventsReport
            | Self::ActivitiesReport
            | Self::CapabilitiesPublish
            | Self::ConfigResult
            | Self::CommandResult
            | Self::ConfigState => Direction::InstallationToCloud,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_type_round_trips_for_all_28_types() {
        let all = [
            MessageType::HandshakeHello,
            MessageType::HandshakeAuthenticate,
            MessageType::HandshakeReady,
            MessageType::HandshakeReject,
            MessageType::PairingRedeem,
            MessageType::PairingGranted,
            MessageType::PairingDenied,
            MessageType::TelemetryReport,
            MessageType::HealthReport,
            MessageType::EventsReport,
            MessageType::ActivitiesReport,
            MessageType::CapabilitiesPublish,
            MessageType::TelemetryAck,
            MessageType::HealthAck,
            MessageType::EventsAck,
            MessageType::ActivitiesAck,
            MessageType::CapabilitiesAck,
            MessageType::IngestAck,
            MessageType::ConfigResult,
            MessageType::CommandResult,
            MessageType::ConfigState,
            MessageType::ConfigApply,
            MessageType::CommandInvoke,
            MessageType::ConfigPullRequest,
            MessageType::Ping,
            MessageType::Pong,
            MessageType::StreamThrottle,
            MessageType::SessionRotate,
        ];
        assert_eq!(all.len(), 28);
        for ty in all {
            assert_eq!(ty.as_str().parse::<MessageType>(), Ok(ty), "{ty:?}");
        }
    }

    #[test]
    fn unknown_message_type_is_rejected_by_routing_but_not_by_decode() {
        assert!("future.thing".parse::<MessageType>().is_err());

        // The envelope still decodes; only routing fails. This is what makes
        // additive evolution safe.
        let wire = serde_json::json!({
            "protocol_version": "1.0.0",
            "message_id": Uuid::new_v4().to_string(),
            "correlation_id": null,
            "sent_at": Utc::now().to_rfc3339(),
            "type": "future.thing",
            "payload": {}
        });
        let decoded: Envelope = serde_json::from_value(wire).expect("decodes");
        assert!(decoded.kind().is_none());
        assert_eq!(decoded.message_type, "future.thing");
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let wire = serde_json::json!({
            "protocol_version": "1.0.0",
            "message_id": Uuid::new_v4().to_string(),
            "sent_at": Utc::now().to_rfc3339(),
            "type": "ping",
            "payload": {},
            "something_new": {"nested": true}
        });
        let decoded: Envelope = serde_json::from_value(wire).expect("additive fields tolerated");
        assert_eq!(decoded.kind(), Some(MessageType::Ping));
    }

    #[test]
    fn absent_correlation_id_is_accepted() {
        let wire = serde_json::json!({
            "protocol_version": "1.0.0",
            "message_id": Uuid::new_v4().to_string(),
            "sent_at": Utc::now().to_rfc3339(),
            "type": "pong",
            "payload": {}
        });
        let decoded: Envelope = serde_json::from_value(wire).expect("decodes");
        assert_eq!(decoded.correlation_id, None);
    }

    #[test]
    fn new_envelope_carries_protocol_version_and_fresh_id() {
        let a = Envelope::new(MessageType::Ping, serde_json::json!({}), None);
        let b = Envelope::new(MessageType::Ping, serde_json::json!({}), None);
        assert_eq!(
            a.protocol_version,
            crate::protocol::version::PROTOCOL_VERSION
        );
        assert_ne!(a.message_id, b.message_id);
    }
}
