//! ARGUS risk engine: advisory, typed risk signals (CAP-7).
//!
//! A [`Risk`] is evidence-backed but is **never** an authorization mechanism:
//! nothing reaches execution through a risk signal (it is data, never
//! authority). The risk model stays advisory by construction — including the
//! labeled predictions (CAP-11) this crate issues and classifies.

mod impact;
mod prediction;

use argus_domain::{ResourceId, Severity};
use serde::{Deserialize, Serialize};

pub use impact::{
    ImpactEstimate, RollbackFeasibility, SimulationInput, impact_summary, simulate_impact,
};
pub use prediction::{
    MIN_TREND_SAMPLES, labeled_prose, predict_capacity, predict_recurring_failures,
    prediction_breach_risk, recurring_failure_risk,
};

/// The category of a risk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskKind {
    Reliability,
    Security,
    Capacity,
    Configuration,
}

/// An advisory risk signal with the evidence behind it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Risk {
    pub kind: RiskKind,
    pub subject: ResourceId,
    pub severity: Severity,
    pub evidence: Vec<String>,
    /// Deterministic prose, never generated text.
    pub recommendation: Option<String>,
}

/// Classify restart loops as reliability risks.
pub fn restart_loop_risks(loops: &[argus_anomaly::RestartLoop], subject_kind: &str) -> Vec<Risk> {
    loops
        .iter()
        .map(|l| Risk {
            kind: RiskKind::Reliability,
            subject: ResourceId::new(subject_kind, &l.name).expect("valid resource id"),
            severity: Severity::Warning,
            evidence: vec![format!("{} restarts in one interval", l.restarts)],
            recommendation: Some(
                "investigate the failing unit; consider rollback or a restart".to_string(),
            ),
        })
        .collect()
}

/// Classify a multi-signal deviation as a capacity/reliability risk.
pub fn multi_signal_risk(anomaly: &argus_anomaly::MultiSignalAnomaly, subject: ResourceId) -> Risk {
    Risk {
        kind: RiskKind::Reliability,
        subject,
        severity: Severity::Warning,
        evidence: anomaly.signals.clone(),
        recommendation: Some(
            "correlate the deviating signals and investigate the common cause".to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_loops_classify_as_reliability_risks() {
        let loops = vec![argus_anomaly::RestartLoop {
            name: "checkout-api".to_string(),
            restarts: 5,
        }];
        let risks = restart_loop_risks(&loops, "service");
        assert_eq!(risks.len(), 1);
        assert_eq!(risks[0].kind, RiskKind::Reliability);
        assert_eq!(risks[0].subject.as_str(), "service:checkout-api");
        assert_eq!(risks[0].severity, Severity::Warning);
    }

    #[test]
    fn risk_serializes() {
        let risk = Risk {
            kind: RiskKind::Capacity,
            subject: ResourceId::new("host", "local").unwrap(),
            severity: Severity::Warning,
            evidence: vec!["disk 90%".to_string()],
            recommendation: Some("add capacity".to_string()),
        };
        let json = serde_json::to_string(&risk).unwrap();
        let back: Risk = serde_json::from_str(&json).unwrap();
        assert_eq!(back, risk);
    }
}
