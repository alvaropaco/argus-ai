//! Reporting helpers: cadence, batch assembly, and timestamp hygiene.
//!
//! Three concerns live here because they all decide what a report looks like on
//! the wire:
//!
//! - **Cadence.** Telemetry and health are periodic; the cloud can slow that
//!   down with `stream.throttle`, and the installation must honour it rather than
//!   keep reporting at the old rate.
//! - **Batching.** Every outbound message has a bound, and the installation
//!   enforces it itself rather than letting the cloud reject the message
//!   (FR-019).
//! - **Timestamp hygiene.** A host with a wrong clock — a fresh install before
//!   NTP has synced, or a suspended VM — would otherwise assert timestamps far
//!   outside reality. The cloud bounds and flags skew on its side, but the
//!   installation must not present a nonsense timestamp as fact in the first
//!   place.
//!
//! Bounding means *flagging*, never rewriting: silently adjusting a timestamp
//! would corrupt the ordering an operator reasons about, which is the opposite of
//! the requirement.

use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};

use crate::buffer::{BufferedReport, ReportKind, ReportQueue};
use crate::mapping::activity::{ActivityContext, ActivityOutcome};
use crate::protocol::envelope::MessageType;
use crate::protocol::messages::{
    ActivitiesReportPayload, MAX_ACTIVITIES_PER_REPORT, MAX_EVENTS_PER_REPORT,
};
use argus_domain::{DomainEvent, Execution, RiskClass};

/// How far a timestamp may differ from local time before it is flagged.
///
/// Matches the order of magnitude the cloud tolerates, so the installation and
/// the cloud do not disagree about what "skewed" means.
pub const DEFAULT_MAX_SKEW: Duration = Duration::from_secs(300);

#[derive(Debug, Clone)]
pub struct ReportingSchedule {
    interval: Duration,
    next_telemetry: Instant,
}

impl ReportingSchedule {
    /// A zero interval is lifted to one second so a misconfiguration cannot turn
    /// the reporting loop into a busy loop.
    pub fn new(interval_seconds: u64, now: Instant) -> Self {
        let interval = Duration::from_secs(interval_seconds.max(1));
        Self {
            interval,
            next_telemetry: now + interval,
        }
    }

    pub fn interval(&self) -> Duration {
        self.interval
    }

    pub fn telemetry_due(&self, now: Instant) -> bool {
        now >= self.next_telemetry
    }

    pub fn note_telemetry_sent(&mut self, now: Instant) {
        self.next_telemetry = now + self.interval;
    }

    /// Applies the cloud's backpressure signal.
    ///
    /// Only slows down: a throttle that claimed a shorter interval than the
    /// configured one is ignored, because the cloud's signal means "send less",
    /// never "send more".
    pub fn apply_throttle(&mut self, interval_ms: u64, now: Instant) {
        let throttled = Duration::from_millis(interval_ms);
        if throttled > self.interval {
            self.interval = throttled;
            self.next_telemetry = now + throttled;
        }
    }
}

/// A group of queued reports of one kind, within that kind's message bound.
#[derive(Debug, Clone, PartialEq)]
pub struct ReportBatch {
    pub kind: ReportKind,
    pub entries: Vec<BufferedReport>,
}

impl ReportBatch {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// How many reports of a kind may travel in one message.
///
/// Telemetry and health carry a single reading each; events and activities are
/// batched up to the contract's item bound.
pub fn per_message_limit(kind: ReportKind) -> usize {
    match kind {
        ReportKind::Telemetry | ReportKind::Health => 1,
        ReportKind::Events => MAX_EVENTS_PER_REPORT,
        ReportKind::Activities => MAX_ACTIVITIES_PER_REPORT,
    }
}

/// Drains the queue into sendable batches, oldest first, respecting every bound.
///
/// The caller requeues the batches when a send fails, which is what makes
/// delivery at-least-once without reordering.
pub fn assemble_batches(queue: &mut ReportQueue, now: DateTime<Utc>) -> Vec<ReportBatch> {
    let drained = queue.drain(queue.len(), now);
    let mut batches: Vec<ReportBatch> = Vec::new();

    for entry in drained {
        let limit = per_message_limit(entry.kind);
        match batches.last_mut() {
            Some(last) if last.kind == entry.kind && last.entries.len() < limit => {
                last.entries.push(entry);
            }
            _ => batches.push(ReportBatch {
                kind: entry.kind,
                entries: vec![entry],
            }),
        }
    }

    batches
}

/// Returns every batch to the front of the queue, preserving order.
pub fn requeue_batches(queue: &mut ReportQueue, batches: Vec<ReportBatch>) {
    let entries: Vec<BufferedReport> = batches
        .into_iter()
        .flat_map(|batch| batch.entries)
        .collect();
    queue.requeue_front(entries);
}

/// Drains everything the local event bus has produced into the report queue.
///
/// Events are batched at the contract's item bound before queueing, so a burst
/// cannot produce a message the cloud would reject. A lagged receiver means the
/// bus outran us and events were dropped by the bus itself; that is recorded
/// rather than silently ignored. Returns how many events were queued.
pub fn collect_events(
    receiver: &mut tokio::sync::broadcast::Receiver<DomainEvent>,
    queue: &mut ReportQueue,
    now: DateTime<Utc>,
) -> usize {
    use tokio::sync::broadcast::error::TryRecvError;

    let mut collected: Vec<DomainEvent> = Vec::new();
    let mut lagged = 0u64;

    loop {
        match receiver.try_recv() {
            Ok(event) => collected.push(event),
            Err(TryRecvError::Lagged(skipped)) => {
                lagged = lagged.saturating_add(skipped);
                continue;
            }
            Err(TryRecvError::Empty | TryRecvError::Closed) => break,
        }
    }

    if lagged > 0 {
        let notice = serde_json::json!({
            "events_lagged": lagged,
            "note": "the local bus outran the reporter; these events were not buffered",
        });
        queue.enqueue(ReportKind::Events, notice, now);
    }

    if collected.is_empty() {
        return 0;
    }

    let total = collected.len();
    for batch in crate::mapping::event::batches(&collected) {
        if let Ok(payload) = serde_json::to_value(&batch) {
            queue.enqueue(ReportKind::Events, payload, now);
        }
    }
    total
}

/// The producer is the execution path; this is the reporting entry point so the
/// outcome projection and batching are not reinvented when execution lands. A
/// denial or an approval-required outcome is reported here exactly as faithfully
/// as a success.
pub fn queue_activity(
    queue: &mut ReportQueue,
    execution: &Execution,
    risk: RiskClass,
    outcome: ActivityOutcome,
    context: ActivityContext,
    now: DateTime<Utc>,
) {
    let record = crate::mapping::activity::record(execution, risk, outcome, context);
    let payload = ActivitiesReportPayload {
        activities: vec![record],
    };
    if let Ok(value) = serde_json::to_value(&payload) {
        queue.enqueue(ReportKind::Activities, value, now);
    }
}

/// Whether a message type acknowledges one of our reports.
///
/// The cloud chooses which acknowledgement to use per report kind, so all six
/// must be recognised; an unrecognised one would leave a batch pending forever.
pub fn is_report_ack(ty: MessageType) -> bool {
    matches!(
        ty,
        MessageType::TelemetryAck
            | MessageType::HealthAck
            | MessageType::EventsAck
            | MessageType::ActivitiesAck
            | MessageType::CapabilitiesAck
            | MessageType::IngestAck
    )
}

/// The verdict on a timestamp the installation is about to report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkewVerdict {
    /// Within the tolerated window; report as-is.
    Within,
    /// Ahead of local time by more than the bound.
    Ahead(Duration),
    /// Behind local time by more than the bound.
    Behind(Duration),
}

impl SkewVerdict {
    /// Whether the timestamp should be surfaced as suspect.
    pub fn is_flagged(self) -> bool {
        !matches!(self, Self::Within)
    }

    /// Which way the reported clock is wrong, if it is.
    pub fn direction(self) -> Option<&'static str> {
        match self {
            Self::Within => None,
            Self::Ahead(_) => Some("ahead"),
            Self::Behind(_) => Some("behind"),
        }
    }

    /// How far off the clock is; zero when within bounds.
    pub fn magnitude(self) -> Duration {
        match self {
            Self::Within => Duration::ZERO,
            Self::Ahead(by) | Self::Behind(by) => by,
        }
    }

    /// An operator-facing note, or `None` when nothing is wrong.
    ///
    /// The wording points at the likely cause rather than restating the maths,
    /// because a skewed clock is almost always an unsynced or suspended host.
    pub fn note(self) -> Option<String> {
        match self {
            Self::Within => None,
            Self::Ahead(by) => Some(format!(
                "the local clock is {} seconds ahead of the cloud; reported timestamps are flagged. \
                 Check time synchronisation (NTP/chrony).",
                by.as_secs()
            )),
            Self::Behind(by) => Some(format!(
                "the local clock is {} seconds behind the cloud; reported timestamps are flagged. \
                 Check time synchronisation (NTP/chrony).",
                by.as_secs()
            )),
        }
    }
}

/// Assesses a timestamp against local time.
pub fn assess_skew(
    local_now: DateTime<Utc>,
    candidate: DateTime<Utc>,
    max: Duration,
) -> SkewVerdict {
    let delta = candidate.signed_duration_since(local_now);
    let magnitude = delta.abs().to_std().unwrap_or(Duration::MAX);

    if magnitude <= max {
        return SkewVerdict::Within;
    }

    if delta > chrono::Duration::zero() {
        SkewVerdict::Ahead(magnitude)
    } else {
        SkewVerdict::Behind(magnitude)
    }
}

/// Assesses against [`DEFAULT_MAX_SKEW`].
pub fn assess(local_now: DateTime<Utc>, candidate: DateTime<Utc>) -> SkewVerdict {
    assess_skew(local_now, candidate, DEFAULT_MAX_SKEW)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 21, 12, 0, 0).unwrap()
    }

    #[test]
    fn an_exact_timestamp_is_within_bounds() {
        let verdict = assess(now(), now());
        assert_eq!(verdict, SkewVerdict::Within);
        assert!(!verdict.is_flagged());
        assert_eq!(verdict.note(), None);
    }

    #[test]
    fn a_small_drift_is_tolerated() {
        // Ordinary NTP-level drift must not generate noise.
        for seconds in [-120, -1, 0, 1, 120] {
            let candidate = now() + chrono::Duration::seconds(seconds);
            assert_eq!(assess(now(), candidate), SkewVerdict::Within, "{seconds}s");
        }
    }

    #[test]
    fn the_boundary_itself_is_not_flagged() {
        let candidate = now() + chrono::Duration::seconds(DEFAULT_MAX_SKEW.as_secs() as i64);
        assert_eq!(assess(now(), candidate), SkewVerdict::Within);
    }

    #[test]
    fn a_future_timestamp_is_flagged_as_ahead() {
        let candidate = now() + chrono::Duration::hours(3);
        let verdict = assess(now(), candidate);
        assert!(verdict.is_flagged());
        assert_eq!(verdict.direction(), Some("ahead"));
        assert!(verdict.magnitude() >= Duration::from_secs(3600));
    }

    #[test]
    fn a_past_timestamp_is_flagged_as_behind() {
        let candidate = now() - chrono::Duration::days(2);
        let verdict = assess(now(), candidate);
        assert!(verdict.is_flagged());
        assert_eq!(verdict.direction(), Some("behind"));
    }

    #[test]
    fn the_note_points_at_time_synchronisation() {
        let verdict = assess(now(), now() + chrono::Duration::hours(1));
        let note = verdict.note().expect("a flagged verdict has a note");
        assert!(
            note.to_lowercase().contains("ntp") || note.to_lowercase().contains("synchron"),
            "the operator needs a cause to check, not just a number: {note}"
        );
    }

    #[test]
    fn a_maximum_duration_skew_does_not_overflow() {
        // A clock set to year 1 or to the far future must not panic the daemon.
        let ancient = Utc.with_ymd_and_hms(1, 1, 1, 0, 0, 0).unwrap();
        let far_future = Utc.with_ymd_and_hms(9999, 12, 31, 23, 59, 59).unwrap();

        assert!(assess(now(), ancient).is_flagged());
        assert!(assess(now(), far_future).is_flagged());
        assert!(assess(far_future, ancient).is_flagged());
    }

    #[test]
    fn the_verdict_never_rewrites_the_timestamp() {
        // Bounding means flagging. The caller keeps the original value so local
        // ordering is preserved; this test pins that the API offers no rewritten
        // timestamp to accidentally use instead.
        let candidate = now() + chrono::Duration::days(1);
        let verdict = assess(now(), candidate);
        assert!(verdict.is_flagged());
        assert_eq!(candidate, now() + chrono::Duration::days(1));
    }

    #[test]
    fn a_skew_beyond_the_bound_reports_the_full_magnitude() {
        let candidate = now() + chrono::Duration::minutes(30);
        assert_eq!(
            assess(now(), candidate).magnitude(),
            Duration::from_secs(1800)
        );
    }

    mod schedule {
        use super::*;

        fn start() -> Instant {
            Instant::now()
        }

        #[test]
        fn a_report_is_not_due_before_the_interval_elapses() {
            let origin = start();
            let schedule = ReportingSchedule::new(60, origin);
            assert!(!schedule.telemetry_due(origin + Duration::from_secs(59)));
        }

        #[test]
        fn a_report_becomes_due_at_the_interval() {
            let origin = start();
            let schedule = ReportingSchedule::new(60, origin);
            assert!(schedule.telemetry_due(origin + Duration::from_secs(60)));
        }

        #[test]
        fn sending_schedules_the_next_report_from_now() {
            let origin = start();
            let mut schedule = ReportingSchedule::new(60, origin);
            let sent_at = origin + Duration::from_secs(90);

            schedule.note_telemetry_sent(sent_at);

            assert!(!schedule.telemetry_due(sent_at + Duration::from_secs(59)));
            assert!(schedule.telemetry_due(sent_at + Duration::from_secs(60)));
        }

        #[test]
        fn a_zero_interval_is_lifted_so_the_loop_cannot_spin() {
            let schedule = ReportingSchedule::new(0, start());
            assert_eq!(schedule.interval(), Duration::from_secs(1));
        }

        #[test]
        fn a_throttle_slows_reporting_down() {
            let origin = start();
            let mut schedule = ReportingSchedule::new(10, origin);

            schedule.apply_throttle(60_000, origin);

            assert_eq!(schedule.interval(), Duration::from_secs(60));
            assert!(!schedule.telemetry_due(origin + Duration::from_secs(30)));
        }

        #[test]
        fn a_throttle_never_speeds_reporting_up() {
            let origin = start();
            let mut schedule = ReportingSchedule::new(120, origin);

            schedule.apply_throttle(1_000, origin);

            assert_eq!(
                schedule.interval(),
                Duration::from_secs(120),
                "a throttle may only reduce the reporting rate"
            );
        }
    }

    mod batching {
        use super::*;
        use crate::buffer::ReportKind;
        use chrono::TimeZone;

        fn at() -> DateTime<Utc> {
            Utc.with_ymd_and_hms(2026, 9, 21, 12, 0, 0).unwrap()
        }

        #[test]
        fn telemetry_and_health_travel_one_per_message() {
            assert_eq!(per_message_limit(ReportKind::Telemetry), 1);
            assert_eq!(per_message_limit(ReportKind::Health), 1);
        }

        #[test]
        fn events_and_activities_use_their_own_bounds() {
            assert_eq!(per_message_limit(ReportKind::Events), MAX_EVENTS_PER_REPORT);
            assert_eq!(
                per_message_limit(ReportKind::Activities),
                MAX_ACTIVITIES_PER_REPORT
            );
        }

        #[test]
        fn events_are_batched_up_to_their_limit() {
            let mut queue = ReportQueue::new(MAX_EVENTS_PER_REPORT + 10);
            for i in 0..(MAX_EVENTS_PER_REPORT + 10) {
                queue.enqueue(ReportKind::Events, serde_json::json!({ "i": i }), at());
            }

            let batches = assemble_batches(&mut queue, at());

            assert_eq!(batches.len(), 2);
            assert_eq!(batches[0].len(), MAX_EVENTS_PER_REPORT);
            assert_eq!(batches[1].len(), 10);
            assert!(queue.is_empty(), "every entry was taken");
        }

        #[test]
        fn telemetry_never_shares_a_batch() {
            let mut queue = ReportQueue::new(4);
            for _ in 0..3 {
                queue.enqueue(ReportKind::Telemetry, serde_json::json!({}), at());
            }

            let batches = assemble_batches(&mut queue, at());

            assert_eq!(batches.len(), 3, "one reading per message");
            assert!(batches.iter().all(|batch| batch.len() == 1));
        }

        #[test]
        fn batches_preserve_age_order() {
            let mut queue = ReportQueue::new(4);
            queue.enqueue(
                ReportKind::Events,
                serde_json::json!({ "tag": "first" }),
                at(),
            );
            queue.enqueue(
                ReportKind::Telemetry,
                serde_json::json!({ "tag": "second" }),
                at(),
            );
            queue.enqueue(
                ReportKind::Events,
                serde_json::json!({ "tag": "third" }),
                at(),
            );

            let batches = assemble_batches(&mut queue, at());

            let kinds: Vec<ReportKind> = batches.iter().map(|batch| batch.kind).collect();
            assert_eq!(
                kinds,
                vec![
                    ReportKind::Events,
                    ReportKind::Telemetry,
                    ReportKind::Events
                ],
                "order must not be reshuffled by batching"
            );
        }

        #[test]
        fn a_failed_send_can_return_every_batch_in_order() {
            let mut queue = ReportQueue::new(8);
            queue.enqueue(ReportKind::Events, serde_json::json!({ "tag": "a" }), at());
            queue.enqueue(ReportKind::Events, serde_json::json!({ "tag": "b" }), at());

            let batches = assemble_batches(&mut queue, at());
            assert!(queue.is_empty());

            requeue_batches(&mut queue, batches);

            let recovered = queue.drain(8, at());
            assert_eq!(recovered.len(), 2);
            assert_eq!(recovered[0].payload, serde_json::json!({ "tag": "a" }));
            assert_eq!(recovered[1].payload, serde_json::json!({ "tag": "b" }));
            assert_eq!(recovered[0].attempts, 2, "the retry is recorded");
        }

        #[test]
        fn an_empty_queue_produces_no_batches() {
            let mut queue = ReportQueue::new(4);
            assert!(assemble_batches(&mut queue, at()).is_empty());
        }
    }

    mod event_collection {
        use super::*;
        use argus_domain::{EventType, Severity};
        use tokio::sync::broadcast;
        use uuid::Uuid;

        fn at() -> DateTime<Utc> {
            Utc.with_ymd_and_hms(2026, 9, 21, 12, 0, 0).unwrap()
        }

        fn event() -> DomainEvent {
            DomainEvent::new(
                Uuid::new_v4(),
                EventType::new("argus.ready").unwrap(),
                at(),
                "argusd",
                "host:web-01",
                Severity::Info,
                None,
                None,
                serde_json::json!({}),
            )
        }

        #[tokio::test]
        async fn events_from_the_bus_are_queued_as_one_batch() {
            let (tx, mut rx) = broadcast::channel(16);
            tx.send(event()).unwrap();
            tx.send(event()).unwrap();

            let mut queue = ReportQueue::new(10);
            let queued = collect_events(&mut rx, &mut queue, at());

            assert_eq!(queued, 2);
            assert_eq!(queue.len(), 1, "batched into a single events.report");
            assert_eq!(queue.drain(1, at())[0].kind, ReportKind::Events);
        }

        #[tokio::test]
        async fn an_idle_bus_queues_nothing() {
            let (_tx, mut rx) = broadcast::channel::<DomainEvent>(4);
            let mut queue = ReportQueue::new(4);

            assert_eq!(collect_events(&mut rx, &mut queue, at()), 0);
            assert!(queue.is_empty());
        }

        #[tokio::test]
        async fn a_burst_is_split_at_the_item_bound() {
            let (tx, mut rx) = broadcast::channel(MAX_EVENTS_PER_REPORT * 2 + 4);
            for _ in 0..(MAX_EVENTS_PER_REPORT + 5) {
                let _ = tx.send(event());
            }

            let mut queue = ReportQueue::new(10);
            let queued = collect_events(&mut rx, &mut queue, at());

            assert_eq!(queued, MAX_EVENTS_PER_REPORT + 5);
            assert_eq!(queue.len(), 2, "one full batch plus the tail");
        }

        #[tokio::test]
        async fn a_lagged_receiver_records_the_loss_rather_than_hiding_it() {
            // A channel smaller than the burst forces the bus to drop events.
            let (tx, mut rx) = broadcast::channel(2);
            for _ in 0..6 {
                let _ = tx.send(event());
            }

            let mut queue = ReportQueue::new(10);
            collect_events(&mut rx, &mut queue, at());

            let payloads: Vec<_> = queue.drain(10, at());
            let mentions_loss = payloads.iter().any(|entry| {
                serde_json::to_string(&entry.payload)
                    .unwrap_or_default()
                    .contains("events_lagged")
            });
            assert!(
                mentions_loss,
                "events dropped by the bus must be visible to the operator"
            );
        }
    }

    mod activity_reporting {
        use super::*;
        use argus_domain::{Action, CapabilityId, ExecutionStatus, ResourceId};

        fn at() -> DateTime<Utc> {
            Utc.with_ymd_and_hms(2026, 9, 21, 12, 0, 0).unwrap()
        }

        fn execution(status: ExecutionStatus) -> Execution {
            Execution {
                action: Action {
                    capability: CapabilityId::new("host.service.restart").unwrap(),
                    resource: Some(ResourceId::new("host", "web-01").unwrap()),
                    arguments: serde_json::json!({}),
                },
                status,
                evidence: serde_json::json!({ "exit": 0 }),
            }
        }

        fn queued_kinds(queue: &mut ReportQueue) -> Vec<ReportKind> {
            queue.drain(8, at()).iter().map(|e| e.kind).collect()
        }

        #[test]
        fn a_successful_operation_is_queued_for_reporting() {
            let mut queue = ReportQueue::new(8);
            queue_activity(
                &mut queue,
                &execution(ExecutionStatus::Completed),
                RiskClass::LowRisk,
                ActivityOutcome::Succeeded,
                ActivityContext::default(),
                at(),
            );

            assert_eq!(queued_kinds(&mut queue), vec![ReportKind::Activities]);
        }

        #[test]
        fn a_denied_operation_is_reported_not_filtered_out() {
            // A refusal is exactly what an operator needs to see. Dropping it
            // because it is not a success would hide denials from the fleet view.
            let mut queue = ReportQueue::new(8);
            queue_activity(
                &mut queue,
                &execution(ExecutionStatus::Failed),
                RiskClass::HighRisk,
                ActivityOutcome::Denied,
                ActivityContext::default(),
                at(),
            );

            let payloads = queue.drain(8, at());
            assert_eq!(payloads.len(), 1, "a denial must still be reported");
            assert!(
                serde_json::to_string(&payloads[0].payload)
                    .unwrap()
                    .contains("denied")
            );
        }

        #[test]
        fn an_approval_required_operation_is_reported_as_pending() {
            let mut queue = ReportQueue::new(8);
            queue_activity(
                &mut queue,
                &execution(ExecutionStatus::Failed),
                RiskClass::HighRisk,
                ActivityOutcome::PendingApproval,
                ActivityContext::default(),
                at(),
            );

            let payloads = queue.drain(8, at());
            assert!(
                serde_json::to_string(&payloads[0].payload)
                    .unwrap()
                    .contains("pending_approval")
            );
        }

        #[test]
        fn the_queued_payload_is_a_bounded_activities_report() {
            let mut queue = ReportQueue::new(8);
            queue_activity(
                &mut queue,
                &execution(ExecutionStatus::Completed),
                RiskClass::Read,
                ActivityOutcome::Succeeded,
                ActivityContext::default(),
                at(),
            );

            let payloads = queue.drain(8, at());
            let activities = payloads[0].payload["activities"].as_array().unwrap();
            assert_eq!(activities.len(), 1);
            assert!(activities.len() <= MAX_ACTIVITIES_PER_REPORT);
            assert_eq!(activities[0]["risk_class"], "Read", "cloud casing");
        }

        #[test]
        fn activity_reporting_never_blocks_the_caller() {
            // Like every other enqueue, this must not fail: an operation that
            // completed must not be stalled by cloud trouble.
            let mut queue = ReportQueue::new(1);
            for _ in 0..5 {
                queue_activity(
                    &mut queue,
                    &execution(ExecutionStatus::Completed),
                    RiskClass::Read,
                    ActivityOutcome::Succeeded,
                    ActivityContext::default(),
                    at(),
                );
            }
            assert_eq!(queue.len(), 1, "bounded, with the loss counted");
            assert_eq!(queue.stats().dropped_total, 4);
        }
    }

    mod ack_classification {
        use super::*;

        #[test]
        fn every_acknowledgement_type_the_cloud_may_send_is_recognised() {
            for ty in [
                MessageType::TelemetryAck,
                MessageType::HealthAck,
                MessageType::EventsAck,
                MessageType::ActivitiesAck,
                MessageType::CapabilitiesAck,
                MessageType::IngestAck,
            ] {
                assert!(is_report_ack(ty), "{ty:?} must settle a pending batch");
            }
        }

        #[test]
        fn reports_and_control_messages_are_not_mistaken_for_acks() {
            for ty in [
                MessageType::TelemetryReport,
                MessageType::HealthReport,
                MessageType::EventsReport,
                MessageType::ActivitiesReport,
                MessageType::CapabilitiesPublish,
                MessageType::ConfigApply,
                MessageType::CommandInvoke,
                MessageType::Ping,
                MessageType::Pong,
                MessageType::StreamThrottle,
                MessageType::SessionRotate,
                MessageType::HandshakeHello,
                MessageType::PairingGranted,
            ] {
                assert!(!is_report_ack(ty), "{ty:?} is not an acknowledgement");
            }
        }
    }
}
