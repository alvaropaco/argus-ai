//! Deterministic fake decision provider for tests (Principle 9, 13).

use std::collections::BTreeMap;

use async_trait::async_trait;

use crate::decision::error::DecisionError;
use crate::decision::provider::DecisionProvider;
use crate::decision::types::{DecisionAnswer, DecisionRequest, DecisionResponse};

/// Returns canned answers keyed by question id. Deterministic and offline.
pub struct FakeDecisionProvider {
    answers: BTreeMap<String, DecisionAnswer>,
}

impl FakeDecisionProvider {
    pub fn new(answers: BTreeMap<String, DecisionAnswer>) -> Self {
        Self { answers }
    }
}

#[async_trait]
impl DecisionProvider for FakeDecisionProvider {
    async fn decide(&self, request: DecisionRequest) -> Result<DecisionResponse, DecisionError> {
        let mut answers = BTreeMap::new();
        for (id, question) in &request.questions {
            let answer = self.answers.get(id).ok_or_else(|| {
                DecisionError::Validation(format!(
                    "fake provider has no canned answer for '{id}' ({})",
                    question.kind()
                ))
            })?;
            answers.insert(id.clone(), answer.clone());
        }
        Ok(DecisionResponse {
            model: request.model,
            answers,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::types::{DecisionQuestion, NoulCriteria};

    #[tokio::test]
    async fn fake_returns_canned_answer() {
        let provider = FakeDecisionProvider::new(BTreeMap::from([(
            "q".to_string(),
            DecisionAnswer::Noul { noul: 0.9 },
        )]));

        let request = DecisionRequest {
            model: None,
            state: serde_json::json!({}),
            questions: BTreeMap::from([(
                "q".to_string(),
                DecisionQuestion::noul("?", NoulCriteria::default()),
            )]),
        };

        let response = provider.decide(request).await.unwrap();
        assert!(matches!(
            response.answers.get("q"),
            Some(DecisionAnswer::Noul { noul: 0.9 })
        ));
    }

    #[tokio::test]
    async fn fake_errors_on_missing_answer() {
        let provider = FakeDecisionProvider::new(BTreeMap::new());
        let request = DecisionRequest {
            model: None,
            state: serde_json::json!({}),
            questions: BTreeMap::from([(
                "q".to_string(),
                DecisionQuestion::noul("?", NoulCriteria::default()),
            )]),
        };
        assert!(provider.decide(request).await.is_err());
    }
}
