//! Cloud client state machine: handshake, pairing, heartbeat, and supervision.
//!
//! Inbound control-message *dispatch* is deliberately absent here; it belongs to
//! `argus-daemon`, which holds the policy and executor boundary.

pub mod config;
pub mod handshake;
pub mod heartbeat;
pub mod identity;
pub mod pairing;
pub mod reporting;
pub mod supervisor;

pub use config::{DeliveryDecision, classify_delivery};
pub use handshake::{AuthenticatedSession, HandshakeFailure, InstallationIdentity, authenticate};
pub use heartbeat::Heartbeat;
pub use identity::{IdentityError, InstallationKey};
pub use pairing::{EnrollmentOutcome, enroll, validate_code};
pub use reporting::{DEFAULT_MAX_SKEW, SkewVerdict, assess, assess_skew};
pub use supervisor::{
    ConnectionOutcome, NextAction, ReconnectPolicy, SessionRequest, StopReason, TransportFactory,
    WssFactory, connect_once,
};
