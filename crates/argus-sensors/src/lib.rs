//! ARGUS kernel-native sensors.
//!
//! Each sensor reads one authoritative Linux interface and returns typed data.
//! Sensors are pure readers: they produce data, never decisions, and they never
//! execute a privileged operation (ADR-0032, constitution Principle 5).
//!
//! Sensors are testable without a live host: every parser is a pure function
//! over `&str`, so unit tests feed `/proc`-style fixtures (Principle 13).

mod cgroup;
mod proc;
mod sys;

pub use cgroup::{CgroupSnapshot, CgroupV2Reader};
pub use proc::{diskstats, loadavg, meminfo, netdev, pressure, process, stat, vmstat};
pub use sys::{disk, net, thermal};

/// Error for a sensor read. A sensor that cannot run reports `Unavailable` and
/// contributes nothing (graceful degradation) rather than failing the loop.
#[derive(Debug, thiserror::Error)]
pub enum SensorError {
    #[error("sensor `{name}` is unavailable: {reason}")]
    Unavailable { name: &'static str, reason: String },
    #[error("failed to read `{name}`: {source}")]
    Read {
        name: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse `{name}`: {message}")]
    Parse { name: &'static str, message: String },
}

/// A kernel-native sensor: one authoritative interface, one typed output.
///
/// Implementations are stateless unit structs. `read` is synchronous and
/// side-effect-free. `available` reports whether the interface is present, so
/// the coordinator (`argus-observe`) can degrade per sensor (ADR-0032 §4).
pub trait Sensor {
    /// The typed output of this sensor.
    type Output;

    /// Stable identifier for logs and health reporting.
    fn name(&self) -> &'static str;

    /// One-shot read of the interface.
    fn read(&self) -> Result<Self::Output, SensorError>;

    /// Whether the underlying interface is present and readable.
    fn available(&self) -> bool {
        true
    }
}

/// Read a file to a string, mapping an IO error onto a sensor `Read` error.
pub(crate) fn read_file(name: &'static str, path: &str) -> Result<String, SensorError> {
    std::fs::read_to_string(path).map_err(|source| SensorError::Read { name, source })
}
