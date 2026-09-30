//! ARGUS correlation: the operational environment graph and event-to-situation
//! correlation (CAP-5, CAP-6, ADR-0033).
//!
//! The graph is a projection over the canonical observation/event store — it is
//! rebuildable from immutable observations and holds no inference. The
//! correlator folds related events into situations by shared correlation key
//! within a time window. Everything is deterministic and testable.

mod correlate;
mod graph;

pub use correlate::{CorrelatedEvent, Correlator, Situation};
pub use graph::{Edge, Graph};
