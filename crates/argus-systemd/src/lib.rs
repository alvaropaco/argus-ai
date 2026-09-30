//! ARGUS systemd adapter.
//!
//! Observes systemd unit state — name, load/active/sub state — over the system
//! D-Bus (zbus), and exposes failure and restart-loop signals. The live client
//! is Linux-gated; the typed model and its predicates are deterministic and
//! testable anywhere (Constitution P9/P13, ADR-0032).

mod client;
mod convert;
mod model;

pub use client::SystemdClient;
pub use convert::unit_observations;
pub use model::SystemdUnit;

/// Adapter error. `Unavailable` means systemd/D-Bus is not reachable, which the
/// daemon treats as graceful degradation rather than a failure.
#[derive(Debug, thiserror::Error)]
pub enum SystemdError {
    #[error("systemd is unavailable: {0}")]
    Unavailable(String),
    #[error("systemd error: {0}")]
    Other(String),
}
