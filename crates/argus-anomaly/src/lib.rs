//! ARGUS anomaly detection.
//!
//! This crate owns the deterministic "what is normal" layer (CAP-4): rolling
//! baselines, per-signal deviation detection (static rules + historical
//! baseline excursions), restart-loop detection over generic managed units
//! (systemd services and containers share the same shape — domain-model §3.10),
//! and least-squares trend fits feeding the labeled predictions (CAP-11).
//!
//! Multi-signal correlation is `argus-correlate` (Milestone 2). Everything here
//! is pure and testable without a host or a live model (Constitution P9/P13).

mod baseline;
mod deviation;
mod multisignal;
mod restart;
mod trend;

pub use baseline::Baseline;
pub use deviation::{Deviation, DeviationConfig, DeviationKind, detect_deviation};
pub use multisignal::{MultiSignalAnomaly, MultiSignalConfig, detect_multi_signal};
pub use restart::{RestartLoop, UnitState, detect_restart_loops};
pub use trend::{Trend, TrendSample};
