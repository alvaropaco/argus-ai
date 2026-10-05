//! Predictions: explicitly-labeled forward-looking estimates (CAP-11, FR-012).
//!
//! A [`Prediction`] is data — never authority, never fact. It always carries
//! the method that produced it, a confidence in `[0,1]`, and an explicit
//! uncertainty band, so no consumer can mistake it for an observed truth
//! (plan.md P4: evidence before inference; predictions are advisory only).

use chrono::Duration;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::DomainError;
use crate::id::ResourceId;

/// Identifies the predicted signal: which resource and which metric.
///
/// `metric` is a dotted lowercase name (e.g. `disk.used_percent`,
/// `memory.pressure`, `restarts.cumulative`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignalKey {
    subject: ResourceId,
    metric: String,
}

impl SignalKey {
    pub fn new(subject: ResourceId, metric: impl Into<String>) -> Result<Self, DomainError> {
        let metric = metric.into();
        if metric.is_empty() {
            return Err(DomainError::InvalidPrediction(
                "metric must be non-empty".to_string(),
            ));
        }
        Ok(Self { subject, metric })
    }

    pub fn subject(&self) -> &ResourceId {
        &self.subject
    }

    pub fn metric(&self) -> &str {
        &self.metric
    }
}

impl std::fmt::Display for SignalKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} on {}", self.metric, self.subject.as_str())
    }
}

/// A reference to the immutable observation backing one piece of prediction
/// evidence. Predictions cite observations; they never replace them
/// (Principle 4: observations are never overwritten by inference).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ObservationRef(Uuid);

impl ObservationRef {
    pub fn new(id: Uuid) -> Self {
        Self(id)
    }

    pub fn id(&self) -> Uuid {
        self.0
    }
}

/// An explicit uncertainty band around a projected value.
///
/// The data-model sketch wrote this as `Range<f64>`; a typed struct is the
/// serde-serializable equivalent (`std::ops::Range` has neither serde support
/// nor a value constructor).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)] // f64
pub struct UncertaintyBand {
    lower: f64,
    upper: f64,
}

impl UncertaintyBand {
    pub fn new(lower: f64, upper: f64) -> Result<Self, DomainError> {
        if lower > upper {
            return Err(DomainError::InvalidPrediction(format!(
                "uncertainty band lower {lower} exceeds upper {upper}"
            )));
        }
        Ok(Self { lower, upper })
    }

    pub fn lower(&self) -> f64 {
        self.lower
    }

    pub fn upper(&self) -> f64 {
        self.upper
    }

    /// Whether `value` falls inside the band.
    pub fn contains(&self, value: f64) -> bool {
        self.lower <= value && value <= self.upper
    }
}

/// A labeled projection of a monitored signal over a horizon (data-model §4).
///
/// Constructed only by the deterministic trend/prediction layer
/// (`argus-anomaly` + `argus-risk`); consumers treat it as advisory input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)] // f64 fields
pub struct Prediction {
    id: Uuid,
    signal: SignalKey,
    current_value: f64,
    /// Fitted growth in value units per second.
    growth_rate: f64,
    projected_value: f64,
    horizon: Duration,
    /// The deterministic method that produced the projection
    /// (e.g. `linear-trend`) — never a model-generated claim.
    method: String,
    confidence: f32,
    uncertainty: UncertaintyBand,
    evidence: Vec<ObservationRef>,
}

impl Prediction {
    /// Validates the invariants that make a prediction honest: positive
    /// horizon, non-empty method, confidence in `[0,1]`, and an uncertainty
    /// band that contains the projected value.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: Uuid,
        signal: SignalKey,
        current_value: f64,
        growth_rate: f64,
        projected_value: f64,
        horizon: Duration,
        method: impl Into<String>,
        confidence: f32,
        uncertainty: UncertaintyBand,
        evidence: Vec<ObservationRef>,
    ) -> Result<Self, DomainError> {
        if horizon <= Duration::zero() {
            return Err(DomainError::InvalidPrediction(
                "horizon must be positive".to_string(),
            ));
        }
        let method = method.into();
        if method.is_empty() {
            return Err(DomainError::InvalidPrediction(
                "method must be non-empty".to_string(),
            ));
        }
        if !(0.0..=1.0).contains(&confidence) {
            return Err(DomainError::ConfidenceOutOfRange(confidence));
        }
        if !uncertainty.contains(projected_value) {
            return Err(DomainError::InvalidPrediction(format!(
                "uncertainty band [{}, {}] must contain the projected value {projected_value}",
                uncertainty.lower(),
                uncertainty.upper()
            )));
        }
        Ok(Self {
            id,
            signal,
            current_value,
            growth_rate,
            projected_value,
            horizon,
            method,
            confidence,
            uncertainty,
            evidence,
        })
    }

    pub fn id(&self) -> Uuid {
        self.id
    }

    pub fn signal(&self) -> &SignalKey {
        &self.signal
    }

    pub fn current_value(&self) -> f64 {
        self.current_value
    }

    pub fn growth_rate(&self) -> f64 {
        self.growth_rate
    }

    pub fn projected_value(&self) -> f64 {
        self.projected_value
    }

    pub fn horizon(&self) -> Duration {
        self.horizon
    }

    pub fn method(&self) -> &str {
        &self.method
    }

    pub fn confidence(&self) -> f32 {
        self.confidence
    }

    pub fn uncertainty(&self) -> &UncertaintyBand {
        &self.uncertainty
    }

    pub fn evidence(&self) -> &[ObservationRef] {
        &self.evidence
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signal() -> SignalKey {
        SignalKey::new(
            ResourceId::new("host", "local").unwrap(),
            "disk.used_percent",
        )
        .unwrap()
    }

    #[test]
    fn accepts_a_well_formed_prediction() {
        let band = UncertaintyBand::new(90.0, 105.0).unwrap();
        let p = Prediction::new(
            Uuid::new_v4(),
            signal(),
            80.0,
            0.001,
            98.0,
            Duration::hours(6),
            "linear-trend",
            0.72,
            band,
            vec![ObservationRef::new(Uuid::new_v4())],
        )
        .unwrap();
        assert_eq!(p.method(), "linear-trend");
        assert_eq!(p.confidence(), 0.72);
        assert!(p.uncertainty().contains(98.0));
    }

    #[test]
    fn rejects_non_positive_horizon() {
        let band = UncertaintyBand::new(0.0, 1.0).unwrap();
        assert_eq!(
            Prediction::new(
                Uuid::new_v4(),
                signal(),
                0.0,
                0.0,
                0.5,
                Duration::zero(),
                "linear-trend",
                0.5,
                band,
                vec![]
            ),
            Err(DomainError::InvalidPrediction(
                "horizon must be positive".to_string()
            ))
        );
    }

    #[test]
    fn rejects_band_that_excludes_the_projection() {
        let band = UncertaintyBand::new(0.0, 1.0).unwrap();
        assert!(
            Prediction::new(
                Uuid::new_v4(),
                signal(),
                0.0,
                0.0,
                5.0,
                Duration::hours(1),
                "linear-trend",
                0.5,
                band,
                vec![]
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_out_of_range_confidence_and_empty_method() {
        let band = UncertaintyBand::new(0.0, 1.0).unwrap();
        let base = |confidence, method: &'static str| {
            Prediction::new(
                Uuid::new_v4(),
                signal(),
                0.0,
                0.0,
                0.5,
                Duration::hours(1),
                method,
                confidence,
                band,
                vec![],
            )
        };
        assert_eq!(
            base(1.5, "linear-trend"),
            Err(DomainError::ConfidenceOutOfRange(1.5))
        );
        assert!(base(0.5, "").is_err());
    }

    #[test]
    fn uncertainty_band_rejects_inverted_bounds() {
        assert!(UncertaintyBand::new(2.0, 1.0).is_err());
        let band = UncertaintyBand::new(1.0, 2.0).unwrap();
        assert!(band.contains(1.0) && band.contains(2.0));
        assert!(!band.contains(2.5));
    }

    #[test]
    fn signal_key_rejects_empty_metric() {
        let subject = ResourceId::new("host", "local").unwrap();
        assert!(SignalKey::new(subject, "").is_err());
    }

    #[test]
    fn serde_round_trip() {
        let p = Prediction::new(
            Uuid::new_v4(),
            signal(),
            80.0,
            0.001,
            98.0,
            Duration::hours(6),
            "linear-trend",
            0.72,
            UncertaintyBand::new(90.0, 105.0).unwrap(),
            vec![ObservationRef::new(Uuid::new_v4())],
        )
        .unwrap();
        let json = serde_json::to_string(&p).unwrap();
        let back: Prediction = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
    }
}
