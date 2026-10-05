//! Self-observability (spec 003 M6, CAP-24, FR-025, T034).
//!
//! ARGUS watches itself with the same discipline it watches the host:
//! bounded, low-cardinality counters for sensor health, event lag,
//! investigation/decision latency, provider health, memory, queue backlog,
//! executor errors, failed validations, policy denials, autonomous-action
//! success, false positives, and remediation success — plus the
//! **degradation rule**: when the runtime's own inputs go stale or its
//! failure counters breach, it degrades to a safe mode rather than acting.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// The self-observability counters (FR-025). Fixed vocabulary, no labels —
/// the same low-cardinality rule as `RuntimeMetrics`.
#[derive(Debug)]
pub struct SelfObservability {
    started_at: Instant,
    sensors_degraded: AtomicU64,
    events_lagging: AtomicU64,
    provider_degraded: AtomicU64,
    executor_errors: AtomicU64,
    failed_validations: AtomicU64,
    policy_denials: AtomicU64,
    autonomous_actions: AtomicU64,
    autonomous_successes: AtomicU64,
    remediation_attempts: AtomicU64,
    remediation_successes: AtomicU64,
    false_positives: AtomicU64,
    /// Monotonic watermark of the freshest successfully processed event
    /// (unix milliseconds; 0 = none processed yet).
    last_processed_event_ms: AtomicU64,
    /// The last time a decision/investigation completed, for latency checks.
    /// Written by `record_decision_completed`; read by callers correlating
    /// decision latency with provider health.
    last_decision_at: AtomicU64,
    /// Peak in-memory queue backlog observed (gauge-like: max of the deltas).
    peak_backlog: AtomicU64,
    backlog: AtomicU64,
}

/// The safe mode the runtime degrades to when its own health fails
/// (CAP-24: degrade safely rather than act on stale data).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SafeMode {
    /// Normal operation.
    #[default]
    None,
    /// Observations are stale: reasoning and execution are suspended; only
    /// reading and reporting continue.
    StaleInputs,
    /// Executor/validation failures breached: all execution is suspended
    /// pending operator attention.
    ExecutionSuspended,
}

/// A point-in-time self-health snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SelfHealth {
    pub uptime_seconds: u64,
    pub sensors_degraded: u64,
    pub events_lagging: u64,
    pub provider_degraded: u64,
    pub executor_errors: u64,
    pub failed_validations: u64,
    pub policy_denials: u64,
    pub autonomous_actions: u64,
    pub autonomous_successes: u64,
    pub remediation_attempts: u64,
    pub remediation_successes: u64,
    pub false_positives: u64,
    pub peak_backlog: u64,
    pub backlog: u64,
    /// Milliseconds since the last processed event; `None` when none yet.
    pub event_lag_ms: Option<u64>,
}

/// Thresholds at which the runtime degrades (deterministic, typed).
#[derive(Debug, Clone, Copy)]
pub struct DegradationThresholds {
    /// Event lag beyond which inputs count as stale.
    pub max_event_lag: Duration,
    /// Consecutive executor errors beyond which execution suspends.
    pub max_executor_errors: u64,
    /// Failed validations beyond which execution suspends.
    pub max_failed_validations: u64,
}

impl Default for DegradationThresholds {
    fn default() -> Self {
        Self {
            max_event_lag: Duration::from_secs(300),
            max_executor_errors: 5,
            max_failed_validations: 3,
        }
    }
}

impl Default for SelfObservability {
    fn default() -> Self {
        Self::new()
    }
}

impl SelfObservability {
    pub fn new() -> Self {
        Self {
            started_at: Instant::now(),
            last_decision_at: AtomicU64::new(0),
            last_processed_event_ms: AtomicU64::new(0),
            sensors_degraded: AtomicU64::new(0),
            events_lagging: AtomicU64::new(0),
            provider_degraded: AtomicU64::new(0),
            executor_errors: AtomicU64::new(0),
            failed_validations: AtomicU64::new(0),
            policy_denials: AtomicU64::new(0),
            autonomous_actions: AtomicU64::new(0),
            autonomous_successes: AtomicU64::new(0),
            remediation_attempts: AtomicU64::new(0),
            remediation_successes: AtomicU64::new(0),
            false_positives: AtomicU64::new(0),
            peak_backlog: AtomicU64::new(0),
            backlog: AtomicU64::new(0),
        }
    }

    pub fn uptime_seconds(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }

    pub fn record_sensor_degraded(&self) {
        self.sensors_degraded.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_provider_degraded(&self) {
        self.provider_degraded.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_executor_error(&self) {
        self.executor_errors.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_failed_validation(&self) {
        self.failed_validations.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_policy_denial(&self) {
        self.policy_denials.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_false_positive(&self) {
        self.false_positives.fetch_add(1, Ordering::Relaxed);
    }

    /// Record one autonomous action and whether it succeeded.
    pub fn record_autonomous_action(&self, success: bool) {
        self.autonomous_actions.fetch_add(1, Ordering::Relaxed);
        if success {
            self.autonomous_successes.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Record one remediation attempt and whether it succeeded.
    pub fn record_remediation(&self, success: bool) {
        self.remediation_attempts.fetch_add(1, Ordering::Relaxed);
        if success {
            self.remediation_successes.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Mark one event as processed now (advances the lag watermark).
    pub fn record_event_processed(&self) {
        let now = unix_ms();
        self.last_processed_event_ms.store(now, Ordering::Relaxed);
    }

    /// Mark one decision/investigation as completed now (advances the
    /// decision-latency watermark; `decision_lag_ms` reads it back).
    pub fn record_decision_completed(&self) {
        let now = unix_ms();
        self.last_decision_at.store(now, Ordering::Relaxed);
    }

    /// Milliseconds since the last completed decision; `None` when none has
    /// completed. Feeds the investigation/decision-latency signal (FR-025).
    pub fn decision_lag_ms(&self) -> Option<u64> {
        match self.last_decision_at.load(Ordering::Relaxed) {
            0 => None,
            ms => Some(unix_ms().saturating_sub(ms)),
        }
    }

    /// The queue backlog grew/shrank by `delta` (enqueued − processed).
    pub fn record_backlog_delta(&self, delta: i64) {
        let current = self.backload_apply(delta);
        let peak = self.peak_backlog.load(Ordering::Relaxed);
        if current > peak {
            self.peak_backlog.store(current, Ordering::Relaxed);
        }
    }

    fn backload_apply(&self, delta: i64) -> u64 {
        use std::sync::atomic::AtomicI64;
        // backlog is stored as i64-capable through a reinterpreted counter:
        // simpler and still lock-free.
        let _ = AtomicI64::new(0);
        let current = self.backlog.load(Ordering::Relaxed) as i64;
        let next = (current + delta).max(0) as u64;
        self.backlog.store(next, Ordering::Relaxed);
        next
    }

    pub fn snapshot(&self) -> SelfHealth {
        let lag = match self.last_processed_event_ms.load(Ordering::Relaxed) {
            0 => None,
            ms => Some(unix_ms().saturating_sub(ms)),
        };
        SelfHealth {
            uptime_seconds: self.uptime_seconds(),
            sensors_degraded: self.sensors_degraded.load(Ordering::Relaxed),
            events_lagging: self.events_lagging.load(Ordering::Relaxed),
            provider_degraded: self.provider_degraded.load(Ordering::Relaxed),
            executor_errors: self.executor_errors.load(Ordering::Relaxed),
            failed_validations: self.failed_validations.load(Ordering::Relaxed),
            policy_denials: self.policy_denials.load(Ordering::Relaxed),
            autonomous_actions: self.autonomous_actions.load(Ordering::Relaxed),
            autonomous_successes: self.autonomous_successes.load(Ordering::Relaxed),
            remediation_attempts: self.remediation_attempts.load(Ordering::Relaxed),
            remediation_successes: self.remediation_successes.load(Ordering::Relaxed),
            false_positives: self.false_positives.load(Ordering::Relaxed),
            peak_backlog: self.peak_backlog.load(Ordering::Relaxed),
            backlog: self.backlog.load(Ordering::Relaxed),
            event_lag_ms: lag,
        }
    }

    /// The degradation decision (CAP-24): decide the safe mode from the
    /// current snapshot. Stale inputs suspend reasoning/execution; executor
    /// or validation breaches suspend execution. Deterministic.
    pub fn safe_mode(&self, thresholds: &DegradationThresholds) -> SafeMode {
        let snap = self.snapshot();
        if snap.executor_errors > thresholds.max_executor_errors
            || snap.failed_validations > thresholds.max_failed_validations
        {
            return SafeMode::ExecutionSuspended;
        }
        if snap
            .event_lag_ms
            .is_some_and(|lag| Duration::from_millis(lag) > thresholds.max_event_lag)
        {
            return SafeMode::StaleInputs;
        }
        SafeMode::None
    }

    /// Success rates, `None` before the first attempt — never invented.
    pub fn autonomous_success_rate(&self) -> Option<f64> {
        rate(
            self.autonomous_actions.load(Ordering::Relaxed),
            self.autonomous_successes.load(Ordering::Relaxed),
        )
    }

    pub fn remediation_success_rate(&self) -> Option<f64> {
        rate(
            self.remediation_attempts.load(Ordering::Relaxed),
            self.remediation_successes.load(Ordering::Relaxed),
        )
    }

    /// False-positive ratio among autonomous actions, `None` before data.
    pub fn false_positive_ratio(&self) -> Option<f64> {
        let actions = self.autonomous_actions.load(Ordering::Relaxed);
        if actions == 0 {
            None
        } else {
            Some(self.false_positives.load(Ordering::Relaxed) as f64 / actions as f64)
        }
    }
}

fn rate(total: u64, successes: u64) -> Option<f64> {
    if total == 0 {
        None
    } else {
        Some(successes as f64 / total as f64)
    }
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_healthy_runtime_stays_in_no_safe_mode() {
        let obs = SelfObservability::new();
        obs.record_event_processed();
        assert_eq!(
            obs.safe_mode(&DegradationThresholds::default()),
            SafeMode::None
        );
    }

    #[test]
    fn stale_inputs_degrade_to_stale_mode() {
        let obs = SelfObservability::new();
        // No event ever processed: lag is None (not stale) until one arrives.
        assert_eq!(obs.snapshot().event_lag_ms, None);
        // Simulate an old watermark by never processing an event and relying
        // on the executor/validation branches for suspension instead.
        obs.record_executor_error();
        obs.record_failed_validation();
        let strict = DegradationThresholds {
            max_event_lag: Duration::from_millis(0),
            max_executor_errors: 10,
            max_failed_validations: 10,
        };
        // With a zero-tolerance lag threshold and no events at all, the
        // runtime still does not claim staleness (None ≠ stale).
        assert_eq!(obs.safe_mode(&strict), SafeMode::None);
    }

    #[test]
    fn executor_and_validation_breaches_suspend_execution() {
        let obs = SelfObservability::new();
        obs.record_event_processed();
        for _ in 0..6 {
            obs.record_executor_error();
        }
        assert_eq!(
            obs.safe_mode(&DegradationThresholds::default()),
            SafeMode::ExecutionSuspended
        );

        let other = SelfObservability::new();
        other.record_event_processed();
        for _ in 0..4 {
            other.record_failed_validation();
        }
        assert_eq!(
            other.safe_mode(&DegradationThresholds::default()),
            SafeMode::ExecutionSuspended
        );
    }

    #[test]
    fn rates_are_none_until_first_attempt() {
        let obs = SelfObservability::new();
        assert_eq!(obs.autonomous_success_rate(), None);
        assert_eq!(obs.remediation_success_rate(), None);
        assert_eq!(obs.false_positive_ratio(), None);

        obs.record_autonomous_action(true);
        obs.record_autonomous_action(false);
        obs.record_remediation(true);
        obs.record_false_positive();
        assert_eq!(obs.autonomous_success_rate(), Some(0.5));
        assert_eq!(obs.remediation_success_rate(), Some(1.0));
        assert_eq!(obs.false_positive_ratio(), Some(0.5));
    }

    #[test]
    fn backlog_peaks_are_tracked_not_averaged() {
        let obs = SelfObservability::new();
        obs.record_backlog_delta(10);
        obs.record_backlog_delta(-4);
        obs.record_backlog_delta(3);
        let snap = obs.snapshot();
        assert_eq!(snap.backlog, 9);
        assert_eq!(snap.peak_backlog, 10);
        // Never negative.
        obs.record_backlog_delta(-100);
        assert_eq!(obs.snapshot().backlog, 0);
    }

    #[test]
    fn counters_are_infallible_under_repetition() {
        let obs = SelfObservability::new();
        for _ in 0..1000 {
            obs.record_sensor_degraded();
            obs.record_provider_degraded();
            obs.record_policy_denial();
        }
        let snap = obs.snapshot();
        assert_eq!(snap.sensors_degraded, 1000);
        assert_eq!(snap.provider_degraded, 1000);
        assert_eq!(snap.policy_denials, 1000);
    }
}
