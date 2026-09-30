//! Per-signal deviation detection: static rules and historical baseline
//! excursions (CAP-4).

use crate::baseline::Baseline;

#[derive(Debug, Clone, Copy)]
pub struct DeviationConfig {
    /// A deviation requires an absolute z-score at least this large.
    pub z_threshold: f64,
    /// An optional absolute ceiling; exceeding it is a static-rule deviation
    /// even before the baseline is ready.
    pub hard_limit: Option<f64>,
}

impl Default for DeviationConfig {
    fn default() -> Self {
        Self {
            z_threshold: 3.0,
            hard_limit: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviationKind {
    /// Value exceeded the absolute hard limit.
    StaticRule,
    /// Value exceeded the historical baseline by `z_threshold` standard
    /// deviations.
    BaselineExcursion,
}

/// A detected deviation for one signal, with the evidence behind it.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::derive_partial_eq_without_eq)] // f64
pub struct Deviation {
    pub signal: String,
    pub value: f64,
    pub baseline_mean: f64,
    pub z_score: f64,
    pub kind: DeviationKind,
}

/// Detect whether `value` deviates for `signal`, preferring the static rule
/// (which fires even while the baseline is still warming up).
pub fn detect_deviation(
    signal: &str,
    value: f64,
    baseline: &Baseline,
    cfg: &DeviationConfig,
) -> Option<Deviation> {
    if cfg.hard_limit.is_some_and(|limit| value > limit) {
        return Some(Deviation {
            signal: signal.to_string(),
            value,
            baseline_mean: baseline.mean(),
            z_score: baseline.z_score(value).unwrap_or(0.0),
            kind: DeviationKind::StaticRule,
        });
    }

    if let Some(z) = baseline.z_score(value).filter(|z| z.abs() >= cfg.z_threshold) {
        return Some(Deviation {
            signal: signal.to_string(),
            value,
            baseline_mean: baseline.mean(),
            z_score: z,
            kind: DeviationKind::BaselineExcursion,
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready_baseline(samples: &[f64]) -> Baseline {
        let mut b = Baseline::new(10, 2);
        for v in samples {
            b.observe(*v);
        }
        b
    }

    #[test]
    fn static_rule_fires_even_when_baseline_is_unready() {
        let b = Baseline::new(10, 100); // never ready
        let cfg = DeviationConfig {
            z_threshold: 3.0,
            hard_limit: Some(90.0),
        };
        let d = detect_deviation("cpu", 95.0, &b, &cfg).unwrap();
        assert_eq!(d.kind, DeviationKind::StaticRule);
        assert_eq!(d.value, 95.0);
    }

    #[test]
    fn baseline_excursion_fires_when_z_is_high() {
        // baseline mean 50, stddev 10; value 85 is +3.5σ.
        let b = ready_baseline(&[40.0, 50.0, 60.0, 50.0, 50.0]);
        let cfg = DeviationConfig {
            z_threshold: 3.0,
            hard_limit: None,
        };
        let d = detect_deviation("cpu", 85.0, &b, &cfg).unwrap();
        assert_eq!(d.kind, DeviationKind::BaselineExcursion);
        assert!(d.z_score >= 3.0);
    }

    #[test]
    fn no_deviation_when_normal() {
        let b = ready_baseline(&[40.0, 50.0, 60.0, 50.0, 50.0]);
        let cfg = DeviationConfig::default();
        assert!(detect_deviation("cpu", 55.0, &b, &cfg).is_none());
    }

    #[test]
    fn no_excursion_when_baseline_unready_and_no_hard_limit() {
        let b = Baseline::new(10, 100);
        let cfg = DeviationConfig::default();
        assert!(detect_deviation("cpu", 9999.0, &b, &cfg).is_none());
    }
}
