//! ARGUS anomaly detection.
//!
//! This crate owns the deterministic "what is normal" layer (CAP-4): rolling
//! baselines, per-signal deviation detection (static rules + historical
//! baseline excursions), and restart-loop detection over generic managed units
//! (systemd services and containers share the same shape — domain-model §3.10).
//!
//! Multi-signal correlation is `argus-correlate` (Milestone 2). Everything here
//! is pure and testable without a host or a live model (Constitution P9/P13).

mod baseline;
mod deviation;
mod multisignal;
mod restart;

pub use baseline::Baseline;
pub use deviation::{detect_deviation, Deviation, DeviationConfig, DeviationKind};
pub use multisignal::{detect_multi_signal, MultiSignalAnomaly, MultiSignalConfig};
pub use restart::{detect_restart_loops, RestartLoop, UnitState};
