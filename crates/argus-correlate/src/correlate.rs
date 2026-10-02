//! Event correlation into operational situations (CAP-6).

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

use argus_domain::ResourceId;

/// An event with enough metadata to correlate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorrelatedEvent {
    pub id: Uuid,
    pub subject: ResourceId,
    /// The shared key that groups related events (e.g. `app:checkout-api`).
    pub correlation_key: String,
    pub timestamp: DateTime<Utc>,
}

/// A group of related events that share a correlation key within a window.
#[derive(Debug, Clone, PartialEq)]
pub struct Situation {
    pub id: Uuid,
    pub correlation_key: String,
    pub members: Vec<Uuid>,
    pub confidence: f32,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
}

impl Situation {
    pub fn member_count(&self) -> usize {
        self.members.len()
    }
}

/// Folds related events into situations. Events sharing a `correlation_key` and
/// arriving within `window` of the situation's last event join the same
/// situation; anything else opens a new one.
#[derive(Debug)]
pub struct Correlator {
    open: HashMap<String, Situation>,
    closed: Vec<Situation>,
    window: Duration,
}

impl Correlator {
    pub fn new(window: Duration) -> Self {
        Self {
            open: HashMap::new(),
            closed: Vec::new(),
            window,
        }
    }

    /// Ingest one event, returning the id of the situation it joined (existing
    /// or newly opened).
    pub fn ingest(&mut self, event: CorrelatedEvent) -> Uuid {
        if let Some(situation) = self.open.get_mut(&event.correlation_key)
            && event.timestamp.signed_duration_since(situation.last_seen) <= self.window
        {
            situation.members.push(event.id);
            situation.last_seen = event.timestamp;
            situation.confidence = confidence(situation.member_count());
            return situation.id;
        }

        // The window elapsed (or a new condition began): close the previous
        // situation for this key before opening a fresh one.
        if let Some(previous) = self.open.remove(&event.correlation_key) {
            self.closed.push(previous);
        }

        let id = Uuid::new_v4();
        self.open.insert(
            event.correlation_key.clone(),
            Situation {
                id,
                correlation_key: event.correlation_key.clone(),
                members: vec![event.id],
                confidence: confidence(1),
                first_seen: event.timestamp,
                last_seen: event.timestamp,
            },
        );
        id
    }

    pub fn situation(&self, id: Uuid) -> Option<&Situation> {
        self.open
            .values()
            .chain(self.closed.iter())
            .find(|s| s.id == id)
    }

    pub fn open_situations(&self) -> impl Iterator<Item = &Situation> {
        self.open.values()
    }

    pub fn closed_situations(&self) -> impl Iterator<Item = &Situation> {
        self.closed.iter()
    }

    /// Close a situation, moving it out of the open set. Later events with the
    /// same key start a new situation.
    pub fn close(&mut self, id: Uuid) -> Option<Situation> {
        let key = self
            .open
            .values()
            .find(|s| s.id == id)?
            .correlation_key
            .clone();
        let situation = self.open.remove(&key)?;
        self.closed.push(situation.clone());
        Some(situation)
    }
}

fn confidence(member_count: usize) -> f32 {
    // Saturating confidence that grows with corroborating members.
    (1.0 - 1.0 / (member_count as f32 + 1.0)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn event(id: u32, key: &str, secs: i64) -> CorrelatedEvent {
        CorrelatedEvent {
            id: Uuid::from_u128(id as u128),
            subject: ResourceId::new("service", &format!("svc-{id}")).unwrap(),
            correlation_key: key.to_string(),
            timestamp: Utc.with_ymd_and_hms(2026, 9, 30, 12, 0, 0).unwrap()
                + Duration::seconds(secs),
        }
    }

    #[test]
    fn related_events_correlate_into_one_situation() {
        let mut c = Correlator::new(Duration::minutes(10));
        let ids: Vec<Uuid> = (0..6)
            .map(|i| c.ingest(event(i, "app:checkout-api", i as i64 * 30)))
            .collect();
        // All six share one situation.
        assert!(ids.windows(2).all(|w| w[0] == w[1]));
        assert_eq!(c.open_situations().count(), 1);
        assert_eq!(c.situation(ids[0]).unwrap().member_count(), 6);
    }

    #[test]
    fn unrelated_events_open_separate_situations() {
        let mut c = Correlator::new(Duration::minutes(10));
        let a = c.ingest(event(1, "app:a", 0));
        let b = c.ingest(event(2, "app:b", 0));
        let cc = c.ingest(event(3, "app:c", 0));
        assert_ne!(a, b);
        assert_ne!(b, cc);
        assert_eq!(c.open_situations().count(), 3);
    }

    #[test]
    fn events_outside_window_close_the_old_situation() {
        let mut c = Correlator::new(Duration::minutes(1));
        let a = c.ingest(event(1, "app:a", 0));
        // 10 minutes later, same key, outside the window.
        let b = c.ingest(event(2, "app:a", 600));
        assert_ne!(a, b);
        assert_eq!(c.open_situations().count(), 1);
        assert_eq!(c.closed_situations().count(), 1);
    }

    #[test]
    fn close_moves_the_situation_to_closed() {
        let mut c = Correlator::new(Duration::minutes(10));
        let id = c.ingest(event(1, "app:a", 0));
        assert!(c.close(id).is_some());
        assert_eq!(c.open_situations().count(), 0);
        assert_eq!(c.closed_situations().count(), 1);
        assert!(c.situation(id).is_some());
    }

    #[test]
    fn confidence_grows_with_members() {
        let mut c = Correlator::new(Duration::minutes(10));
        let id = c.ingest(event(1, "app:a", 0));
        c.ingest(event(2, "app:a", 10));
        let situation = c.situation(id).unwrap();
        assert!(situation.confidence > 0.5);
    }
}
