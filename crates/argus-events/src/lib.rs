//! Transport-independent domain events and the local event bus.
//!
//! The default local transport uses Tokio primitives and does not require
//! NATS. NATS/JetStream is an optional adapter behind the [`EventBus`] trait
//! (ADR-012).

mod bus;
mod error;
mod ledger;

pub use bus::{EventBus, LocalEventBus};
pub use error::EventError;
pub use ledger::{LedgerSink, NoopLedgerSink};

/// Canonical reasoning/plan/execution event types (contracts/events.md).
pub mod types {
    pub const HYPOTHESIS_CREATED: &str = "hypothesis.created";
    pub const PLAN_PROPOSED: &str = "plan.proposed";
    pub const PLAN_APPROVED: &str = "plan.approved";
    pub const PLAN_DENIED: &str = "plan.denied";
    pub const PLAN_FAILED: &str = "plan.failed";
    pub const PLAN_ROLLING_BACK: &str = "plan.rolling.back";
    pub const PLAN_ROLLED_BACK: &str = "plan.rolled.back";
    pub const PLAN_NEEDS_MANUAL: &str = "plan.needs.manual";
    pub const ACTION_EXECUTED: &str = "action.executed";
    pub const ACTION_FAILED: &str = "action.failed";
    pub const ACTION_ROLLED_BACK: &str = "action.rolled.back";
    pub const ACTION_ROLLBACK_FAILED: &str = "action.rollback.failed";
    pub const ACTION_ALREADY_DESIRED: &str = "action.already.desired";
    pub const VALIDATION_PASSED: &str = "validation.passed";
    pub const VALIDATION_FAILED: &str = "validation.failed";
    pub const PROVIDER_DEGRADED: &str = "provider.degraded";

    // Nervous system (spec 007): the brain-cycle trace and the token-usage
    // metering records are new event vocabulary; the action side reuses the
    // `action.*` constants above rather than inventing parallel types.
    pub const BRAIN_TRACE: &str = "brain.trace";
    pub const TOKEN_USAGE: &str = "token.usage";

    // Graduated autonomy (spec 008): promotions and demotions of the earned
    // rung are first-class domain events; the ladder itself is local state
    // (ADR-0040), so nothing cloud-side consumes these beyond the ledger.
    pub const AUTONOMY_PROMOTED: &str = "autonomy.promoted";
    pub const AUTONOMY_DEMOTED: &str = "autonomy.demoted";

    // Observation / sensor (ADR-0032)
    pub const SENSOR_HEALTHY: &str = "sensor.healthy";
    pub const SENSOR_DEGRADED: &str = "sensor.degraded";
    pub const OBSERVATION_COLLECTED: &str = "observation.collected";

    // Baseline (CAP-4)
    pub const BASELINE_ESTABLISHED: &str = "baseline.established";
    pub const BASELINE_DEVIATION: &str = "baseline.deviation";

    // Process lifecycle + anomaly (CAP-2)
    pub const PROCESS_STARTED: &str = "process.started";
    pub const PROCESS_EXITED: &str = "process.exited";
    pub const PROCESS_CHANGED: &str = "process.changed";
    pub const PROCESS_ANOMALY: &str = "process.anomaly";

    // Restart loop (CAP-3)
    pub const UNIT_RESTART_LOOP: &str = "unit.restart.loop";

    // Prediction (CAP-11)
    pub const PREDICTION_ISSUED: &str = "prediction.issued";

    // Change intelligence (CAP-18)
    pub const CHANGE_DETECTED: &str = "change.detected";
}
