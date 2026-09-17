//! Health and readiness value objects.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Readiness state of the runtime or a component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthState {
    Ready,
    Degraded,
    NotReady,
}

/// A typed health report.
///
/// The `reason` is required (and only meaningful) when the state is
/// [`HealthState::Degraded`]; the constructors enforce this invariant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthStatus {
    state: HealthState,
    reason: Option<String>,
    checked_at: DateTime<Utc>,
}

impl HealthStatus {
    pub fn ready(checked_at: DateTime<Utc>) -> Self {
        Self {
            state: HealthState::Ready,
            reason: None,
            checked_at,
        }
    }

    pub fn degraded(reason: impl Into<String>, checked_at: DateTime<Utc>) -> Self {
        Self {
            state: HealthState::Degraded,
            reason: Some(reason.into()),
            checked_at,
        }
    }

    pub fn not_ready(checked_at: DateTime<Utc>) -> Self {
        Self {
            state: HealthState::NotReady,
            reason: None,
            checked_at,
        }
    }

    pub fn state(&self) -> HealthState {
        self.state
    }

    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }

    pub fn checked_at(&self) -> DateTime<Utc> {
        self.checked_at
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn ts() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 16, 12, 0, 0).unwrap()
    }

    #[test]
    fn ready_has_no_reason() {
        let h = HealthStatus::ready(ts());
        assert_eq!(h.state(), HealthState::Ready);
        assert_eq!(h.reason(), None);
    }

    #[test]
    fn degraded_requires_reason() {
        let h = HealthStatus::degraded("lancedb init failed", ts());
        assert_eq!(h.state(), HealthState::Degraded);
        assert_eq!(h.reason(), Some("lancedb init failed"));
    }

    #[test]
    fn not_ready_state() {
        let h = HealthStatus::not_ready(ts());
        assert_eq!(h.state(), HealthState::NotReady);
    }

    #[test]
    fn serde_round_trip() {
        let h = HealthStatus::degraded("boom", ts());
        let json = serde_json::to_string(&h).unwrap();
        let back: HealthStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(h, back);
        assert!(json.contains("degraded"));
    }
}
