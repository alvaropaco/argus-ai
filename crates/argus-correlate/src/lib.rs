//! ARGUS correlation: the operational environment graph, event-to-situation
//! correlation (CAP-5, CAP-6, ADR-0033), and change intelligence (CAP-18).
//!
//! The graph is a projection over the canonical observation/event store — it is
//! rebuildable from immutable observations and holds no inference. The
//! correlator folds related events into situations by shared correlation key
//! within a time window, and the change ledger answers "what changed before
//! the incident" with an evidence-backed hypothesis. Everything is
//! deterministic and testable.

mod change;
mod correlate;
mod graph;

pub use change::{ChangeHypothesis, ChangeLedger, RankedChange};
pub use correlate::{CorrelatedEvent, Correlator, Situation};
pub use graph::{Edge, Graph};
