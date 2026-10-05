//! Deterministic trend analysis: least-squares linear fits over a sample
//! series (CAP-11, T027).
//!
//! Pure math, no IO and no model inference — the fit and its projection
//! intervals feed the labeled predictions in `argus-risk`. Everything is
//! testable without a host (Constitution P9/P13).

use chrono::{DateTime, Utc};

/// One `(time, value)` sample of a monitored signal.
#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(clippy::derive_partial_eq_without_eq)] // f64
pub struct TrendSample {
    pub at: DateTime<Utc>,
    pub value: f64,
}

/// A least-squares linear fit over a sample series.
///
/// Time is measured in seconds; the fit is anchored at the series' mean
/// timestamp, so `projected_value(reference)` is the mean value exactly.
#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(clippy::derive_partial_eq_without_eq)] // f64
pub struct Trend {
    slope_per_second: f64,
    /// Fitted value at `reference` (the samples' mean timestamp).
    intercept_at_reference: f64,
    reference: DateTime<Utc>,
    r_squared: f64,
    residual_stddev: f64,
    /// Sum of squared deviations of the sample times from `reference`, in
    /// seconds² — the denominator of the prediction-interval width.
    sxx: f64,
    n: usize,
}

/// Slopes smaller than this (in value units per second) are treated as flat.
const FLAT_EPSILON: f64 = 1e-12;

impl Trend {
    /// Fit a line through `samples`. Returns `None` when fewer than two
    /// samples are given or all samples share one timestamp (no time spread
    /// to fit a slope against).
    pub fn fit(samples: &[TrendSample]) -> Option<Self> {
        if samples.len() < 2 {
            return None;
        }
        let n = samples.len() as f64;
        let first = samples[0].at;
        let t: Vec<f64> = samples
            .iter()
            .map(|s| (s.at - first).num_milliseconds() as f64 / 1000.0)
            .collect();
        let v: Vec<f64> = samples.iter().map(|s| s.value).collect();

        let t_mean = t.iter().sum::<f64>() / n;
        let v_mean = v.iter().sum::<f64>() / n;
        let sxx = t
            .iter()
            .map(|&ti| (ti - t_mean) * (ti - t_mean))
            .sum::<f64>();
        if sxx <= 0.0 {
            return None;
        }
        let sxy = t
            .iter()
            .zip(&v)
            .map(|(&ti, &vi)| (ti - t_mean) * (vi - v_mean))
            .sum::<f64>();

        let slope = sxy / sxx;
        let intercept = v_mean;
        let sse = v
            .iter()
            .zip(&t)
            .map(|(&vi, &ti)| {
                let resid = vi - (intercept + slope * (ti - t_mean));
                resid * resid
            })
            .sum::<f64>();
        let sst = v
            .iter()
            .map(|&vi| (vi - v_mean) * (vi - v_mean))
            .sum::<f64>();
        let r_squared = if sst <= 0.0 {
            // All values equal and the fit passes through them: perfect fit.
            1.0
        } else {
            1.0 - sse / sst
        };

        Some(Self {
            slope_per_second: slope,
            intercept_at_reference: intercept,
            reference: first + chrono::Duration::milliseconds((t_mean * 1000.0) as i64),
            r_squared,
            residual_stddev: (sse / n).sqrt(),
            sxx,
            n: samples.len(),
        })
    }

    pub fn slope_per_second(&self) -> f64 {
        self.slope_per_second
    }

    pub fn r_squared(&self) -> f64 {
        self.r_squared
    }

    pub fn residual_stddev(&self) -> f64 {
        self.residual_stddev
    }

    pub fn reference(&self) -> DateTime<Utc> {
        self.reference
    }

    pub fn sample_count(&self) -> usize {
        self.n
    }

    /// The fitted value at `at`.
    pub fn projected_value(&self, at: DateTime<Utc>) -> f64 {
        let dt = (at - self.reference).num_milliseconds() as f64 / 1000.0;
        self.intercept_at_reference + self.slope_per_second * dt
    }

    /// The half-width of the ~95% prediction interval at `at`:
    /// `z · σ · sqrt(1 + 1/n + (t−t̄)²/Sxx)` — the standard interval grows
    /// with distance from the data's time centroid, so uncertainty at the
    /// horizon is honest rather than flat.
    pub fn interval_half_width(&self, at: DateTime<Utc>) -> f64 {
        const Z: f64 = 1.96;
        let dt = (at - self.reference).num_milliseconds() as f64 / 1000.0;
        Z * self.residual_stddev * (1.0 + 1.0 / self.n as f64 + dt * dt / self.sxx).sqrt()
    }

    /// When the fitted value reaches `threshold`, given the trend heads
    /// toward it. `None` when the trend is flat or moving away — never an
    /// invented ETA.
    pub fn time_to_reach(&self, threshold: f64, from: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let gap = threshold - self.projected_value(from);
        let slope = self.slope_per_second;
        let heading_toward =
            (gap > 0.0 && slope > FLAT_EPSILON) || (gap < 0.0 && slope < -FLAT_EPSILON);
        if heading_toward {
            Some(from + chrono::Duration::milliseconds((gap / slope * 1000.0) as i64))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 5, 12, 0, 0).unwrap()
    }

    fn series(values: &[f64], step_seconds: i64) -> Vec<TrendSample> {
        values
            .iter()
            .enumerate()
            .map(|(i, &v)| TrendSample {
                at: t0() + chrono::Duration::seconds(step_seconds * i as i64),
                value: v,
            })
            .collect()
    }

    #[test]
    fn perfectly_linear_series_fits_exactly() {
        // 10 units per 60s → slope 1/6 per second, every residual zero.
        let samples = series(&[10.0, 20.0, 30.0, 40.0], 60);
        let fit = Trend::fit(&samples).unwrap();
        assert!((fit.slope_per_second - 1.0 / 6.0).abs() < 1e-9);
        assert!((fit.r_squared - 1.0).abs() < 1e-9);
        assert!(fit.residual_stddev < 1e-9);
        let last = samples[3].at;
        assert!((fit.projected_value(last) - 40.0).abs() < 1e-9);
    }

    #[test]
    fn projection_extrapolates_forward() {
        let samples = series(&[10.0, 20.0, 30.0, 40.0], 60);
        let fit = Trend::fit(&samples).unwrap();
        let last = samples[3].at;
        assert!((fit.projected_value(last + chrono::Duration::seconds(60)) - 50.0).abs() < 1e-9);
    }

    #[test]
    fn too_few_samples_or_no_time_spread_do_not_fit() {
        assert!(Trend::fit(&[]).is_none());
        assert!(Trend::fit(&series(&[1.0], 60)).is_none());
        // Same timestamp twice: no spread, no slope.
        let flat_time = vec![
            TrendSample {
                at: t0(),
                value: 1.0,
            },
            TrendSample {
                at: t0(),
                value: 2.0,
            },
        ];
        assert!(Trend::fit(&flat_time).is_none());
    }

    #[test]
    fn constant_series_is_flat_with_perfect_fit() {
        let fit = Trend::fit(&series(&[5.0, 5.0, 5.0, 5.0], 60)).unwrap();
        assert!(fit.slope_per_second.abs() < FLAT_EPSILON);
        assert!((fit.r_squared - 1.0).abs() < 1e-9);
    }

    #[test]
    fn time_to_reach_gives_eta_only_when_heading_toward_the_threshold() {
        let rising = Trend::fit(&series(&[10.0, 20.0, 30.0, 40.0], 60)).unwrap();
        let last = rising.reference();
        // The reference sits at the fitted mean value 25; at +10 per 60s,
        // reaching 70 takes (70-25)·6 = 270s.
        assert!((rising.projected_value(last) - 25.0).abs() < 1e-9);
        let eta = rising.time_to_reach(70.0, last).unwrap();
        let expected = last + chrono::Duration::seconds(270);
        assert!((eta - expected).num_seconds().abs() <= 1);

        // Flat trend: no ETA.
        let flat = Trend::fit(&series(&[5.0, 5.0, 5.0], 60)).unwrap();
        assert!(flat.time_to_reach(70.0, flat.reference()).is_none());

        // Falling trend never "reaches" a higher threshold.
        let falling = Trend::fit(&series(&[40.0, 30.0, 20.0], 60)).unwrap();
        assert!(falling.time_to_reach(70.0, falling.reference()).is_none());
        // But it does reach a lower one (e.g. free-memory depletion).
        assert!(falling.time_to_reach(10.0, falling.reference()).is_some());
    }

    #[test]
    fn interval_grows_with_distance_from_the_data() {
        // A noisy series with a clear trend.
        let samples = series(&[10.0, 22.0, 28.0, 41.0], 60);
        let fit = Trend::fit(&samples).unwrap();
        let near = fit.reference();
        let far = near + chrono::Duration::seconds(6 * 3600);
        assert!(fit.interval_half_width(far) > fit.interval_half_width(near));
        // The band is degenerate only for a perfect fit; here it is strictly positive.
        assert!(fit.interval_half_width(near) > 0.0);
    }
}
