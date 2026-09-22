//! Heartbeat scheduling and liveness detection.
//!
//! The cloud negotiates `heartbeat_interval_seconds` at handshake and marks an
//! instance offline when heartbeats are missed, so this side must ping on the
//! agreed cadence and treat a missing reply as a lost connection rather than
//! waiting for the socket to fail on its own.
//!
//! Time is injected, so the schedule is deterministic in tests and needs no
//! sleeping.

use std::time::{Duration, Instant};

/// Sends liveness pings and detects when the peer has gone quiet.
#[derive(Debug, Clone)]
pub struct Heartbeat {
    interval: Duration,
    grace: Duration,
    last_seen: Instant,
}

impl Heartbeat {
    /// Builds a heartbeat for the negotiated interval.
    ///
    /// The grace period is half the interval, so a single lost ping is tolerated
    /// while a genuinely dead peer is still noticed promptly.
    pub fn new(interval_seconds: u64, now: Instant) -> Self {
        let interval = Duration::from_secs(interval_seconds.max(1));
        Self {
            interval,
            grace: interval / 2,
            last_seen: now,
        }
    }

    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// How long without a reply before the peer is considered gone.
    pub fn timeout(&self) -> Duration {
        self.interval + self.grace
    }

    /// Records a reply, restarting the schedule.
    pub fn note_seen(&mut self, now: Instant) {
        self.last_seen = now;
    }

    /// Whether a ping should be sent now.
    pub fn ping_due(&self, now: Instant) -> bool {
        now.duration_since(self.last_seen) >= self.interval
    }

    /// Whether the peer has been silent past the timeout.
    pub fn is_overdue(&self, now: Instant) -> bool {
        now.duration_since(self.last_seen) >= self.timeout()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn epoch() -> Instant {
        Instant::now()
    }

    #[test]
    fn the_negotiated_interval_is_honoured() {
        let start = epoch();
        let heartbeat = Heartbeat::new(20, start);
        assert_eq!(heartbeat.interval(), Duration::from_secs(20));
    }

    #[test]
    fn a_zero_interval_is_lifted_to_one_second() {
        // A peer negotiating zero must not produce a busy loop.
        let heartbeat = Heartbeat::new(0, epoch());
        assert_eq!(heartbeat.interval(), Duration::from_secs(1));
    }

    #[test]
    fn no_ping_is_due_before_the_interval_elapses() {
        let start = epoch();
        let heartbeat = Heartbeat::new(20, start);
        assert!(!heartbeat.ping_due(start + Duration::from_secs(19)));
    }

    #[test]
    fn a_ping_becomes_due_at_the_interval() {
        let start = epoch();
        let heartbeat = Heartbeat::new(20, start);
        assert!(heartbeat.ping_due(start + Duration::from_secs(20)));
        assert!(heartbeat.ping_due(start + Duration::from_secs(45)));
    }

    #[test]
    fn a_single_missed_ping_is_tolerated() {
        let start = epoch();
        let heartbeat = Heartbeat::new(20, start);
        assert!(
            !heartbeat.is_overdue(start + Duration::from_secs(20)),
            "one interval silent is within grace"
        );
        assert!(!heartbeat.is_overdue(start + Duration::from_secs(29)));
    }

    #[test]
    fn the_peer_is_overdue_once_the_timeout_passes() {
        let start = epoch();
        let heartbeat = Heartbeat::new(20, start);
        assert_eq!(heartbeat.timeout(), Duration::from_secs(30));
        assert!(heartbeat.is_overdue(start + Duration::from_secs(30)));
    }

    #[test]
    fn a_reply_restarts_the_schedule() {
        let start = epoch();
        let mut heartbeat = Heartbeat::new(20, start);

        let later = start + Duration::from_secs(25);
        heartbeat.note_seen(later);

        assert!(!heartbeat.ping_due(later));
        assert!(!heartbeat.is_overdue(later + Duration::from_secs(29)));
        assert!(heartbeat.is_overdue(later + Duration::from_secs(30)));
    }

    #[test]
    fn an_overdue_peer_is_also_ping_due() {
        // The two predicates must not disagree: a dead peer needs a reconnect,
        // not another ping into the void.
        let start = epoch();
        let heartbeat = Heartbeat::new(20, start);
        let past_timeout = start + Duration::from_secs(31);
        assert!(heartbeat.is_overdue(past_timeout));
        assert!(heartbeat.ping_due(past_timeout));
    }
}
