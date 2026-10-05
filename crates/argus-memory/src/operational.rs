//! Operational memory: the latest observed value per (subject, attribute),
//! with a freshness gate so stale state is never served as current
//! (ADR-0038 §1, CAP-24).

use std::collections::HashMap;

use argus_domain::ResourceId;
use chrono::{DateTime, Utc};

/// The latest recorded value for one signal.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::derive_partial_eq_without_eq)] // f64
pub struct Current {
    pub subject: ResourceId,
    pub attribute: String,
    pub value: f64,
    pub observed_at: DateTime<Utc>,
}

/// Latest-value store keyed by `(subject, attribute)`. Writes are cheap and
/// idempotent per key; reads can be freshness-bounded.
#[derive(Debug, Default)]
pub struct OperationalMemory {
    latest: HashMap<(ResourceId, String), Current>,
}

impl OperationalMemory {
    /// Record one observation, superseding any previous value for the same
    /// key. An older timestamp never overwrites a newer one (out-of-order
    /// delivery cannot rewind state).
    pub fn record(&mut self, current: Current) {
        let key = (current.subject.clone(), current.attribute.clone());
        match self.latest.get(&key) {
            Some(existing) if existing.observed_at >= current.observed_at => return,
            _ => {}
        }
        self.latest.insert(key, current);
    }

    /// The current value for a signal, regardless of age.
    pub fn current(&self, subject: &ResourceId, attribute: &str) -> Option<&Current> {
        self.latest.get(&(subject.clone(), attribute.to_string()))
    }

    /// The current value only if it is fresh against `now` — stale state
    /// returns `None` instead of masquerading as current (CAP-24).
    pub fn current_within(
        &self,
        subject: &ResourceId,
        attribute: &str,
        now: DateTime<Utc>,
        max_age: chrono::Duration,
    ) -> Option<&Current> {
        let c = self.current(subject, attribute)?;
        if now.signed_duration_since(c.observed_at) <= max_age {
            Some(c)
        } else {
            None
        }
    }

    /// Every attribute currently held for one subject.
    pub fn attributes_for(&self, subject: &ResourceId) -> Vec<&Current> {
        let mut out: Vec<&Current> = self
            .latest
            .values()
            .filter(|c| c.subject == *subject)
            .collect();
        out.sort_by(|a, b| a.attribute.cmp(&b.attribute));
        out
    }

    pub fn len(&self) -> usize {
        self.latest.len()
    }

    pub fn is_empty(&self) -> bool {
        self.latest.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn subject() -> ResourceId {
        ResourceId::new("host", "local").unwrap()
    }

    fn ts(minute: u32) -> DateTime<Utc> {
        chrono::Utc
            .with_ymd_and_hms(2026, 10, 5, 12, minute, 0)
            .unwrap()
    }

    fn sample(value: f64, minute: u32) -> Current {
        Current {
            subject: subject(),
            attribute: "cpu.utilization".to_string(),
            value,
            observed_at: ts(minute),
        }
    }

    #[test]
    fn latest_value_wins_per_key() {
        let mut m = OperationalMemory::default();
        m.record(sample(10.0, 0));
        m.record(sample(20.0, 1));
        let c = m.current(&subject(), "cpu.utilization").unwrap();
        assert_eq!(c.value, 20.0);
    }

    #[test]
    fn out_of_order_delivery_cannot_rewind_state() {
        let mut m = OperationalMemory::default();
        m.record(sample(20.0, 5));
        m.record(sample(10.0, 1));
        assert_eq!(
            m.current(&subject(), "cpu.utilization").unwrap().value,
            20.0
        );
    }

    #[test]
    fn stale_state_is_not_served_as_current() {
        let mut m = OperationalMemory::default();
        m.record(sample(50.0, 0));
        let now = ts(30);
        assert!(
            m.current_within(
                &subject(),
                "cpu.utilization",
                now,
                chrono::Duration::minutes(5)
            )
            .is_none()
        );
        assert!(
            m.current_within(
                &subject(),
                "cpu.utilization",
                now,
                chrono::Duration::hours(1)
            )
            .is_some()
        );
    }

    #[test]
    fn attributes_are_listed_deterministically() {
        let mut m = OperationalMemory::default();
        for attribute in ["mem.pressure", "cpu.utilization", "disk.used_percent"] {
            m.record(Current {
                subject: subject(),
                attribute: attribute.to_string(),
                value: 1.0,
                observed_at: ts(0),
            });
        }
        let names: Vec<&str> = m
            .attributes_for(&subject())
            .iter()
            .map(|c| c.attribute.as_str())
            .collect();
        assert_eq!(
            names,
            ["cpu.utilization", "disk.used_percent", "mem.pressure"]
        );
    }
}
