//! ARGUS container adapter.
//!
//! Observes container state, restart counts, and lifecycle over the Docker
//! socket (its `/containers/json` endpoint). The typed model, JSON parsing, and
//! HTTP-response parsing are deterministic and testable anywhere; the socket
//! I/O itself is Linux/Docker-gated (Constitution P9/P13, ADR-0032).
//!
//! This complements the kernel-native container association already available
//! from a process's `/proc/<pid>/cgroup` membership (the cgroup-v2 hierarchy in
//! `argus-sensors`); the socket supplies the runtime-level restart count and
//! image/state that cgroups do not expose.

mod client;
mod convert;
mod model;

pub use client::DockerClient;
pub use convert::container_observations;
pub use model::{parse_containers, Container};

/// Adapter error. `Unavailable` means the Docker socket is not reachable, which
/// the daemon treats as graceful degradation rather than a failure.
#[derive(Debug, thiserror::Error)]
pub enum ContainerError {
    #[error("docker is unavailable: {0}")]
    Unavailable(String),
    #[error("docker error: {0}")]
    Other(String),
    #[error("failed to parse docker response: {0}")]
    Parse(String),
}
