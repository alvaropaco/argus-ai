//! ARGUS observation coordinator.
//!
//! This crate owns the live process inventory, its lifecycle diffing, and
//! process-level anomaly detection (CAP-2). It meets `argus-sensors` (the
//! kernel readers) and will meet `argus-domain` (Observation) and
//! `argus-events` (publishing) as the coordinator is wired into the daemon
//! (ADR-0032).
//!
//! The core logic — inventory diffing and anomaly detection — is pure and
//! deterministic, testable without a host or a live model (Constitution P9/P13).

mod anomaly;
mod convert;
mod inventory;
mod observer;
mod snapshot;

pub use anomaly::{
    AnomalyConfig, ProcessAnomaly, ProcessAnomalyKind, detect_lifecycle_anomalies, detect_runaway,
};
pub use convert::ObservationEmitter;
pub use inventory::{Field, LifecycleChange, ProcessInventory, ProcessRecord, diff};
pub use observer::{Observer, TickOutput};
pub use snapshot::ProcSnapshotter;

/// Coordinator error. A failed process listing is the only fallible step here;
/// per-process reads degrade to `None` rather than erroring (ADR-0032 §4).
#[derive(Debug, thiserror::Error)]
pub enum ObserveError {
    #[error("failed to list processes: {0}")]
    ListPids(std::io::Error),
}
