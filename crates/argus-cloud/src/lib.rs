//! Argus Cloud integration for ARGUS installations.
//!
//! This crate owns the entire cloud concern — the agent protocol vocabulary,
//! the transport, and the projections between local domain types and the wire
//! format — so that cloud concepts cannot leak into [`argus_domain`]
//! (constitution Principle 6) and the integration can be removed without
//! touching the core (Principle 7).
//!
//! Layering:
//!
//! - [`protocol`] is pure: envelope, message payloads, error codes, and version
//!   negotiation. No I/O.
//! - [`mapping`] is pure: projections between local domain types and the wire
//!   vocabulary, including the vocabulary translations fixed by ADR-0016.
//! - [`transport`] is the **only** module that performs I/O, and it does so
//!   behind a trait so the client is testable without a network
//!   (Principle 13).
//! - [`client`] is the connection state machine built on [`transport`].
//!
//! The client never dispatches inbound control messages. Inbound
//! `config.apply`, `command.invoke`, and `session.rotate` handling belongs to
//! `argus-daemon`, which holds the policy and executor boundary. A cloud
//! message is a request, never an authorization grant (Principle 2).

pub mod buffer;
pub mod client;
pub mod config;
pub mod error;
pub mod mapping;
pub mod protocol;
pub mod state;
pub mod transport;
