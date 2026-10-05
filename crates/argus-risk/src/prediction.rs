//! Labeled capacity/failure predictions (CAP-11, FR-012, T027).
//!
//! Every prediction is **explicitly labeled** — it carries its method,
//! confidence, and an explicit uncertainty band, and [`labeled_prose`] always
//! prefixes it with `PREDICTED` — so no consumer (human, cloud, or model) can
//! mistake it for fact or for authorization. The advisory risks derived here
//! cite the prediction as evidence and never authorize anything (P4).

use chrono::Duration;
use uuid::Uuid;

use argus_anomaly::TrendSample;
use argus_domain::{ObservationRef, Prediction, ResourceId, Severity, SignalKey, UncertaintyBand};

use crate::Risk;
use crate::RiskKind;

/// A prediction requires at least this many samples: fewer cannot support a
/// labeled uncertainty band honestly.
pub const MIN_TREND_SAMPLES: usize = 3;

/// Confidence saturates with sample count: `n / (n + 5)` — 3 samples cap at
/// 0.375, 20 at ~0.8 — multiplied by the fit's r².
const SAMPLE_CONFIDENCE_K: f64 = 5.0;

/// Issue a labeled linear-trend prediction for a capacity-style signal
/// (disk exhaustion, memory pressure, CPU/network saturation, Kubernetes
/// capacity, workload growth).
///
/// `samples` must be the recent history of the metric; they are sorted
/// defensively. Returns `None` when the history is too thin or has no time
/// spread — the function never fabricates a projection from insufficient
/// evidence. `now` is the last sample's timestamp, never the wall clock, so
/// output is deterministic given the same series.
pub fn predict_capacity(
    signal: SignalKey,
    samples: &[TrendSample],
    horizon: Duration,
    evidence: Vec<ObservationRef>,
) -> Option<Prediction> {
    capacity_prediction(signal, samples, horizon, evidence)
}

/// Issue a labeled prediction for recurring failures from a cumulative
/// restart-count series (restart trends, FR-012).
pub fn predict_recurring_failures(
    subject: ResourceId,
    restart_series: &[TrendSample],
    horizon: Duration,
    evidence: Vec<ObservationRef>,
) -> Option<Prediction> {
    let signal = SignalKey::new(subject, "restarts.cumulative").ok()?;
    capacity_prediction(signal, restart_series, horizon, evidence)
}

fn capacity_prediction(
    signal: SignalKey,
    samples: &[TrendSample],
    horizon: Duration,
    evidence: Vec<ObservationRef>,
) -> Option<Prediction> {
    if samples.len() < MIN_TREND_SAMPLES {
        return None;
    }
    let mut ordered: Vec<TrendSample> = samples.to_vec();
    ordered.sort_by_key(|s| s.at);

    let fit = argus_anomaly::Trend::fit(&ordered)?;
    let now = ordered[ordered.len() - 1].at;
    let at_horizon = now + horizon;
    let projected = fit.projected_value(at_horizon);
    let half_width = fit.interval_half_width(at_horizon);
    let band = UncertaintyBand::new(projected - half_width, projected + half_width).ok()?;

    // Confidence: fit quality × sample-count saturation, clamped to [0,1].
    let n = fit.sample_count() as f64;
    let confidence = (fit.r_squared().max(0.0) * (n / (n + SAMPLE_CONFIDENCE_K))) as f32;
    let confidence = confidence.clamp(0.0, 1.0);

    Prediction::new(
        Uuid::new_v4(),
        signal,
        ordered[ordered.len() - 1].value,
        fit.slope_per_second(),
        projected,
        horizon,
        "linear-trend",
        confidence,
        band,
        evidence,
    )
    .ok()
}

/// Advisory risk when the prediction's uncertainty band crosses `limit`
/// within its horizon. Severity is deterministic:
///
/// - `Error`   — even the band's *lower* edge exceeds the limit (certain breach);
/// - `Warning` — the projected value exceeds the limit;
/// - `Info`    — only the band's *upper* edge exceeds the limit (possible breach).
pub fn prediction_breach_risk(prediction: &Prediction, limit: f64, kind: RiskKind) -> Option<Risk> {
    let band = prediction.uncertainty();
    let (severity, cause) = if band.lower() > limit {
        (
            Severity::Error,
            format!(
                "the entire uncertainty band [{:.2}, {:.2}] already exceeds the {:.2} limit",
                band.lower(),
                band.upper(),
                limit
            ),
        )
    } else if prediction.projected_value() > limit {
        (
            Severity::Warning,
            format!(
                "the projected value {:.2} exceeds the {:.2} limit",
                prediction.projected_value(),
                limit
            ),
        )
    } else if band.upper() > limit {
        (
            Severity::Info,
            format!(
                "only the upper band edge {:.2} exceeds the {:.2} limit (possible breach)",
                band.upper(),
                limit
            ),
        )
    } else {
        return None;
    };

    Some(Risk {
        kind,
        subject: prediction.signal().subject().clone(),
        severity,
        evidence: vec![
            labeled_prose(prediction),
            format!(
                "within the {} horizon: {cause}",
                fmt_duration(prediction.horizon())
            ),
        ],
        recommendation: Some(
            "verify the trend with fresh observations before acting; any capacity \
             action must go through policy and never through this signal"
                .to_string(),
        ),
    })
}

/// Advisory reliability risk when the predicted *growth* in recurring
/// failures over the horizon reaches `threshold` additional restarts.
pub fn recurring_failure_risk(prediction: &Prediction, threshold: f64) -> Option<Risk> {
    let growth = prediction.projected_value() - prediction.current_value();
    if growth < threshold {
        return None;
    }
    Some(Risk {
        kind: RiskKind::Reliability,
        subject: prediction.signal().subject().clone(),
        severity: if growth >= threshold * 2.0 {
            Severity::Error
        } else {
            Severity::Warning
        },
        evidence: vec![
            labeled_prose(prediction),
            format!(
                "predicted {:.2} additional restarts within the horizon (threshold {:.2})",
                growth, threshold
            ),
        ],
        recommendation: Some(
            "investigate the failing unit; remediation stays inside the typed \
             capabilities and the approval path"
                .to_string(),
        ),
    })
}

/// Deterministic, explicitly-labeled prose for a prediction. The `PREDICTED`
/// prefix and the confidence/uncertainty figures are structural: a prediction
/// can never be rendered as a plain statement of fact (FR-012).
pub fn labeled_prose(prediction: &Prediction) -> String {
    format!(
        "PREDICTED ({}, confidence {:.2}): {} currently {:.2}, projected {:.2} in {} \
         — uncertainty [{:.2}, {:.2}], growth {:.6}/s; evidence: {} observation(s)",
        prediction.method(),
        prediction.confidence(),
        prediction.signal(),
        prediction.current_value(),
        prediction.projected_value(),
        fmt_duration(prediction.horizon()),
        prediction.uncertainty().lower(),
        prediction.uncertainty().upper(),
        prediction.growth_rate(),
        prediction.evidence().len(),
    )
}

/// Deterministic `h/m/s` rendering of a horizon.
fn fmt_duration(d: Duration) -> String {
    let total = d.num_seconds().max(0);
    let (h, rem) = (total / 3600, total % 3600);
    let (m, s) = (rem / 60, rem % 60);
    if h > 0 {
        format!("{h}h{m}m")
    } else if m > 0 {
        format!("{m}m{s}s")
    } else {
        format!("{s}s")
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn t0() -> chrono::DateTime<chrono::Utc> {
        chrono::Utc.with_ymd_and_hms(2026, 10, 5, 12, 0, 0).unwrap()
    }

    fn rising_disk_series() -> (SignalKey, Vec<TrendSample>) {
        let signal = SignalKey::new(
            ResourceId::new("host", "local").unwrap(),
            "disk.used_percent",
        )
        .unwrap();
        // ~+10% per 60s with sensor noise (a perfectly linear series would
        // have zero residual and therefore a zero-width uncertainty band).
        let samples = [80.0, 91.0, 99.0, 111.0, 120.0]
            .iter()
            .enumerate()
            .map(|(i, &v)| TrendSample {
                at: t0() + chrono::Duration::seconds(60 * i as i64),
                value: v,
            })
            .collect();
        (signal, samples)
    }

    fn evidence(n: usize) -> Vec<ObservationRef> {
        (0..n)
            .map(|_| ObservationRef::new(Uuid::new_v4()))
            .collect()
    }

    #[test]
    fn rising_disk_trend_predicts_exhaustion() {
        let (signal, samples) = rising_disk_series();
        let p = predict_capacity(signal, &samples, Duration::minutes(10), evidence(5)).unwrap();
        assert_eq!(p.method(), "linear-trend");
        assert_eq!(p.signal().metric(), "disk.used_percent");
        // +10%/60s over 600s: projected ≈ 80 + 100 = 180 from the observed 120.
        assert!((p.current_value() - 120.0).abs() < 1e-9);
        assert!(p.projected_value() > 150.0);
        // The uncertainty band contains the projection and grows past it.
        assert!(p.uncertainty().contains(p.projected_value()));
        assert!(p.uncertainty().upper() > p.projected_value());
        assert_eq!(p.evidence().len(), 5);
    }

    #[test]
    fn thin_history_is_never_predicted() {
        let (signal, samples) = rising_disk_series();
        assert!(predict_capacity(signal, &samples[..2], Duration::hours(1), vec![]).is_none());
    }

    #[test]
    fn flat_history_predicts_no_growth_and_no_breach() {
        let signal =
            SignalKey::new(ResourceId::new("host", "local").unwrap(), "cpu.utilization").unwrap();
        let samples: Vec<TrendSample> = (0..5)
            .map(|i| TrendSample {
                at: t0() + chrono::Duration::seconds(60 * i),
                value: 40.0,
            })
            .collect();
        let p = predict_capacity(signal, &samples, Duration::hours(1), evidence(3)).unwrap();
        assert!(p.growth_rate().abs() < 1e-9);
        assert!((p.projected_value() - 40.0).abs() < 1e-9);
        // Perfectly linear flat fit: zero residual, zero-width band.
        assert!(prediction_breach_risk(&p, 90.0, RiskKind::Capacity).is_none());
    }

    #[test]
    fn breach_severity_is_tiered_by_the_uncertainty_band() {
        let (signal, samples) = rising_disk_series();
        let p = predict_capacity(signal, &samples, Duration::minutes(10), vec![]).unwrap();

        // Certain breach: even the lower band edge passes 100.
        let certain = prediction_breach_risk(&p, 100.0, RiskKind::Capacity).unwrap();
        assert_eq!(certain.severity, Severity::Error);
        assert_eq!(certain.kind, RiskKind::Capacity);
        assert_eq!(certain.subject.as_str(), "host:local");

        // Possible breach: a limit between the projection and the upper band
        // edge is exceeded only at the band's upper edge.
        let possible_limit = (p.projected_value() + p.uncertainty().upper()) / 2.0;
        let possible = prediction_breach_risk(&p, possible_limit, RiskKind::Capacity).unwrap();
        assert_eq!(possible.severity, Severity::Info);
        assert!(possible.evidence[1].contains("possible breach"));

        // No breach above the entire band.
        assert!(
            prediction_breach_risk(&p, p.uncertainty().upper() + 100.0, RiskKind::Capacity)
                .is_none()
        );
    }

    #[test]
    fn recurring_restart_trend_yields_reliability_risk() {
        let subject = ResourceId::new("service", "checkout-api").unwrap();
        // Cumulative restarts accelerating: 2, 5, 9, 14 over four minutes.
        let series = [2.0, 5.0, 9.0, 14.0]
            .iter()
            .enumerate()
            .map(|(i, &v)| TrendSample {
                at: t0() + chrono::Duration::seconds(60 * i as i64),
                value: v,
            })
            .collect::<Vec<_>>();
        let p = predict_recurring_failures(subject, &series, Duration::minutes(30), evidence(4))
            .unwrap();
        assert_eq!(p.signal().metric(), "restarts.cumulative");
        // ~+12 restarts/min → ≥ 10 more within 30 minutes.
        let risk = recurring_failure_risk(&p, 10.0).unwrap();
        assert_eq!(risk.kind, RiskKind::Reliability);
        assert_eq!(risk.severity, Severity::Error); // ≥ 2× threshold
        assert!(risk.recommendation.as_deref().unwrap().contains("typed"));

        // Below threshold: no risk.
        assert!(recurring_failure_risk(&p, 1000.0).is_none());
    }

    #[test]
    fn prose_is_always_explicitly_labeled() {
        let (signal, samples) = rising_disk_series();
        let p = predict_capacity(signal, &samples, Duration::minutes(10), evidence(2)).unwrap();
        let prose = labeled_prose(&p);
        assert!(prose.starts_with("PREDICTED (linear-trend"));
        assert!(prose.contains("confidence"));
        assert!(prose.contains("uncertainty ["));
        assert!(prose.contains("disk.used_percent on host:local"));
        assert!(prose.contains("evidence: 2 observation(s)"));
        // The advisory risk cites the labeled prose, never a bare claim.
        let risk = prediction_breach_risk(&p, 100.0, RiskKind::Capacity).unwrap();
        assert!(risk.evidence[0].starts_with("PREDICTED"));
    }

    #[test]
    fn prediction_output_is_deterministic_given_the_same_series() {
        let (signal, samples) = rising_disk_series();
        let a = predict_capacity(signal.clone(), &samples, Duration::minutes(10), vec![]).unwrap();
        let b = predict_capacity(signal, &samples, Duration::minutes(10), vec![]).unwrap();
        // Same fit, projection, band, and confidence (only ids differ).
        assert_eq!(a.projected_value(), b.projected_value());
        assert_eq!(a.uncertainty(), b.uncertainty());
        assert_eq!(a.confidence(), b.confidence());
        assert_eq!(labeled_prose(&a), labeled_prose(&b));
    }
}
