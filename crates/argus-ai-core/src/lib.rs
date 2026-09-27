//! ARGUS core runtime: lifecycle, coordination, and control-loop orchestration.

pub mod decision;
pub mod model;
pub mod view;

pub use view::{NOTHING_TO_EXPLAIN, explain_decision, explain_plan, summarize_evidence};
