//! Transport-independent domain events and the local event bus.
//!
//! The default local transport uses Tokio primitives and does not require
//! NATS. NATS/JetStream is an optional adapter behind the [`EventBus`] trait
//! (ADR-012).

mod bus;
mod error;

pub use bus::{EventBus, LocalEventBus};
pub use error::EventError;

/// Canonical reasoning/plan/execution event types (contracts/events.md).
pub mod types {
    pub const HYPOTHESIS_CREATED: &str = "hypothesis.created";
    pub const PLAN_PROPOSED: &str = "plan.proposed";
    pub const PLAN_APPROVED: &str = "plan.approved";
    pub const PLAN_DENIED: &str = "plan.denied";
    pub const ACTION_EXECUTED: &str = "action.executed";
    pub const ACTION_FAILED: &str = "action.failed";
    pub const VALIDATION_PASSED: &str = "validation.passed";
    pub const VALIDATION_FAILED: &str = "validation.failed";
    pub const PROVIDER_DEGRADED: &str = "provider.degraded";
}
