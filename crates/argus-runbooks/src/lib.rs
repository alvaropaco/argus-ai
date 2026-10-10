//! ARGUS declarative runbooks with gated promotion (spec 003 M6, CAP-17,
//! FR-019, ADR-0036).
//!
//! A [`Runbook`] is a declarative operational procedure: trigger, required
//! evidence, investigation steps, decision criteria, allowed actions,
//! rollback, and validation. Its `allowed_actions` are **candidate** typed
//! capabilities — never grants: selecting a runbook yields a candidate plan
//! that still crosses policy and the typed executor exactly like any other
//! plan (ADR-0036 §2).
//!
//! Learned procedures are gated: a candidate passes
//! evaluation → simulation → validation → policy → approval → promotion
//! before use, one gate at a time, in that order (ADR-0036 §3). Nothing in
//! this crate self-promotes.

mod criterion;
mod delivery;
mod library;
mod loader;
mod promotion;
mod runbook;

pub use criterion::{Comparison, Criterion, Reading};
pub use delivery::DeliveredRunbook;
pub use library::RunbookLibrary;
pub use loader::{LoadResult, RunbookFile, load_runbooks, parse_runbook};
pub use promotion::{Gate, GateError, PromotionError};
pub use runbook::{EvidenceKind, Runbook, RunbookStatus, RunbookTrigger, Step};
