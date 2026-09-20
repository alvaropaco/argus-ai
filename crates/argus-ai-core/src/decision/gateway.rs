//! The reasoning gateway: calls a decision provider, validates, and maps to
//! typed domain entities (FR-003).

use std::sync::Arc;

use argus_domain::{Action, BlastRadius, Plan, PlanStatus};

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
/// The caller supplies the actions derived from the decisions (e.g. a
/// remediation `choice`); this function enforces the confidence gate (FR-003)
/// and produces the typed plan.
pub fn propose_plan(
    response: &DecisionResponse,
    threshold: f64,
    objective: impl Into<String>,
    actions: Vec<Action>,
) -> Option<Plan> {
    let confidence = aggregate_confidence(response);
    if confidence < threshold {
        return None;
    }
    Some(Plan {
        objective: objective.into(),
        actions,
        preconditions: Vec::new(),
        expected_outcomes: Vec::new(),
        rollback: None,
        blast_radius: BlastRadius::Host,
        confidence,
        status: PlanStatus::Proposed,
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
            answers: BTreeMap::from([("q".to_string(), DecisionAnswer::Noul { noul: 0.95 })]),
        };
        let action = Action {
            capability: argus_domain::CapabilityId::new("host.service.restart").unwrap(),
            resource: None,
            arguments: serde_json::json!({ "unit": "nginx.service" }),
        };
        let plan = propose_plan(&high, 0.8, "restore nginx", vec![action.clone()]);
        assert!(plan.is_some());
        assert_eq!(plan.unwrap().status, PlanStatus::Proposed);

        let low = DecisionResponse {
            model: None,
            answers: BTreeMap::from([("q".to_string(), DecisionAnswer::Noul { noul: 0.5 })]),
        };
        assert!(propose_plan(&low, 0.8, "restore nginx", vec![action]).is_none());
    }
}
