//! Laya (`laya-serve`) HTTP adapter.
//!
//! Laya is the System-1 decision engine. Its `/predict` request body matches
//! [`DecisionRequest`] (`state`, `questions`, `model?`); only the response shape
//! is Laya-specific: `answers`, `probabilities`, and `confidence` are
//! per-question maps, and `routing.model` names the checkpoint that answered.

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;

use crate::decision::error::DecisionError;
use crate::decision::provider::DecisionProvider;
use crate::decision::types::{DecisionAnswer, DecisionRequest, DecisionResponse};

/// Laya's `noul` answers are probabilities; validation requires `[0.01, 0.99]`.
const NOUL_MIN: f64 = 0.01;
const NOUL_MAX: f64 = 0.99;

/// A decision engine reached over the Laya `/predict` protocol.
pub struct LayaHttpProvider {
    client: Client,
    base_url: String,
}

impl LayaHttpProvider {
    /// `base_url` is the `laya-serve` root (e.g. `http://127.0.0.1:8000`).
    pub fn new(base_url: impl Into<String>) -> Self {
        Self::with_timeout(base_url, Duration::from_secs(30))
    }

    /// Like [`LayaHttpProvider::new`] with an explicit request timeout, so a
    /// hung sidecar cannot stall the control loop indefinitely.
    pub fn with_timeout(base_url: impl Into<String>, timeout: Duration) -> Self {
        let client = Client::builder()
            .timeout(timeout)
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            client,
            base_url: base_url.into().trim_end_matches('/').to_string(),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }
}

#[async_trait]
impl DecisionProvider for LayaHttpProvider {
    async fn decide(&self, request: DecisionRequest) -> Result<DecisionResponse, DecisionError> {
        let url = format!("{}/predict", self.base_url);
        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| DecisionError::Unavailable(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            return Err(DecisionError::Unavailable(format!(
                "laya returned {status}"
            )));
        }

        let body: Value = response
            .json()
            .await
            .map_err(|e| DecisionError::Invalid(e.to_string()))?;
        map_response(&body)
    }
}

/// Maps a `laya-serve` `/predict` response onto the native [`DecisionResponse`].
fn map_response(body: &Value) -> Result<DecisionResponse, DecisionError> {
    let answers = body
        .get("answers")
        .and_then(Value::as_object)
        .ok_or_else(|| DecisionError::Invalid("missing `answers`".into()))?;
    let confidence = body.get("confidence").and_then(Value::as_object);
    let probabilities = body.get("probabilities").and_then(Value::as_object);

    let mut mapped = BTreeMap::new();
    for (id, answer) in answers {
        let conf = confidence
            .and_then(|c| c.get(id))
            .and_then(Value::as_f64)
            .ok_or_else(|| DecisionError::Validation(format!("missing confidence for '{id}'")))?;
        let probs: BTreeMap<String, f64> = probabilities
            .and_then(|p| p.get(id))
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| v.as_f64().map(|v| (k.clone(), v)))
                    .collect()
            })
            .unwrap_or_default();

        let has_choice = answer.get("choice").and_then(Value::as_str);
        let has_score = answer.get("score").and_then(Value::as_f64);
        let has_noul = answer.get("noul").and_then(Value::as_f64);
        if [
            has_choice.is_some(),
            has_score.is_some(),
            has_noul.is_some(),
        ]
        .into_iter()
        .filter(|present| *present)
        .count()
            != 1
        {
            return Err(DecisionError::Invalid(format!(
                "answer for '{id}' must carry exactly one of choice/score/noul"
            )));
        }

        let answer = if let Some(choice) = has_choice {
            DecisionAnswer::Choice {
                choice: choice.to_string(),
                confidence: conf,
                probabilities: probs,
            }
        } else if let Some(score) = has_score {
            DecisionAnswer::Score {
                score,
                confidence: conf,
                probabilities: probs,
                legend: BTreeMap::new(),
            }
        } else {
            DecisionAnswer::Noul {
                noul: has_noul
                    .expect("checked exactly one")
                    .clamp(NOUL_MIN, NOUL_MAX),
            }
        };
        mapped.insert(id.clone(), answer);
    }

    let model = body
        .get("routing")
        .and_then(|r| r.get("model"))
        .and_then(Value::as_str)
        .map(str::to_string);

    Ok(DecisionResponse {
        model,
        usage: None,
        answers: mapped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn base_url_trims_trailing_slash() {
        assert_eq!(
            LayaHttpProvider::new("http://127.0.0.1:8000/").base_url(),
            "http://127.0.0.1:8000"
        );
    }

    #[test]
    fn maps_choice_score_and_noul() {
        let body = json!({
            "answers": {
                "remediation": {"choice": "restart"},
                "urgency": {"score": 2},
                "degraded": {"noul": 0.8}
            },
            "probabilities": {"remediation": {"restart": 0.88, "noop": 0.12}},
            "confidence": {"remediation": 0.88, "urgency": 0.7, "degraded": 0.8},
            "routing": {"model": "english"}
        });
        let resp = map_response(&body).expect("maps");
        assert_eq!(resp.model.as_deref(), Some("english"));
        match &resp.answers["remediation"] {
            DecisionAnswer::Choice {
                choice,
                confidence,
                probabilities,
            } => {
                assert_eq!(choice, "restart");
                assert_eq!(*confidence, 0.88);
                assert_eq!(probabilities["restart"], 0.88);
            }
            other => panic!("expected choice, got {other:?}"),
        }
        assert!(matches!(
            resp.answers["degraded"],
            DecisionAnswer::Noul { .. }
        ));
    }

    #[test]
    fn missing_confidence_is_rejected() {
        let body = json!({"answers": {"remediation": {"choice": "restart"}}});
        assert!(matches!(
            map_response(&body),
            Err(DecisionError::Validation(_))
        ));
    }

    #[test]
    fn ambiguous_answer_is_rejected() {
        let body = json!({
            "answers": {"q": {"choice": "restart", "noul": 0.8}},
            "confidence": {"q": 0.8}
        });
        assert!(matches!(
            map_response(&body),
            Err(DecisionError::Invalid(_))
        ));
    }

    #[tokio::test]
    async fn sidecar_down_is_unavailable() {
        // Nothing listens on port 1, so the connection is refused.
        let provider = LayaHttpProvider::with_timeout("http://127.0.0.1:1", Duration::from_secs(2));
        let request = DecisionRequest {
            model: None,
            state: json!({}),
            questions: BTreeMap::new(),
        };
        let error = provider.decide(request).await.expect_err("refused");
        assert!(matches!(error, DecisionError::Unavailable(_)));
    }

    #[test]
    fn noul_is_clamped_into_range() {
        let body = json!({
            "answers": {"degraded": {"noul": 1.0}},
            "confidence": {"degraded": 1.0}
        });
        match &map_response(&body).expect("maps").answers["degraded"] {
            DecisionAnswer::Noul { noul } => assert_eq!(*noul, 0.99),
            other => panic!("expected noul, got {other:?}"),
        }
    }
}
