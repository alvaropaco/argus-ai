//! Wire protocol: envelope, message payloads, error codes, version negotiation.
//!
//! Pure and free of I/O. The cloud is already implemented and authoritative, so
//! these types mirror the cloud's published schemas rather than defining new
//! ones. See `specs/001-argus-cloud-sync/contracts/agent-protocol-conformance.md`.

pub mod envelope;
pub mod errors;
pub mod messages;
pub mod version;

pub use envelope::{Direction, Envelope, MessageType, UnknownMessageType};
pub use errors::{PairingDenialCode, ProtocolError, ProtocolErrorCode};
pub use version::{PROTOCOL_VERSION, SUPPORTED_PROTOCOL_VERSIONS, negotiate};
