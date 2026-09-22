//! Bounded report buffer with oldest-first discard.
//!
//! Reports accumulate while the cloud is unreachable. Two properties matter more
//! than throughput here:
//!
//! 1. **Local operation is never blocked.** Enqueueing always succeeds. A full
//!    buffer discards the oldest record rather than applying backpressure to the
//!    runtime, so a long cloud outage can never stall the host it manages.
//! 2. **Loss is counted, not hidden.** Every discard increments a counter that is
//!    surfaced in the next telemetry report, so an operator sees that data was
//!    dropped instead of silently missing it.
//!
//! Entries are delivered at least once: [`ReportQueue::drain`] removes them, and
//! [`ReportQueue::requeue_front`] puts them back in order when a send fails.

use std::collections::VecDeque;

use argus_domain::ReportBuffer as ReportBufferStats;
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

/// The batch bound an entry must respect is fixed by its kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportKind {
    Telemetry,
    Health,
    Events,
    Activities,
}

impl ReportKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Telemetry => "telemetry",
            Self::Health => "health",
            Self::Events => "events",
            Self::Activities => "activities",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BufferedReport {
    pub entry_id: Uuid,
    pub kind: ReportKind,
    pub payload: Value,
    pub enqueued_at: DateTime<Utc>,
    /// Delivery attempts so far, so a repeatedly failing entry is visible.
    pub attempts: u32,
}

#[derive(Debug, Clone)]
pub struct ReportQueue {
    capacity: usize,
    entries: VecDeque<BufferedReport>,
    dropped_total: u64,
    last_drain_at: Option<DateTime<Utc>>,
}

impl ReportQueue {
    /// A zero capacity is lifted to one so the buffer cannot silently discard
    /// everything it is given.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: VecDeque::new(),
            dropped_total: 0,
            last_drain_at: None,
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn dropped_total(&self) -> u64 {
        self.dropped_total
    }

    /// Adds a report, discarding the oldest when the buffer is full.
    ///
    /// Returns the discarded entry so the caller can log it; it never fails,
    /// because blocking the runtime on cloud trouble is the thing this type
    /// exists to prevent.
    pub fn enqueue(
        &mut self,
        kind: ReportKind,
        payload: Value,
        at: DateTime<Utc>,
    ) -> Option<BufferedReport> {
        let discarded = if self.entries.len() >= self.capacity {
            let dropped = self.entries.pop_front();
            if dropped.is_some() {
                self.dropped_total = self.dropped_total.saturating_add(1);
            }
            dropped
        } else {
            None
        };

        self.entries.push_back(BufferedReport {
            entry_id: Uuid::new_v4(),
            kind,
            payload,
            enqueued_at: at,
            attempts: 0,
        });

        discarded
    }

    /// Removes and returns up to `limit` reports, oldest first.
    pub fn drain(&mut self, limit: usize, at: DateTime<Utc>) -> Vec<BufferedReport> {
        let take = limit.min(self.entries.len());
        let mut taken: Vec<BufferedReport> = self.entries.drain(..take).collect();
        for entry in &mut taken {
            entry.attempts = entry.attempts.saturating_add(1);
        }
        if !taken.is_empty() {
            self.last_drain_at = Some(at);
        }
        taken
    }

    /// Returns reports to the front of the queue, preserving order.
    ///
    /// Used when a delivery attempt fails. Returning them to the front keeps the
    /// queue ordered by age, so a failed batch does not overtake newer reports.
    pub fn requeue_front(&mut self, entries: Vec<BufferedReport>) {
        for entry in entries.into_iter().rev() {
            self.entries.push_front(entry);
        }

        while self.entries.len() > self.capacity {
            if self.entries.pop_front().is_some() {
                self.dropped_total = self.dropped_total.saturating_add(1);
            }
        }
    }

    /// The occupancy snapshot an operator sees.
    pub fn stats(&self) -> ReportBufferStats {
        ReportBufferStats {
            capacity: self.capacity,
            count: self.entries.len(),
            dropped_total: self.dropped_total,
            last_drain_at: self.last_drain_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 21, 12, 0, 0).unwrap()
    }

    fn payload(tag: &str) -> Value {
        serde_json::json!({ "tag": tag })
    }

    #[test]
    fn an_empty_queue_reports_empty() {
        let queue = ReportQueue::new(10);
        assert!(queue.is_empty());
        assert_eq!(queue.len(), 0);
        assert_eq!(queue.dropped_total(), 0);
        assert_eq!(queue.stats().last_drain_at, None);
    }

    #[test]
    fn enqueueing_never_fails_even_when_full() {
        let mut queue = ReportQueue::new(2);
        assert!(
            queue
                .enqueue(ReportKind::Telemetry, payload("a"), at())
                .is_none()
        );
        assert!(
            queue
                .enqueue(ReportKind::Telemetry, payload("b"), at())
                .is_none()
        );

        let discarded = queue.enqueue(ReportKind::Telemetry, payload("c"), at());
        assert!(
            discarded.is_some(),
            "the oldest entry is returned, not an error"
        );
        assert_eq!(queue.len(), 2, "occupancy stays at capacity");
    }

    #[test]
    fn the_oldest_entry_is_the_one_discarded() {
        let mut queue = ReportQueue::new(2);
        queue.enqueue(ReportKind::Events, payload("oldest"), at());
        queue.enqueue(ReportKind::Events, payload("newer"), at());

        let discarded = queue
            .enqueue(ReportKind::Events, payload("newest"), at())
            .unwrap();
        assert_eq!(discarded.payload, payload("oldest"));

        let remaining = queue.drain(2, at());
        assert_eq!(remaining[0].payload, payload("newer"));
        assert_eq!(remaining[1].payload, payload("newest"));
    }

    #[test]
    fn every_discard_is_counted() {
        let mut queue = ReportQueue::new(1);
        queue.enqueue(ReportKind::Health, payload("a"), at());
        queue.enqueue(ReportKind::Health, payload("b"), at());
        queue.enqueue(ReportKind::Health, payload("c"), at());

        assert_eq!(
            queue.dropped_total(),
            2,
            "two records were lost and must be reported"
        );
        assert_eq!(queue.stats().dropped_total, 2);
    }

    #[test]
    fn drain_returns_reports_oldest_first_and_removes_them() {
        let mut queue = ReportQueue::new(5);
        for tag in ["a", "b", "c"] {
            queue.enqueue(ReportKind::Activities, payload(tag), at());
        }

        let drained = queue.drain(2, at());
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].payload, payload("a"));
        assert_eq!(drained[1].payload, payload("b"));
        assert_eq!(queue.len(), 1, "drained entries are removed");
        assert_eq!(queue.stats().last_drain_at, Some(at()));
    }

    #[test]
    fn drain_is_bounded_by_the_requested_limit() {
        let mut queue = ReportQueue::new(10);
        for _ in 0..10 {
            queue.enqueue(ReportKind::Events, payload("x"), at());
        }

        assert_eq!(queue.drain(3, at()).len(), 3);
        assert_eq!(queue.drain(100, at()).len(), 7, "never more than it holds");
        assert!(queue.is_empty());
    }

    #[test]
    fn draining_records_the_attempt() {
        let mut queue = ReportQueue::new(5);
        queue.enqueue(ReportKind::Telemetry, payload("a"), at());

        let first = queue.drain(1, at());
        assert_eq!(first[0].attempts, 1);

        queue.requeue_front(first);
        let second = queue.drain(1, at());
        assert_eq!(second[0].attempts, 2, "a retried report is visibly retried");
    }

    #[test]
    fn a_failed_delivery_can_be_returned_in_order() {
        let mut queue = ReportQueue::new(5);
        queue.enqueue(ReportKind::Events, payload("old"), at());
        queue.enqueue(ReportKind::Events, payload("new"), at());

        let failed = queue.drain(1, at());
        queue.requeue_front(failed);

        let after = queue.drain(5, at());
        assert_eq!(after[0].payload, payload("old"), "requeued at the front");
        assert_eq!(after[1].payload, payload("new"));
    }

    #[test]
    fn requeueing_beyond_capacity_discards_the_oldest_and_counts_it() {
        let mut queue = ReportQueue::new(2);
        let mut incoming = Vec::new();
        for tag in ["a", "b", "c"] {
            incoming.push(BufferedReport {
                entry_id: Uuid::new_v4(),
                kind: ReportKind::Telemetry,
                payload: payload(tag),
                enqueued_at: at(),
                attempts: 1,
            });
        }

        queue.requeue_front(incoming);

        assert_eq!(queue.len(), 2);
        assert_eq!(queue.dropped_total(), 1);
        let remaining = queue.drain(2, at());
        assert_eq!(
            remaining[0].payload,
            payload("b"),
            "oldest-first policy: 'a' is the entry that was discarded"
        );
        assert_eq!(remaining[1].payload, payload("c"));
    }

    #[test]
    fn a_zero_capacity_is_lifted_so_the_buffer_cannot_discard_everything() {
        let mut queue = ReportQueue::new(0);
        assert_eq!(queue.capacity(), 1);
        assert!(
            queue
                .enqueue(ReportKind::Health, payload("a"), at())
                .is_none()
        );
    }

    #[test]
    fn the_stats_snapshot_matches_the_queue() {
        let mut queue = ReportQueue::new(4);
        queue.enqueue(ReportKind::Events, payload("a"), at());
        queue.enqueue(ReportKind::Events, payload("b"), at());

        let stats = queue.stats();
        assert_eq!(stats.capacity, 4);
        assert_eq!(stats.count, 2);
        assert_eq!(stats.dropped_total, 0);
    }
}
