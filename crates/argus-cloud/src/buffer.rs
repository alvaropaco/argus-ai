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
    Sentinel,
    /// Ledger: one execution attempt (`action.event`, spec 007).
    ActionLedger,
    /// Ledger: one bounded trace (`brain.trace`, spec 007).
    BrainTrace,
    /// Ledger: one usage record or rolling batch (`token.usage`, spec 007).
    TokenUsage,
}

impl ReportKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Telemetry => "telemetry",
            Self::Sentinel => "sentinel",
            Self::Health => "health",
            Self::Events => "events",
            Self::Activities => "activities",
            Self::ActionLedger => "actions",
            Self::BrainTrace => "traces",
            Self::TokenUsage => "usage",
        }
    }

    /// Whether this kind belongs to the action ledger's dedicated buffer,
    /// whose backpressure rules differ from the report queue's (spec 007
    /// FR-005: ledger events are never dropped; traces coalesce; usage
    /// batches).
    pub fn is_ledger(self) -> bool {
        matches!(
            self,
            Self::ActionLedger | Self::BrainTrace | Self::TokenUsage
        )
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

/// How many action events the upstream ledger buffer holds while offline
/// (spec 007 FR-005). Large by design: the producer awaits capacity, so this
/// bound is the sustained-burst ceiling, not a discard point.
pub const LEDGER_ACTION_CAPACITY: usize = 8192;
/// How many traces the buffer coalesces down to under pressure.
pub const LEDGER_TRACE_CAPACITY: usize = 256;
/// How long a producer waits for capacity before giving up for that record
/// (it is already safe in the local ledger; emission must never stall an
/// execution — spec 007 always-constraints).
pub const LEDGER_PUSH_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// The buffer is at action capacity; the caller must release its lock, let
/// the flusher run, and retry (or force room after the wait bound).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PushBlocked;

/// The rolling token-usage batch accumulated for one `(cycle_id, model)` pair
/// while waiting to be flushed (FR-005: usage batches).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UsageSum {
    /// Stable identity of the batch: minted when the batch is first created
    /// and carried through every drain/requeue cycle, so a lost-ack retry
    /// deduplicates at the cloud's `(instance_id, usage_id)` unique instead of
    /// double-counting.
    pub usage_id: Uuid,
    pub cycle_id: Option<Uuid>,
    pub model: Option<String>,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    /// Calls summed into this batch.
    pub calls: u32,
    /// Calls whose provider usage was absent — the unknowns (never zeros).
    pub unknown_usage: u32,
    pub duration_ms: u64,
}

impl UsageSum {
    fn merge(&mut self, other: &UsageSum) {
        // `usage_id` is intentionally untouched: the batch keeps the identity
        // it was minted with — merge is a re-count of the same rolling sum,
        // never a new record (a lost-ack retry must dedup at the cloud).
        // SQL-`SUM` semantics: unknowns do not drag known sums to zero; the
        // unknown count keeps them visible.
        self.prompt_tokens = sum_opt(self.prompt_tokens, other.prompt_tokens);
        self.completion_tokens = sum_opt(self.completion_tokens, other.completion_tokens);
        self.total_tokens = sum_opt(self.total_tokens, other.total_tokens);
        self.calls = self.calls.saturating_add(other.calls);
        self.unknown_usage = self.unknown_usage.saturating_add(other.unknown_usage);
        self.duration_ms = self.duration_ms.saturating_add(other.duration_ms);
    }
}

fn sum_opt(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.saturating_add(y)),
        _ => a,
    }
}

/// The bounded upstream buffer for the action ledger (spec 007 FR-005).
///
/// Three streams, three policies, one rule: **ledger action events are never
/// dropped**. Actions await capacity (bounded by [`LEDGER_PUSH_WAIT`] — the
/// record is already persisted locally, so a pathological stall degrades to a
/// logged upstream gap rather than a blocked execution). Traces coalesce the
/// oldest under pressure and count the loss. Usage batches into rolling sums
/// per `(cycle_id, model)`.
#[derive(Debug)]
pub struct LedgerBuffer {
    actions: VecDeque<BufferedReport>,
    traces: VecDeque<BufferedReport>,
    dropped_traces: u64,
    usage: Vec<UsageSum>,
}

impl Default for LedgerBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl LedgerBuffer {
    pub fn new() -> Self {
        Self {
            actions: VecDeque::new(),
            traces: VecDeque::new(),
            dropped_traces: 0,
            usage: Vec::new(),
        }
    }

    pub fn dropped_traces(&self) -> u64 {
        self.dropped_traces
    }

    pub fn action_len(&self) -> usize {
        self.actions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.actions.is_empty() && self.traces.is_empty() && self.usage.is_empty()
    }

    /// Attempts to admit one action event without waiting (FR-005: never
    /// dropped — the caller retries while the buffer is at capacity, releasing
    /// the lock between attempts so the flusher can drain and free room).
    ///
    /// Returns the entries sacrificed to make room (normally empty), or
    /// [`PushBlocked`] when the buffer is at capacity and nothing was inserted.
    pub fn try_push_action(
        &mut self,
        payload: Value,
        at: DateTime<Utc>,
    ) -> Result<Vec<BufferedReport>, PushBlocked> {
        if self.actions.len() >= LEDGER_ACTION_CAPACITY {
            return Err(PushBlocked);
        }
        self.actions.push_back(BufferedReport {
            entry_id: Uuid::new_v4(),
            kind: ReportKind::ActionLedger,
            payload,
            enqueued_at: at,
            attempts: 0,
        });
        Ok(Vec::new())
    }

    /// Makes room by sacrificing the oldest `count` action events, returning
    /// them so the caller can log the loss. The last resort of the capacity
    /// wait: every record here is still safe in the local ledger.
    pub fn sacrifice_actions(&mut self, count: usize) -> Vec<BufferedReport> {
        let mut dropped = Vec::with_capacity(count);
        for _ in 0..count {
            if let Some(entry) = self.actions.pop_front() {
                dropped.push(entry);
            }
        }
        if !dropped.is_empty() {
            tracing::warn!(
                count = dropped.len(),
                "the upstream ledger buffer overflowed; oldest action events gave way \
                 (all remain in the local ledger)"
            );
        }
        dropped
    }

    /// Adds one trace; under pressure the oldest coalesces away and the loss
    /// is counted onto the next trace's `coalesced_before` (FR-005). The
    /// counter resets once stamped, so the loss is reported exactly once —
    /// visible on the first trace pushed after the drop, not re-reported by
    /// every later trace.
    pub fn push_trace(&mut self, mut payload: Value, at: DateTime<Utc>) {
        while self.traces.len() >= LEDGER_TRACE_CAPACITY {
            if self.traces.pop_front().is_some() {
                self.dropped_traces = self.dropped_traces.saturating_add(1);
            }
        }
        if self.dropped_traces > 0
            && let Some(object) = payload.as_object_mut()
        {
            object.insert(
                "coalesced_before".to_string(),
                Value::from(self.dropped_traces),
            );
            self.dropped_traces = 0;
        }
        self.traces.push_back(BufferedReport {
            entry_id: Uuid::new_v4(),
            kind: ReportKind::BrainTrace,
            payload,
            enqueued_at: at,
            attempts: 0,
        });
    }

    /// Batches one usage record into the rolling sum for its
    /// `(cycle_id, model)` pair (FR-005).
    pub fn push_usage(&mut self, entry: UsageSum) {
        if let Some(existing) = self
            .usage
            .iter_mut()
            .find(|sum| sum.cycle_id == entry.cycle_id && sum.model == entry.model)
        {
            existing.merge(&entry);
        } else {
            self.usage.push(entry);
        }
    }

    /// Drains everything into sendable entries, stream by stream, oldest
    /// first. Usage batches materialize here, one entry per `(cycle_id, model)`.
    pub fn drain(&mut self, now: DateTime<Utc>) -> Vec<BufferedReport> {
        let mut drained: Vec<BufferedReport> = self.actions.drain(..).collect();
        drained.extend(self.traces.drain(..));
        for sum in self.usage.drain(..) {
            let payload = serde_json::json!({
                "usage_id": sum.usage_id,
                "cycle_id": sum.cycle_id,
                "model": sum.model,
                "prompt_tokens": sum.prompt_tokens,
                "completion_tokens": sum.completion_tokens,
                "total_tokens": sum.total_tokens,
                "calls": sum.calls,
                "unknown_usage": sum.unknown_usage,
                "duration_ms": sum.duration_ms,
                "occurred_at": now.to_rfc3339(),
            });
            drained.push(BufferedReport {
                entry_id: Uuid::new_v4(),
                kind: ReportKind::TokenUsage,
                payload,
                enqueued_at: now,
                attempts: 0,
            });
        }
        for entry in &mut drained {
            entry.attempts = entry.attempts.saturating_add(1);
        }
        drained
    }

    /// Returns undelivered entries, preserving order; usage entries re-batch
    /// into their rolling sums so a retry does not double-count them.
    pub fn requeue(&mut self, entries: Vec<BufferedReport>) {
        let mut actions = Vec::new();
        let mut traces = Vec::new();
        for entry in entries {
            match entry.kind {
                ReportKind::ActionLedger => actions.push(entry),
                ReportKind::BrainTrace => traces.push(entry),
                ReportKind::TokenUsage => {
                    if let Ok(value) = serde_json::from_value::<UsageSum>(entry.payload.clone()) {
                        if let Some(existing) = self
                            .usage
                            .iter_mut()
                            .find(|sum| sum.cycle_id == value.cycle_id && sum.model == value.model)
                        {
                            existing.merge(&value);
                        } else {
                            self.usage.push(value);
                        }
                    }
                }
                _ => {}
            }
        }
        // Entries arrive oldest-first; pushing each at the front in reverse
        // puts the oldest back at the very front without disturbing what is
        // already queued (which is newer).
        for entry in actions.into_iter().rev() {
            self.actions.push_front(entry);
        }
        for entry in traces.into_iter().rev() {
            self.traces.push_front(entry);
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

    mod ledger {
        use super::*;
        use serde_json::json;

        #[test]
        fn actions_buffer_in_order_and_never_drop_below_capacity() {
            let mut buffer = LedgerBuffer::new();
            for i in 0..16 {
                buffer
                    .try_push_action(json!({ "i": i }), at())
                    .expect("below capacity, admission is immediate");
            }
            let drained = buffer.drain(at());
            assert_eq!(drained.len(), 16);
            assert!(
                drained
                    .windows(2)
                    .all(|w| w[0].payload["i"].as_u64() < w[1].payload["i"].as_u64()),
                "per-instance ordering is preserved"
            );
            assert!(drained.iter().all(|e| e.kind == ReportKind::ActionLedger));
        }

        #[test]
        fn a_full_buffer_blocks_admission_and_sacrifice_frees_it() {
            let mut buffer = LedgerBuffer::new();
            for i in 0..LEDGER_ACTION_CAPACITY {
                buffer
                    .try_push_action(json!({ "i": i }), at())
                    .expect("fills exactly to capacity");
            }
            assert_eq!(
                buffer.try_push_action(json!({ "i": "blocked" }), at()),
                Err(super::super::PushBlocked),
                "at capacity the caller is told to release the lock and retry"
            );
            let dropped = buffer.sacrifice_actions(4);
            assert_eq!(dropped.len(), 4);
            assert_eq!(dropped[0].payload["i"], 0, "the oldest give way");
            buffer
                .try_push_action(json!({ "i": "after" }), at())
                .expect("room was made");
            assert_eq!(
                buffer.action_len(),
                LEDGER_ACTION_CAPACITY - 3,
                "four freed, one admitted"
            );
        }

        #[test]
        fn traces_coalesce_the_oldest_and_count_the_loss() {
            let mut buffer = LedgerBuffer::new();
            for i in 0..(LEDGER_TRACE_CAPACITY + 10) {
                buffer.push_trace(json!({ "i": i }), at());
            }
            let drained = buffer.drain(at());
            assert_eq!(drained.len(), LEDGER_TRACE_CAPACITY);
            // Each post-drop trace reports the loss since the last stamp and
            // then resets the counter: the loss is visible exactly once, not
            // re-reported by every later trace.
            assert_eq!(
                drained.last().unwrap().payload["coalesced_before"],
                1,
                "the loss is visible, never hidden"
            );
            assert_eq!(
                buffer.dropped_traces(),
                0,
                "the counter resets after stamping"
            );
            // A quiet trace afterwards carries no stale count.
            buffer.push_trace(json!({ "later": true }), at());
            assert!(
                buffer.drain(at())[0]
                    .payload
                    .get("coalesced_before")
                    .is_none(),
                "no stale coalesced count after reset"
            );
        }

        #[test]
        fn usage_batches_into_rolling_sums_per_cycle_and_model() {
            let mut buffer = LedgerBuffer::new();
            let cycle = Uuid::new_v4();
            let usage_id = Uuid::new_v4();
            for _ in 0..3 {
                buffer.push_usage(UsageSum {
                    usage_id,
                    cycle_id: Some(cycle),
                    model: Some("m".into()),
                    prompt_tokens: Some(10),
                    completion_tokens: Some(5),
                    total_tokens: Some(15),
                    calls: 1,
                    unknown_usage: 0,
                    duration_ms: 100,
                });
            }
            // An unknown stays unknown: it joins the sum as an unknown count.
            buffer.push_usage(UsageSum {
                usage_id: Uuid::new_v4(),
                cycle_id: Some(cycle),
                model: Some("m".into()),
                prompt_tokens: None,
                completion_tokens: None,
                total_tokens: None,
                calls: 1,
                unknown_usage: 1,
                duration_ms: 40,
            });
            // A different key batches separately.
            buffer.push_usage(UsageSum {
                usage_id: Uuid::new_v4(),
                cycle_id: None,
                model: None,
                prompt_tokens: Some(1),
                completion_tokens: Some(1),
                total_tokens: Some(2),
                calls: 1,
                unknown_usage: 0,
                duration_ms: 10,
            });

            let drained = buffer.drain(at());
            assert_eq!(drained.len(), 2, "one entry per (cycle, model) pair");
            let batched = &drained[0].payload;
            assert_eq!(batched["calls"], 4);
            assert_eq!(batched["prompt_tokens"], 30);
            assert_eq!(batched["total_tokens"], 45);
            assert_eq!(
                batched["unknown_usage"], 1,
                "the unknown call is counted, not zeroed"
            );
            assert_eq!(batched["duration_ms"], 340);
            assert_eq!(
                batched["usage_id"].as_str(),
                Some(usage_id.to_string()).as_deref(),
                "the batch keeps the id it was minted with"
            );
        }

        #[test]
        fn a_failed_flush_returns_in_order_and_usage_rebatches() {
            let mut buffer = LedgerBuffer::new();
            let usage_id = Uuid::new_v4();
            buffer
                .try_push_action(json!({ "i": 1 }), at())
                .expect("admits");
            buffer
                .try_push_action(json!({ "i": 2 }), at())
                .expect("admits");
            buffer.push_trace(json!({ "t": 1 }), at());
            buffer.push_usage(UsageSum {
                usage_id,
                cycle_id: None,
                model: None,
                prompt_tokens: Some(5),
                completion_tokens: Some(5),
                total_tokens: Some(10),
                calls: 1,
                unknown_usage: 0,
                duration_ms: 60,
            });

            let mut drained = buffer.drain(at());
            assert_eq!(drained.len(), 4);
            // The flush failed: everything comes back.
            buffer.requeue(std::mem::take(&mut drained));

            let again = buffer.drain(at());
            assert_eq!(again.len(), 4, "nothing was lost");
            assert_eq!(again[0].payload["i"], 1, "order restored");
            assert_eq!(again[1].payload["i"], 2);
            assert_eq!(again[3].payload["calls"], 1, "the usage batch is intact");
            assert_eq!(
                again[3].payload["usage_id"].as_str(),
                Some(usage_id.to_string()).as_deref(),
                "the id survives drain → requeue → drain, so a lost-ack retry dedups"
            );
        }

        #[test]
        fn an_empty_buffer_admits_actions_without_waiting() {
            let mut buffer = LedgerBuffer::new();
            let dropped = buffer
                .try_push_action(json!({ "quick": true }), at())
                .expect("admits");
            assert!(dropped.is_empty());
            assert_eq!(buffer.action_len(), 1);
        }
    }
}
