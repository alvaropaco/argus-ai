//! The reasoning gateway: calls a decision provider, validates, and maps to
//! typed domain entities (FR-003).

use std::sync::Arc;

use argus_domain::{BlastRadius, Plan, PlanStatus, PlanStep};

use crate::decision::error::DecisionError;
use crate::decision::provider::DecisionProvider;
use crate::decision::types::{DecisionRequest, DecisionResponse};
use crate::decision::validate::validate_response;

/// Orchestrates a single reasoning step against a decision engine.
pub struct ReasoningGateway {
    provider: Arc<dyn DecisionProvider>,
    confidence_threshold: f64,
}

impl ReasoningGateway {
    pub fn new(provider: Arc<dyn DecisionProvider>, confidence_threshold: f64) -> Self {
        Self {
            provider,
            confidence_threshold,
        }
    }

    pub fn confidence_threshold(&self) -> f64 {
        self.confidence_threshold
    }

    /// Runs the decision engine and validates the response against the request.
    pub async fn decide(
        &self,
        request: DecisionRequest,
    ) -> Result<DecisionResponse, DecisionError> {
        let response = self.provider.decide(request.clone()).await?;
        validate_response(&request, &response)?;
        Ok(response)
    }

    /// Runs the engine and returns a typed, fail-closed outcome: a decision, or
    /// a no-decision (`noul`) with its reason. Malformed output is still an error.
    pub async fn decide_outcome(
        &self,
        request: DecisionRequest,
    ) -> Result<DecisionOutcome, DecisionError> {
        decide_outcome_with(self.provider.as_ref(), request, self.confidence_threshold).await
    }
}

/// Why the gateway produced no decision (`noul`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoDecisionReason {
    /// The answer cleared validation but not the confidence threshold.
    LowConfidence,
    /// The decision engine could not be reached.
    EngineUnavailable,
}

/// A fail-closed gateway outcome: a validated decision, or a typed no-decision.
#[derive(Debug, Clone, PartialEq)]
pub enum DecisionOutcome {
    Decided(DecisionResponse),
    NoDecision(NoDecisionReason),
}

/// The fail-closed decision rule, independent of any `Arc`.
///
/// An unreachable engine and an answer below the threshold both resolve to a
/// typed `NoDecision`; malformed output is rejected as an error. Callers act
/// only on [`DecisionOutcome::Decided`].
pub async fn decide_outcome_with(
    provider: &dyn DecisionProvider,
    request: DecisionRequest,
    confidence_threshold: f64,
) -> Result<DecisionOutcome, DecisionError> {
    let response = match provider.decide(request.clone()).await {
        Ok(response) => response,
        Err(DecisionError::Unavailable(_)) => {
            return Ok(DecisionOutcome::NoDecision(
                NoDecisionReason::EngineUnavailable,
            ));
        }
        Err(other) => return Err(other),
    };
    validate_response(&request, &response)?;
    if aggregate_confidence(&response) < confidence_threshold {
        return Ok(DecisionOutcome::NoDecision(NoDecisionReason::LowConfidence));
    }
    Ok(DecisionOutcome::Decided(response))
}

/// Aggregate confidence over all answers: the minimum, so a single low-confidence
/// answer drags the whole outcome down (conservative).
pub fn aggregate_confidence(response: &DecisionResponse) -> f64 {
    response
        .answers
        .values()
        .map(|a| a.confidence())
        .fold(1.0, f64::min)
}

/// Builds a proposed `Plan` only if the response clears the confidence threshold.
///
/// The caller supplies the steps derived from the decisions (e.g. a
/// remediation `choice`), each carrying its declarative rollback; this function
/// enforces the confidence gate (FR-003) and produces the typed plan.
pub fn propose_plan(
    response: &DecisionResponse,
    threshold: f64,
    objective: impl Into<String>,
    steps: Vec<PlanStep>,
) -> Option<Plan> {
    let confidence = aggregate_confidence(response);
    if confidence < threshold {
        return None;
    }
    Some(Plan {
        objective: objective.into(),
        steps,
        preconditions: Vec::new(),
        expected_outcomes: Vec::new(),
        blast_radius: BlastRadius::Host,
        confidence,
        status: PlanStatus::Proposed,
        runbook: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use crate::decision::adapters::FakeDecisionProvider;
    use crate::decision::types::{DecisionAnswer, DecisionQuestion, NoulCriteria};

    fn noul_question() -> BTreeMap<String, DecisionQuestion> {
        BTreeMap::from([(
            "degraded".to_string(),
            DecisionQuestion::noul("Is it degraded?", NoulCriteria::default()),
        )])
    }

    fn noul_request() -> DecisionRequest {
        DecisionRequest {
            model: None,
            state: serde_json::json!({}),
            questions: noul_question(),
        }
    }

    struct UnavailableProvider;

    #[async_trait::async_trait]
    impl DecisionProvider for UnavailableProvider {
        async fn decide(
            &self,
            _request: DecisionRequest,
        ) -> Result<DecisionResponse, DecisionError> {
            Err(DecisionError::Unavailable("engine down".into()))
        }
    }

    #[tokio::test]
    async fn confident_response_is_decided() {
        let provider = FakeDecisionProvider::new(BTreeMap::from([(
            "degraded".to_string(),
            DecisionAnswer::Noul { noul: 0.95 },
        )]));
        let outcome = decide_outcome_with(&provider, noul_request(), 0.7)
            .await
            .unwrap();
        assert!(matches!(outcome, DecisionOutcome::Decided(_)));
    }

    #[tokio::test]
    async fn low_confidence_is_a_no_decision() {
        let provider = FakeDecisionProvider::new(BTreeMap::from([(
            "degraded".to_string(),
            DecisionAnswer::Noul { noul: 0.3 },
        )]));
        let outcome = decide_outcome_with(&provider, noul_request(), 0.7)
            .await
            .unwrap();
        assert_eq!(
            outcome,
            DecisionOutcome::NoDecision(NoDecisionReason::LowConfidence)
        );
    }

    #[tokio::test]
    async fn unavailable_engine_is_a_no_decision() {
        let outcome = decide_outcome_with(&UnavailableProvider, noul_request(), 0.7)
            .await
            .unwrap();
        assert_eq!(
            outcome,
            DecisionOutcome::NoDecision(NoDecisionReason::EngineUnavailable)
        );
    }

    #[tokio::test]
    async fn malformed_response_is_rejected_by_the_outcome() {
        let provider = FakeDecisionProvider::new(BTreeMap::from([(
            "degraded".to_string(),
            DecisionAnswer::Choice {
                choice: "x".into(),
                confidence: 0.9,
                probabilities: BTreeMap::new(),
            },
        )]));
        assert!(matches!(
            decide_outcome_with(&provider, noul_request(), 0.7).await,
            Err(DecisionError::Validation(_))
        ));
    }

    #[tokio::test]
    async fn gateway_validates_and_returns_response() {
        let provider = FakeDecisionProvider::new(BTreeMap::from([(
            "degraded".to_string(),
            DecisionAnswer::Noul { noul: 0.9 },
        )]));
        let gateway = ReasoningGateway::new(Arc::new(provider), 0.7);

        let request = DecisionRequest {
            model: None,
            state: serde_json::json!({}),
            questions: noul_question(),
        };
        let response = gateway.decide(request).await.unwrap();
        assert_eq!(aggregate_confidence(&response), 0.9);
    }

    #[tokio::test]
    async fn gateway_rejects_invalid_response() {
        // Fake returns an answer whose kind mismatches the question (choice vs noul).
        let provider = FakeDecisionProvider::new(BTreeMap::from([(
            "degraded".to_string(),
            DecisionAnswer::Choice {
                choice: "x".into(),
                confidence: 0.9,
                probabilities: BTreeMap::new(),
            },
        )]));
        let gateway = ReasoningGateway::new(Arc::new(provider), 0.7);

        let request = DecisionRequest {
            model: None,
            state: serde_json::json!({}),
            questions: noul_question(),
        };
        assert!(gateway.decide(request).await.is_err());
    }

    #[test]
    fn propose_plan_gates_on_confidence() {
        let high = DecisionResponse {
            model: None,
            usage: None,
            answers: BTreeMap::from([("q".to_string(), DecisionAnswer::Noul { noul: 0.95 })]),
        };
        let action = argus_domain::Action {
            capability: argus_domain::CapabilityId::new("host.service.restart").unwrap(),
            resource: None,
            arguments: serde_json::json!({ "unit": "nginx.service" }),
        };
        let step = PlanStep {
            action: action.clone(),
            rollback: None,
        };
        let plan = propose_plan(&high, 0.8, "restore nginx", vec![step.clone()]);
        assert!(plan.is_some());
        assert_eq!(plan.unwrap().status, PlanStatus::Proposed);

        let low = DecisionResponse {
            model: None,
            usage: None,
            answers: BTreeMap::from([("q".to_string(), DecisionAnswer::Noul { noul: 0.5 })]),
        };
        assert!(propose_plan(&low, 0.8, "restore nginx", vec![step]).is_none());
    }
}
