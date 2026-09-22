//! Exponential backoff with full jitter.
//!
//! Reconnection must never hammer the cloud: the gateway rate-limits handshakes,
//! so a fixed-interval retry would turn a fleet-wide cloud restart into a
//! self-inflicted denial of service. Delays grow exponentially up to a cap and
//! carry full jitter so that many installations do not synchronise.
//!
//! The jitter source is a small deterministic generator that can be seeded
//! explicitly, which keeps the schedule testable without injecting an RNG
//! (research R1).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Exponential backoff with full jitter.
#[derive(Debug, Clone)]
pub struct Backoff {
    base: Duration,
    max: Duration,
    attempt: u32,
    state: u64,
}

impl Backoff {
    /// Creates a backoff seeded from the clock and process id.
    pub fn new(base: Duration, max: Duration) -> Self {
        Self::with_seed(base, max, seed_from_environment())
    }

    /// Creates a backoff with an explicit seed, for deterministic tests.
    ///
    /// A zero seed is replaced by the golden-ratio constant so the generator can
    /// never get stuck at zero.
    pub fn with_seed(base: Duration, max: Duration, seed: u64) -> Self {
        let state = if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        };
        Self {
            base,
            max: max.max(base),
            attempt: 0,
            state,
        }
    }

    /// Current attempt index (0 before the first delay).
    pub fn attempt(&self) -> u32 {
        self.attempt
    }

    /// The un-jittered ceiling for the current attempt, capped at `max`.
    pub fn ceiling(&self) -> Duration {
        let factor = 1u32.checked_shl(self.attempt.min(63)).unwrap_or(u32::MAX);
        self.base.saturating_mul(factor).min(self.max)
    }

    /// Returns the next delay and advances the attempt counter.
    ///
    /// Full jitter: uniformly distributed in `[0, ceiling]`.
    pub fn next_delay(&mut self) -> Duration {
        let ceiling_ms = self.ceiling().as_millis().min(u64::MAX as u128) as u64;
        let jittered_ms = if ceiling_ms == 0 {
            0
        } else {
            self.next_u64() % (ceiling_ms + 1)
        };
        self.attempt = self.attempt.saturating_add(1);
        Duration::from_millis(jittered_ms)
    }

    /// Resets the schedule. Called after a successful handshake.
    pub fn reset(&mut self) {
        self.attempt = 0;
    }

    /// xorshift64*, adequate for jitter and free of dependencies.
    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

fn seed_from_environment() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x5DEE_CE66_D1CE_4E5B);
    let pid = std::process::id() as u64;
    nanos ^ pid.wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ceiling_grows_exponentially_then_caps() {
        let b = Backoff::with_seed(Duration::from_secs(1), Duration::from_secs(300), 42);
        assert_eq!(b.ceiling(), Duration::from_secs(1));
        let mut b = b;
        b.next_delay();
        assert_eq!(b.ceiling(), Duration::from_secs(2));
        b.next_delay();
        assert_eq!(b.ceiling(), Duration::from_secs(4));
        b.next_delay();
        assert_eq!(b.ceiling(), Duration::from_secs(8));
    }

    #[test]
    fn ceiling_never_exceeds_the_cap() {
        let mut b = Backoff::with_seed(Duration::from_secs(1), Duration::from_secs(10), 7);
        for _ in 0..40 {
            b.next_delay();
            assert!(
                b.ceiling() <= Duration::from_secs(10),
                "ceiling: {:?}",
                b.ceiling()
            );
        }
    }

    #[test]
    fn delays_stay_within_the_full_jitter_range() {
        let mut b = Backoff::with_seed(Duration::from_millis(100), Duration::from_secs(5), 99);
        for _ in 0..50 {
            let ceiling = b.ceiling();
            let delay = b.next_delay();
            assert!(
                delay <= ceiling,
                "delay {delay:?} exceeded ceiling {ceiling:?}"
            );
        }
    }

    #[test]
    fn reset_returns_the_schedule_to_the_start() {
        let mut b = Backoff::with_seed(Duration::from_secs(1), Duration::from_secs(300), 13);
        for _ in 0..5 {
            b.next_delay();
        }
        assert_eq!(b.attempt(), 5);
        b.reset();
        assert_eq!(b.attempt(), 0);
        assert_eq!(b.ceiling(), Duration::from_secs(1));
    }

    #[test]
    fn seeded_backoffs_are_deterministic() {
        let mut a = Backoff::with_seed(Duration::from_secs(1), Duration::from_secs(300), 2024);
        let mut b = Backoff::with_seed(Duration::from_secs(1), Duration::from_secs(300), 2024);
        for _ in 0..10 {
            assert_eq!(a.next_delay(), b.next_delay());
        }
    }

    #[test]
    fn different_seeds_produce_different_schedules() {
        let mut a = Backoff::with_seed(Duration::from_secs(1), Duration::from_secs(300), 1);
        let mut b = Backoff::with_seed(Duration::from_secs(1), Duration::from_secs(300), 2);
        let a_delays: Vec<_> = (0..10).map(|_| a.next_delay()).collect();
        let b_delays: Vec<_> = (0..10).map(|_| b.next_delay()).collect();
        assert_ne!(a_delays, b_delays);
    }

    #[test]
    fn zero_seed_is_replaced_and_still_produces_varying_delays() {
        let mut b = Backoff::with_seed(Duration::from_millis(500), Duration::from_secs(300), 0);
        let delays: Vec<_> = (0..8).map(|_| b.next_delay()).collect();
        assert!(
            delays.iter().any(|d| *d != delays[0]),
            "generator must not be stuck: {delays:?}"
        );
    }

    #[test]
    fn zero_base_yields_zero_delay_without_panicking() {
        let mut b = Backoff::with_seed(Duration::ZERO, Duration::from_secs(10), 5);
        assert_eq!(b.next_delay(), Duration::ZERO);
    }
}
