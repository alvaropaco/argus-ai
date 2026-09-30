//! Multi-signal anomaly detection: a deviation is corroborated when several
//! signals deviate together (CAP-4) — a single high sample alone is not an
//! incident.

use crate::deviation::Deviation;

#[derive(Debug, Clone, Copy)]
pub struct MultiSignalConfig {
    /// Number of co-deviating signals required to raise a multi-signal anomaly.
    pub min_correlated: usize,
}

impl Default for MultiSignalConfig {
    fn default() -> Self {
        Self { min_correlated: 2 }
    }
}

/// A multi-signal anomaly: several signals deviated in the same interval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultiSignalAnomaly {
    pub signals: Vec<String>,
}

/// Detect a multi-signal anomaly when at least `min_correlated` signals deviate
/// together.
pub fn detect_multi_signal(
    deviations: &[Deviation],
    config: &MultiSignalConfig,
) -> Option<MultiSignalAnomaly> {
    if deviations.len() < config.min_correlated {
        return None;
    }
    Some(MultiSignalAnomaly {
        signals: deviations.iter().map(|d| d.signal.clone()).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deviation::DeviationKind;

    fn deviation(signal: &str) -> Deviation {
        Deviation {
            signal: signal.to_string(),
            value: 100.0,
            baseline_mean: 50.0,
            z_score: 5.0,
            kind: DeviationKind::BaselineExcursion,
        }
    }

    #[test]
    fn detects_when_enough_signals_deviate() {
        let deviations = vec![deviation("cpu.utilization"), deviation("load.1m")];
        let anomaly = detect_multi_signal(&deviations, &MultiSignalConfig::default()).unwrap();
        assert_eq!(
            anomaly.signals,
            vec!["cpu.utilization".to_string(), "load.1m".to_string()]
        );
    }

    #[test]
    fn single_signal_is_not_multi_signal() {
        let deviations = vec![deviation("cpu.utilization")];
        assert!(detect_multi_signal(&deviations, &MultiSignalConfig::default()).is_none());
    }

    #[test]
    fn respects_the_threshold() {
        let deviations = vec![deviation("a"), deviation("b")];
        let config = MultiSignalConfig { min_correlated: 3 };
        assert!(detect_multi_signal(&deviations, &config).is_none());
    }
}
