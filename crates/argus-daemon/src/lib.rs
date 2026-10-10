//! ARGUS privileged daemon (`argusd`).
//!
//! The trusted control boundary. Owns authorization enforcement, auditing, and
//! execution controls; the AI layer must not receive unrestricted root access.

pub mod autonomy;
pub mod brain;
pub mod brain_state;
pub mod cloud;
pub mod config;
pub mod control;
pub mod handler;
pub mod ledger;
pub mod observe;
pub mod privileged;
pub mod runbooks;
pub mod runtime;
pub mod sentinel;

pub use cloud::{
    CloudSecretStore, EnrollmentError, EnrollmentReport, EnrollmentRequest, SecretError,
    SessionCredential,
};
pub use config::DaemonConfig;
pub use observe::{BaselineManager, ObservationLoop};
pub use privileged::{PrivilegedError, PrivilegedLimiter};
pub use runtime::{Daemon, DiagnoseContext, DispatchError, InMemoryDedup, diagnose_with};
