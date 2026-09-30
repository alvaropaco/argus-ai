//! ARGUS investigation engine: hypothesis-driven root-cause analysis
//! (CAP-8/9, ADR-0034).
//!
//! The engine is a deterministic state machine: it generates candidate
//! hypotheses through a [`HypothesisGenerator`] (the *only* stochastic step —
//! the existing structured-decision gateway), then deterministically tests,
//! eliminates, and concludes. It never fabricates certainty: when evidence is
//! insufficient, it reports the unresolved uncertainty.

use std::sync::Arc;

use uuid::Uuid;

/// The lifecycle of a hypothesis under investigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HypothesisStatus {
    Proposed,
    Supported,
    Contradicted,
    Eliminated,
    Confirmed,
}

/// A candidate explanation, with the evidence for and against it.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::derive_partial_eq_without_eq)] // confidence is f32
pub struct Hypothesis {
    pub id: Uuid,
    pub statement: String,
    pub confidence: f32,
    pub status: HypothesisStatus,
    pub supporting: Vec<String>,
    pub contradicting: Vec<String>,
}

/// A piece of evidence, declaring which hypothesis statements it supports or
/// contradicts. Statements are matched by value (deterministic).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evidence {
    pub description: String,
    pub supports: Vec<String>,
    pub contradicts: Vec<String>,
}

/// Produces candidate hypotheses from evidence. The production implementation
/// is the structured-decision gateway; a deterministic fake is used in tests.
pub trait HypothesisGenerator: Send + Sync {
    fn generate(&self, evidence: &[Evidence]) -> Vec<Hypothesis>;
}

/// A completed investigation.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::derive_partial_eq_without_eq)] // Hypothesis has f32
pub struct Investigation {
    pub incident_id: Uuid,
    pub hypotheses: Vec<Hypothesis>,
    pub root_cause: Option<String>,
    pub uncertainty: Vec<String>,
    /// Candidate remediation capability ids. These are data: they still cross
    /// policy and the typed executor before anything executes.
    pub remediation: Vec<String>,
}

/// The deterministic investigation engine.
pub struct InvestigationEngine {
    generator: Arc<dyn HypothesisGenerator>,
    min_confidence: f32,
}

impl InvestigationEngine {
    pub fn new(generator: Arc<dyn HypothesisGenerator>) -> Self {
        Self {
            generator,
            min_confidence: 0.5,
        }
    }

    pub fn with_min_confidence(mut self, min_confidence: f32) -> Self {
        self.min_confidence = min_confidence;
        self
    }

    /// Run one investigation: generate → test → eliminate → conclude.
    pub fn investigate(&self, incident_id: Uuid, evidence: &[Evidence]) -> Investigation {
        // 1. Generate candidate hypotheses (the stochastic step).
        let mut hypotheses = self.generator.generate(evidence);

        // 2. Deterministic testing: annotate each hypothesis with the evidence
        //    that supports or contradicts it.
        for hypothesis in &mut hypotheses {
            let mut supporting = Vec::new();
            let mut contradicting = Vec::new();
            for item in evidence {
                if item.supports.iter().any(|s| s == &hypothesis.statement) {
                    supporting.push(item.description.clone());
                }
                if item.contradicts.iter().any(|s| s == &hypothesis.statement) {
                    contradicting.push(item.description.clone());
                }
            }
            hypothesis.supporting = supporting;
            hypothesis.contradicting = contradicting;
            hypothesis.status = if !hypothesis.contradicting.is_empty() {
                HypothesisStatus::Eliminated
            } else if !hypothesis.supporting.is_empty() {
                HypothesisStatus::Supported
            } else {
                HypothesisStatus::Proposed
            };
        }

        // 3. Conclude: the strongest non-eliminated hypothesis above the
        //    confidence threshold becomes the root cause; otherwise report
        //    uncertainty.
        let best = hypotheses
            .iter()
            .filter(|h| h.status != HypothesisStatus::Eliminated)
            .max_by(|a, b| {
                a.confidence
                    .partial_cmp(&b.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });

        let mut root_cause = None;
        let mut uncertainty = Vec::new();
        if let Some(best) = best {
            if best.confidence >= self.min_confidence {
                root_cause = Some(best.statement.clone());
            } else {
                uncertainty.push(format!(
                    "best hypothesis '{}' has confidence {:.2} below threshold {:.2}",
                    best.statement, best.confidence, self.min_confidence
                ));
            }
        } else {
            uncertainty.push("all hypotheses were eliminated by the evidence".to_string());
        }

        let remediation = if root_cause.is_some() {
            // A deterministic placeholder: the real remediation is a typed Plan
            // authored by the planner and routed through policy (M3).
            vec!["host.service.restart".to_string()]
        } else {
            Vec::new()
        };

        Investigation {
            incident_id,
            hypotheses,
            root_cause,
            uncertainty,
            remediation,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic generator for tests.
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

    fn evidence() -> Vec<Evidence> {
        vec![
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
        ]
    }

    #[test]
    fn concludes_the_best_supported_hypothesis() {
        let generator = Arc::new(FakeGenerator {
            statements: vec![
                ("deployment introduced a retry-loop change".to_string(), 0.9),
                ("database failure".to_string(), 0.4),
            ],
        });
        let engine = InvestigationEngine::new(generator);
        let investigation = engine.investigate(Uuid::new_v4(), &evidence());

        assert_eq!(
            investigation.root_cause.as_deref(),
            Some("deployment introduced a retry-loop change")
        );
        assert!(investigation.uncertainty.is_empty());
        // The database-failure hypothesis is contradicted by evidence.
        let db = investigation
            .hypotheses
            .iter()
            .find(|h| h.statement == "database failure")
            .unwrap();
        assert_eq!(db.status, HypothesisStatus::Eliminated);
        assert!(!db.contradicting.is_empty());
    }

    #[test]
    fn reports_uncertainty_when_below_threshold() {
        let generator = Arc::new(FakeGenerator {
            statements: vec![("a weak hypothesis".to_string(), 0.2)],
        });
        let engine = InvestigationEngine::new(generator);
        let investigation = engine.investigate(Uuid::new_v4(), &[]);
        assert!(investigation.root_cause.is_none());
        assert_eq!(investigation.uncertainty.len(), 1);
    }

    #[test]
    fn reports_uncertainty_when_all_eliminated() {
        let generator = Arc::new(FakeGenerator {
            statements: vec![("database failure".to_string(), 0.9)],
        });
        let engine = InvestigationEngine::new(generator);
        let investigation = engine.investigate(Uuid::new_v4(), &evidence());
        assert!(investigation.root_cause.is_none());
        assert!(!investigation.uncertainty.is_empty());
    }
}
