//! Transport: the only module in this crate that performs I/O.
//!
//! Everything networked sits behind the `Transport` trait so that the cloud
//! client's lifecycle can be exercised against a scripted fake with no sockets
//! (constitution Principle 13).

pub mod backoff;
pub mod fake;
pub mod websocket;

use async_trait::async_trait;

use crate::protocol::Envelope;

/// Why a connection ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    /// The installation closed it deliberately.
    Client,
    /// The cloud revoked this instance. Terminal until re-enrollment.
    Revoked,
    /// The cloud suspended this instance. Recoverable, and never terminal.
    Suspended,
    /// No mutually supported protocol version. Do not hammer.
    VersionMismatch,
    /// The credential was rejected.
    Unauthenticated,
    /// A transport-level failure.
    Error,
    /// The network dropped the connection.
    Network,
}

/// Errors from the transport boundary.
///
/// Variants are deliberately distinguishable so the supervisor can classify a
/// failure: a revoked instance must stop retrying, a rate limit must slow down,
/// and a version mismatch must surface a diagnostic.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("connect failed: {0}")]
    Connect(String),

    #[error("send failed: {0}")]
    Send(String),

    #[error("receive failed: {0}")]
    Receive(String),

    #[error("connection is closed")]
    Closed,

    #[error("frame is not a valid envelope: {0}")]
    Malformed(String),

    #[error("cloud identity mismatch: expected '{expected}', got '{actual}'")]
    CloudIdentityMismatch { expected: String, actual: String },

    #[error("secure transport failure: {0}")]
    SecureTransport(String),

    #[error("i/o failure: {0}")]
    Io(String),
}

/// The agent channel, abstracted so the client is testable without a socket.
#[async_trait]
pub trait Transport: Send + Sync {
    /// Sends one envelope as a single frame.
    async fn send(&self, envelope: &Envelope) -> Result<(), TransportError>;

    /// Receives the next envelope.
    ///
    /// Returns `Ok(None)` when the peer closed the connection cleanly; that is
    /// not an error, it is the end of the stream.
    async fn recv(&mut self) -> Result<Option<Envelope>, TransportError>;

    /// Closes the connection.
    async fn close(&mut self, reason: CloseReason);
}
