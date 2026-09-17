//! ARGUS privileged daemon (`argusd`).
//!
//! The trusted control boundary. Owns authorization enforcement, auditing, and
//! execution controls; the AI layer must not receive unrestricted root access.

pub mod config;
pub mod handler;
pub mod runtime;

pub use config::DaemonConfig;
pub use runtime::{Daemon, DispatchError};
