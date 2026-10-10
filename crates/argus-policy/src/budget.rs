//! The blast-radius budget evaluator (spec 008 FR-004, ADR-0040 §2).
//!
//! Pure and deterministic: anchored UTC hour/day buckets keyed by risk class
//! and scope — no network, no clock reads (callers inject `now`), which is
//! what makes the windows restart-safe. A window whose anchor no longer
//! matches the current hour/day has rolled over and counts as empty.
//!
//! Exhaustion is a *pause* decision for the caller: the plan loop parks the
//! plan on the ordinary approval machinery (`budget.exhausted`) — never a
//! denial of service, never an execution. A unit is reserved atomically
//! ([`reserve`]: rollover + room-check + consumption in one call) and
//! surrendered by [`refund`] when the step turned out not to execute — an
//! already-desired no-op, or a pause after the reserve. A failed-then-rolled-back
//! step keeps its consumption: the blast radius was touched.

use argus_domain::{BlastRadius, BudgetWindows, RiskClass};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The configured budget limits — the `[autonomy] budgets` shape (spec 008
/// FR-006). Deserialization clamps every key to ≥1: a budget of zero would be
/// a denial of service, which exhaustion must never be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BudgetLimits {
    /// Low-risk auto-executions per UTC hour.
    pub low_risk_per_hour: u32,
    /// Controlled-risk auto-executions per UTC day.
    pub controlled_per_day: u32,
    /// Host-scope auto-executions per UTC day, any risk class.
    pub host_scope_per_day: u32,
}

impl Default for BudgetLimits {
    fn default() -> Self {
        Self {
            low_risk_per_hour: 20,
            controlled_per_day: 5,
            host_scope_per_day: 3,
        }
    }
}

impl<'de> Deserialize<'de> for BudgetLimits {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Wire {
            #[serde(default)]
            low_risk_per_hour: Option<u32>,
            #[serde(default)]
            controlled_per_day: Option<u32>,
            #[serde(default)]
            host_scope_per_day: Option<u32>,
        }
        let wire = Wire::deserialize(deserializer)?;
        // Clamped ≥1: a zero budget pauses everything, which is a denial.
        Ok(Self {
            low_risk_per_hour: wire.low_risk_per_hour.unwrap_or(20).max(1),
            controlled_per_day: wire.controlled_per_day.unwrap_or(5).max(1),
            host_scope_per_day: wire.host_scope_per_day.unwrap_or(3).max(1),
        })
    }
}

/// Hours since the UTC epoch — the hourly window anchor (`div_euclid` keeps
/// pre-epoch timestamps well-formed).
pub fn hour_index(now: DateTime<Utc>) -> i64 {
    now.timestamp().div_euclid(3_600)
}

/// Days since the UTC epoch — the daily window anchor.
pub fn day_index(now: DateTime<Utc>) -> i64 {
    now.timestamp().div_euclid(86_400)
}

/// Applies the window rollover and returns the consumed count: a window whose
/// anchor does not match the current index restarts at zero.
fn rolled(window: Option<(i64, u32)>, anchor: i64) -> u32 {
    match window {
        Some((window_anchor, used)) if window_anchor == anchor => used,
        _ => 0,
    }
}

/// Whether a step of `risk` at `scope` may auto-execute now under `limits`,
/// consuming its unit in the same breath: rollover, room-check, and
/// consumption happen in one call, so a caller that serializes reserves on
/// its state lock can never overshoot the cap with concurrent plans.
///
/// Every applicable window must have room — a step at host scope consumes
/// both its risk window and the host-scope window. `false` means exhausted:
/// the rollover was still applied, but nothing was consumed.
pub fn reserve(
    windows: &mut BudgetWindows,
    limits: &BudgetLimits,
    risk: RiskClass,
    scope: BlastRadius,
    now: DateTime<Utc>,
) -> bool {
    let hour = hour_index(now);
    let day = day_index(now);

    if risk == RiskClass::LowRisk {
        let used = rolled(windows.low_risk_hour, hour);
        windows.low_risk_hour = Some((hour, used));
        if used >= limits.low_risk_per_hour {
            return false;
        }
    }
    if risk == RiskClass::Controlled {
        let used = rolled(windows.controlled_day, day);
        windows.controlled_day = Some((day, used));
        if used >= limits.controlled_per_day {
            return false;
        }
    }
    if scope == BlastRadius::Host {
        let used = rolled(windows.host_scope_day, day);
        windows.host_scope_day = Some((day, used));
        if used >= limits.host_scope_per_day {
            return false;
        }
    }
    // Room in every applicable window: consume one unit in each.
    bump(&mut windows.low_risk_hour, hour, risk == RiskClass::LowRisk);
    bump(
        &mut windows.controlled_day,
        day,
        risk == RiskClass::Controlled,
    );
    bump(&mut windows.host_scope_day, day, scope == BlastRadius::Host);
    true
}

/// Consumes one unit in `window` when the step's shape applies to it.
fn bump(window: &mut Option<(i64, u32)>, anchor: i64, applies: bool) {
    if applies {
        let used = rolled(*window, anchor);
        *window = Some((anchor, used + 1));
    }
}

/// Surrenders one unit in every window the step's risk and scope apply to
/// (floored at zero). Called when a reserved step turned out not to
/// execute: an already-desired no-op took no effect, or the plan paused
/// after the reserve. A failed-then-rolled-back step is *not* refunded —
/// the blast radius was touched.
pub fn refund(
    windows: &mut BudgetWindows,
    risk: RiskClass,
    scope: BlastRadius,
    now: DateTime<Utc>,
) {
    let hour = hour_index(now);
    let day = day_index(now);
    if risk == RiskClass::LowRisk {
        let used = rolled(windows.low_risk_hour, hour);
        windows.low_risk_hour = Some((hour, used.saturating_sub(1)));
    }
    if risk == RiskClass::Controlled {
        let used = rolled(windows.controlled_day, day);
        windows.controlled_day = Some((day, used.saturating_sub(1)));
    }
    if scope == BlastRadius::Host {
        let used = rolled(windows.host_scope_day, day);
        windows.host_scope_day = Some((day, used.saturating_sub(1)));
    }
}

/// The remaining budget per window, keyed by the config vocabulary
/// (`low_risk_per_hour`, `controlled_per_day`, `host_scope_per_day`) — the
/// shape the sentinel view reports (spec 008 FR-005).
pub fn remaining(
    windows: &BudgetWindows,
    limits: &BudgetLimits,
    now: DateTime<Utc>,
) -> Vec<(&'static str, u32, u32)> {
    let hour = hour_index(now);
    let day = day_index(now);
    let low = rolled(windows.low_risk_hour, hour);
    let controlled = rolled(windows.controlled_day, day);
    let host = rolled(windows.host_scope_day, day);
    vec![
        (
            "low_risk_per_hour",
            limits.low_risk_per_hour.saturating_sub(low),
            limits.low_risk_per_hour,
        ),
        (
            "controlled_per_day",
            limits.controlled_per_day.saturating_sub(controlled),
            limits.controlled_per_day,
        ),
        (
            "host_scope_per_day",
            limits.host_scope_per_day.saturating_sub(host),
            limits.host_scope_per_day,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(hours: i64, extra_seconds: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_800_000_000 + hours * 3_600 + extra_seconds, 0)
            .unwrap()
    }

    fn limits_default() -> BudgetLimits {
        BudgetLimits::default()
    }

    #[test]
    fn anchors_are_stable_hours_and_days() {
        // 1_800_000_000 is 08:00 UTC on day 20833: an hour boundary, mid-day.
        let t = at(0, 0);
        assert_eq!(hour_index(t), hour_index(at(0, 3_599)));
        assert_eq!(hour_index(t) + 1, hour_index(at(1, 0)));
        // The last second of the day shares the anchor; midnight rolls it.
        assert_eq!(day_index(t), day_index(at(0, 57_599)));
        assert_eq!(day_index(t) + 1, day_index(at(0, 57_600)));
    }

    #[test]
    fn low_risk_exhausts_its_hourly_window() {
        let mut windows = BudgetWindows::default();
        let limits = BudgetLimits {
            low_risk_per_hour: 2,
            ..BudgetLimits::default()
        };
        let now = at(0, 0);

        assert!(reserve(
            &mut windows,
            &limits,
            RiskClass::LowRisk,
            BlastRadius::None,
            now
        ));
        assert!(reserve(
            &mut windows,
            &limits,
            RiskClass::LowRisk,
            BlastRadius::None,
            now
        ));

        assert!(
            !reserve(
                &mut windows,
                &limits,
                RiskClass::LowRisk,
                BlastRadius::None,
                now
            ),
            "the third low-risk step this hour is exhausted"
        );

        // A refund returns the unit without waiting for the window to roll.
        refund(&mut windows, RiskClass::LowRisk, BlastRadius::None, now);
        assert!(reserve(
            &mut windows,
            &limits,
            RiskClass::LowRisk,
            BlastRadius::None,
            now
        ));

        // The window rolls with time: the next hour is empty again.
        let next_hour = at(1, 0);
        let (_, left, limit) = remaining(&windows, &limits, next_hour)
            .into_iter()
            .find(|(name, _, _)| *name == "low_risk_per_hour")
            .unwrap();
        assert_eq!((left, limit), (2, 2), "the rollover restored the budget");
        assert!(reserve(
            &mut windows,
            &limits,
            RiskClass::LowRisk,
            BlastRadius::None,
            next_hour
        ));
    }

    #[test]
    fn controlled_exhausts_its_daily_window() {
        let mut windows = BudgetWindows::default();
        let limits = BudgetLimits {
            controlled_per_day: 1,
            ..BudgetLimits::default()
        };
        let now = at(0, 0);

        assert!(reserve(
            &mut windows,
            &limits,
            RiskClass::Controlled,
            BlastRadius::Environment,
            now
        ));
        assert!(!reserve(
            &mut windows,
            &limits,
            RiskClass::Controlled,
            BlastRadius::Environment,
            now
        ));

        // The next UTC day rolls the window.
        assert!(reserve(
            &mut windows,
            &limits,
            RiskClass::Controlled,
            BlastRadius::Environment,
            at(24, 0)
        ));
    }

    #[test]
    fn host_scope_bounds_every_risk_at_host_scope() {
        let mut windows = BudgetWindows::default();
        let limits = BudgetLimits {
            host_scope_per_day: 1,
            ..BudgetLimits::default()
        };
        let now = at(0, 0);

        // A low-risk step at host scope consumed the host-scope day window…
        assert!(reserve(
            &mut windows,
            &limits,
            RiskClass::LowRisk,
            BlastRadius::Host,
            now
        ));

        // …so a controlled step at host scope is now exhausted too, even
        // though its own daily window is untouched.
        assert!(
            !reserve(
                &mut windows,
                &limits,
                RiskClass::Controlled,
                BlastRadius::Host,
                now
            ),
            "the host-scope window bounds any risk at host scope"
        );
        // Off-host the same risk still has room.
        assert!(reserve(
            &mut windows,
            &limits,
            RiskClass::Controlled,
            BlastRadius::Environment,
            now
        ));
    }

    #[test]
    fn shapes_without_an_applicable_window_consume_nothing() {
        // Off-host, no window applies to a read or a destructive step: the
        // reserve passes through untouched (the autonomy matrix, not the
        // budget, is what gates those risks).
        let mut windows = BudgetWindows::default();
        let limits = BudgetLimits::default();
        let now = at(0, 0);
        assert!(reserve(
            &mut windows,
            &limits,
            RiskClass::Read,
            BlastRadius::Environment,
            now
        ));
        assert!(reserve(
            &mut windows,
            &limits,
            RiskClass::HighRisk,
            BlastRadius::Environment,
            now
        ));
        assert!(reserve(
            &mut windows,
            &limits,
            RiskClass::Destructive,
            BlastRadius::None,
            now
        ));
        assert!(
            remaining(&windows, &limits, now)
                .iter()
                .all(|(_, left, limit)| left == limit),
            "nothing was consumed — the windows may have rolled, never filled"
        );

        // At host scope the host-scope window applies to any risk: bounded
        // host work, whatever its class.
        assert!(reserve(
            &mut windows,
            &limits,
            RiskClass::Read,
            BlastRadius::Host,
            now
        ));
        let host = remaining(&windows, &limits, now)
            .into_iter()
            .find(|(name, _, _)| *name == "host_scope_per_day")
            .unwrap();
        assert_eq!(host, ("host_scope_per_day", 2, 3));
    }

    #[test]
    fn reserve_only_consumes_the_applicable_windows() {
        let mut windows = BudgetWindows::default();
        let now = at(5, 30);
        assert!(reserve(
            &mut windows,
            &limits_default(),
            RiskClass::LowRisk,
            BlastRadius::Environment,
            now
        ));
        assert_eq!(windows.low_risk_hour, Some((hour_index(now), 1)));
        assert_eq!(
            windows.controlled_day, None,
            "low risk consumes no daily window"
        );
        assert_eq!(
            windows.host_scope_day, None,
            "an off-host step consumes no host window"
        );
    }

    #[test]
    fn limits_deserialize_with_defaults_and_clamp_to_one() {
        let limits: BudgetLimits = serde_json::from_str("{}").unwrap();
        assert_eq!(limits, BudgetLimits::default());

        let limits: BudgetLimits =
            serde_json::from_str(r#"{ "low_risk_per_hour": 0, "controlled_per_day": 5000 }"#)
                .unwrap();
        assert_eq!(limits.low_risk_per_hour, 1, "clamped to the floor");
        assert_eq!(limits.controlled_per_day, 5000);
        assert_eq!(
            limits.host_scope_per_day, 3,
            "the absent key keeps its default"
        );
    }

    #[test]
    fn exhaustion_is_reported_in_the_remaining_view() {
        let mut windows = BudgetWindows::default();
        let limits = BudgetLimits {
            host_scope_per_day: 3,
            ..BudgetLimits::default()
        };
        let now = at(0, 0);
        for _ in 0..3 {
            assert!(reserve(
                &mut windows,
                &limits,
                RiskClass::Read,
                BlastRadius::Host,
                now
            ));
        }
        let view = remaining(&windows, &limits, now);
        let host = view
            .iter()
            .find(|(name, _, _)| *name == "host_scope_per_day")
            .unwrap();
        assert_eq!(
            *host,
            ("host_scope_per_day", 0, 3),
            "the empty budget is visible"
        );
    }

    #[test]
    fn refund_returns_units_only_in_the_applicable_windows_and_floors_at_zero() {
        let limits = BudgetLimits::default();
        let now = at(0, 0);

        // A refund with nothing reserved stays at zero, never negative.
        let mut windows = BudgetWindows::default();
        refund(&mut windows, RiskClass::LowRisk, BlastRadius::Host, now);
        assert_eq!(windows.low_risk_hour, Some((hour_index(now), 0)));
        assert_eq!(windows.host_scope_day, Some((day_index(now), 0)));

        // A refund touches only the windows the shape applies to.
        let mut windows = BudgetWindows::default();
        assert!(reserve(
            &mut windows,
            &limits,
            RiskClass::LowRisk,
            BlastRadius::Environment,
            now
        ));
        refund(&mut windows, RiskClass::Controlled, BlastRadius::None, now);
        assert_eq!(
            windows.low_risk_hour,
            Some((hour_index(now), 1)),
            "the low-risk unit is untouched by an unrelated refund"
        );
        refund(&mut windows, RiskClass::LowRisk, BlastRadius::None, now);
        assert_eq!(windows.low_risk_hour, Some((hour_index(now), 0)));
    }
}
