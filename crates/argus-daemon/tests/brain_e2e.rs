//! End-to-end test of the Milestone 2 Operational Brain, all deterministic.
//!
//! Walks the `checkout-api` scenario through correlation → multi-signal anomaly
//! → advisory risk → incident (deduplicated) → investigation → root cause, and
//! asserts the security invariant: risk signals and investigation output are
//! data, never an executed action.

use std::sync::Arc;

use argus_anomaly::{Deviation, DeviationKind, MultiSignalConfig, detect_multi_signal};
use argus_correlate::{CorrelatedEvent, Correlator};
use argus_domain::{ResourceId, Severity};
use argus_incidents::IncidentManager;
use argus_investigate::{
    Evidence, Hypothesis, HypothesisGenerator, HypothesisStatus, InvestigationEngine,
};
use argus_risk::{RiskKind, multi_signal_risk};
use chrono::{Duration, TimeZone, Utc};
use uuid::Uuid;

/// A deterministic hypothesis generator standing in for the structured-decision
/// gateway (the only stochastic step in production).
struct FakeGenerator {
    statements: Vec<(String, f32)>,
}

impl HypothesisGenerator for FakeGenerator {
    fn generate(&self, _evidence: &[Evidence]) -> Vec<Hypothesis> {
        self.statements
            .iter()
            .map(|(statement, confidence)| Hypothesis {
                id: Uuid::new_v4(),
                statement: statement.clone(),
                confidence: *confidence,
                status: HypothesisStatus::Proposed,
                supporting: Vec::new(),
                contradicting: Vec::new(),
            })
            .collect()
    }
}

fn t0() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 30, 12, 0, 0).unwrap()
}

#[test]
fn checkout_api_scenario_produces_the_deployment_root_cause() {
    let key = "app:checkout-api";
    let subject = ResourceId::new("service", "checkout-api").unwrap();

    // 1. Correlation: the six-event chain folds into one situation.
    let mut correlator = Correlator::new(Duration::minutes(10));
    let mut situation_id = None;
    for (i, name) in ["deployment", "pod", "memory", "oom", "latency", "incident"]
        .iter()
        .enumerate()
    {
        situation_id = Some(correlator.ingest(CorrelatedEvent {
            id: Uuid::new_v4(),
            subject: ResourceId::new("service", name).unwrap(),
            correlation_key: key.to_string(),
            timestamp: t0() + Duration::seconds(i as i64 * 20),
        }));
    }
    let situation = correlator.situation(situation_id.unwrap()).unwrap();
    assert_eq!(situation.member_count(), 6);

    // 2. Multi-signal anomaly: CPU and memory deviate together.
    let deviations = vec![
        Deviation {
            signal: "cpu.utilization".to_string(),
            value: 840.0,
            baseline_mean: 100.0,
            z_score: 7.4,
            kind: DeviationKind::BaselineExcursion,
        },
        Deviation {
            signal: "memory.used_percent".to_string(),
            value: 95.0,
            baseline_mean: 50.0,
            z_score: 4.5,
            kind: DeviationKind::BaselineExcursion,
        },
    ];
    let anomaly = detect_multi_signal(&deviations, &MultiSignalConfig::default()).unwrap();
    assert_eq!(anomaly.signals.len(), 2);

    // 3. Advisory risk (data, never authority).
    let risk = multi_signal_risk(&anomaly, subject.clone());
    assert_eq!(risk.kind, RiskKind::Reliability);
    assert_eq!(risk.subject, subject);
    assert_eq!(risk.severity, Severity::Warning);

    // 4. Incident management with deduplication.
    let mut incidents = IncidentManager::new();
    let (incident_id, created) =
        incidents.open(key, Severity::Error, vec![subject.clone()], "anomaly", t0());
    assert!(created);
    let (dedup_id, dedup_created) = incidents.open(key, Severity::Error, vec![], "anomaly", t0());
    assert!(!dedup_created);
    assert_eq!(incident_id, dedup_id);

    // 5. Investigation → root cause.
    let generator = Arc::new(FakeGenerator {
        statements: vec![
            ("deployment introduced a retry-loop change".to_string(), 0.9),
            ("database failure".to_string(), 0.4),
        ],
    });
    let engine = InvestigationEngine::new(generator);
    let evidence = vec![
        Evidence {
            description: "deployment revision 184 occurred 11 minutes before the incident"
                .to_string(),
            supports: vec!["deployment introduced a retry-loop change".to_string()],
            contradicts: vec!["database failure".to_string()],
        },
        Evidence {
            description: "CPU increased 840%".to_string(),
            supports: vec!["deployment introduced a retry-loop change".to_string()],
            contradicts: Vec::new(),
        },
        Evidence {
            description: "database latency increased 3.2x".to_string(),
            supports: vec!["database failure".to_string()],
            contradicts: Vec::new(),
        },
    ];
    let investigation = engine.investigate(incident_id, &evidence);

    // 6. Root cause + the security invariant.
    assert_eq!(
        investigation.root_cause.as_deref(),
        Some("deployment introduced a retry-loop change")
    );
    assert!(investigation.uncertainty.is_empty());
    // The database-failure hypothesis was contradicted by evidence.
    let db = investigation
        .hypotheses
        .iter()
        .find(|h| h.statement == "database failure")
        .unwrap();
    assert_eq!(db.status, HypothesisStatus::Eliminated);
    // The remediation is a *candidate capability id* — data, not an executed
    // action. No executor exists anywhere in the M2 crates by construction.
    assert_eq!(
        investigation.remediation,
        vec!["host.service.restart".to_string()]
    );
}
