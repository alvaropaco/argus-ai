//! A rolling statistical baseline of a numeric signal.

use std::collections::VecDeque;

/// A rolling window baseline: mean and standard deviation over the most recent
/// samples, with a readiness gate so a thin history never reads as a deviation.
#[derive(Debug, Clone)]
pub struct Baseline {
    samples: VecDeque<f64>,
    capacity: usize,
    min_samples: usize,
    mean: f64,
    variance: f64,
}

impl Baseline {
    /// `capacity` bounds the rolling window; `min_samples` is the readiness
    /// threshold (clamped to `capacity`).
    pub fn new(capacity: usize, min_samples: usize) -> Self {
        Self {
            samples: VecDeque::with_capacity(capacity),
            capacity,
            min_samples: min_samples.min(capacity),
            mean: 0.0,
            variance: 0.0,
        }
    }

    /// Record one sample and recompute the rolling statistics.
    pub fn observe(&mut self, value: f64) {
        self.samples.push_back(value);
        if self.samples.len() > self.capacity {
            self.samples.pop_front();
        }
        self.recompute();
    }

    /// Whether enough samples have been seen to trust a deviation.
    pub fn is_ready(&self) -> bool {
        self.samples.len() >= self.min_samples
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn mean(&self) -> f64 {
        self.mean
    }

    /// Population standard deviation over the current window.
    pub fn stddev(&self) -> f64 {
        self.variance.sqrt()
    }

    /// Z-score of `value` against the baseline. `None` when the baseline is not
    /// ready or has zero variance (in which case no score is meaningful).
    pub fn z_score(&self, value: f64) -> Option<f64> {
        if !self.is_ready() {
            return None;
        }
        let sd = self.stddev();
        if sd <= 0.0 {
            return None;
        }
        Some((value - self.mean) / sd)
    }

    fn recompute(&mut self) {
        let n = self.samples.len();
        if n == 0 {
            self.mean = 0.0;
            self.variance = 0.0;
            return;
        }
        let mean = self.samples.iter().sum::<f64>() / n as f64;
        let variance = self
            .samples
            .iter()
            .map(|&x| {
                let d = x - mean;
                d * d
            })
            .sum::<f64>()
            / n as f64;
        self.mean = mean;
        self.variance = variance;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mean_and_stddev_are_correct() {
        let mut b = Baseline::new(10, 2);
        for v in [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0] {
            b.observe(v);
        }
        assert!((b.mean() - 5.0).abs() < 1e-9);
        assert!((b.stddev() - 2.0).abs() < 1e-9);
    }

    #[test]
    fn ready_only_after_min_samples() {
        let mut b = Baseline::new(10, 3);
        b.observe(1.0);
        b.observe(2.0);
        assert!(!b.is_ready());
        b.observe(3.0);
        assert!(b.is_ready());
    }

    #[test]
    fn window_caps_sample_count() {
        let mut b = Baseline::new(3, 2);
        for v in [1.0, 2.0, 3.0, 4.0, 5.0] {
            b.observe(v);
        }
        assert_eq!(b.len(), 3);
    }

    #[test]
    fn min_samples_is_clamped_to_capacity() {
        let b = Baseline::new(3, 100);
        assert!(!b.is_ready()); // empty baseline is not ready
        assert_eq!(b.min_samples, 3);
    }

    #[test]
    fn z_score_is_none_when_unready() {
        let mut b = Baseline::new(10, 5);
        b.observe(1.0);
        assert_eq!(b.z_score(1.0), None);
    }

    #[test]
    fn z_score_is_none_for_zero_variance() {
        let mut b = Baseline::new(10, 3);
        for _ in 0..3 {
            b.observe(5.0);
        }
        assert_eq!(b.z_score(99.0), None);
    }

    #[test]
    fn z_score_is_correct() {
        let mut b = Baseline::new(10, 2);
        b.observe(0.0);
        b.observe(2.0); // mean 1, stddev 1
        let z = b.z_score(3.0).unwrap();
        assert!((z - 2.0).abs() < 1e-9);
    }
}
