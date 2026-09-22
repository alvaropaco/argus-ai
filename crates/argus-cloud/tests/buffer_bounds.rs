//! Report buffer tests (T049).
//!
//! Exercises the buffer through the crate's public API and asserts the four
//! properties FR-020 depends on: the bound is enforced, the oldest record is the
//! one discarded, enqueueing never blocks or fails, and every loss is counted.

use argus_cloud::buffer::{ReportKind, ReportQueue};
use chrono::{DateTime, TimeZone, Utc};
use serde_json::json;

fn at() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 21, 12, 0, 0).unwrap()
}

fn payload(tag: &str) -> serde_json::Value {
    json!({ "tag": tag })
}

#[test]
fn the_bound_is_enforced_regardless_of_how_much_is_enqueued() {
    let mut queue = ReportQueue::new(4);
    for i in 0..100 {
        queue.enqueue(ReportKind::Telemetry, payload(&format!("m{i}")), at());
    }

    assert_eq!(queue.len(), 4, "occupancy never exceeds the bound");
    assert_eq!(queue.stats().capacity, 4);
}

#[test]
fn the_oldest_record_is_the_one_discarded() {
    let mut queue = ReportQueue::new(3);
    for tag in ["first", "second", "third", "fourth"] {
        queue.enqueue(ReportKind::Events, payload(tag), at());
    }

    let remaining = queue.drain(3, at());
    let tags: Vec<&str> = remaining
        .iter()
        .map(|entry| entry.payload["tag"].as_str().unwrap())
        .collect();
    assert_eq!(
        tags,
        vec!["second", "third", "fourth"],
        "the newest data is kept and the oldest dropped, never the reverse"
    );
}

#[test]
fn enqueueing_never_fails_so_local_operation_is_never_blocked() {
    // The return type is the contract: an Option describing what was discarded,
    // never a Result that a caller would have to handle. A full buffer must not
    // be able to apply backpressure to the runtime.
    let mut queue = ReportQueue::new(1);

    let first = queue.enqueue(ReportKind::Health, payload("a"), at());
    assert!(first.is_none(), "nothing discarded while there is room");

    let second = queue.enqueue(ReportKind::Health, payload("b"), at());
    assert!(
        second.is_some(),
        "the caller is told what was lost, not given an error"
    );
}

#[test]
fn every_discard_is_counted_so_loss_is_visible_to_the_operator() {
    let mut queue = ReportQueue::new(2);
    for i in 0..7 {
        queue.enqueue(ReportKind::Activities, payload(&format!("a{i}")), at());
    }

    assert_eq!(
        queue.stats().dropped_total,
        5,
        "a silent loss would make the operator's view of the fleet wrong"
    );
}

#[test]
fn a_report_kind_survives_the_round_trip_through_the_queue() {
    let mut queue = ReportQueue::new(4);
    queue.enqueue(ReportKind::Telemetry, payload("t"), at());
    queue.enqueue(ReportKind::Health, payload("h"), at());
    queue.enqueue(ReportKind::Events, payload("e"), at());
    queue.enqueue(ReportKind::Activities, payload("a"), at());

    let drained = queue.drain(4, at());
    let kinds: Vec<&str> = drained.iter().map(|entry| entry.kind.as_str()).collect();
    assert_eq!(kinds, vec!["telemetry", "health", "events", "activities"]);
}

#[test]
fn a_failed_delivery_does_not_lose_reports() {
    let mut queue = ReportQueue::new(5);
    queue.enqueue(ReportKind::Events, payload("important"), at());

    let attempted = queue.drain(5, at());
    assert!(queue.is_empty(), "drained entries leave the queue first");

    queue.requeue_front(attempted);

    let retried = queue.drain(5, at());
    assert_eq!(retried.len(), 1);
    assert_eq!(retried[0].payload, payload("important"));
    assert_eq!(retried[0].attempts, 2, "the retry is recorded");
}

#[test]
fn an_empty_queue_drains_to_nothing() {
    let mut queue = ReportQueue::new(5);
    assert!(queue.drain(10, at()).is_empty());
    assert!(queue.is_empty());
    assert_eq!(queue.stats().dropped_total, 0);
}
