//! ARGUS privileged daemon (`argusd`).
//!
//! The trusted control boundary. Owns authorization enforcement, auditing, and
//! execution controls; the AI layer must not receive unrestricted root access.

pub mod cloud;
pub mod config;
pub mod control;
pub mod handler;
pub mod runtime;

pub use cloud::{
    CloudSecretStore, EnrollmentError, EnrollmentReport, EnrollmentRequest, SecretError,
    SessionCredential,
};
pub use config::DaemonConfig;
pub use runtime::{Daemon, DispatchError};
