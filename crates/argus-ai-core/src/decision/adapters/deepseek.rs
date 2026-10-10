//! DeepSeek (OpenAI-compatible) chat-completions adapter (spec 005 FR-001).
//!
//! The model answers the **typed decision contract**, not a free-form
//! prompt: the request (`state` + `questions`) is serialized into the user
//! message, JSON mode is requested, and the reply is parsed into typed
//! [`DecisionAnswer`]s with fail-closed validation — every requested
//! question must be answered, a `choice` must be among the offered
//! criteria, a `noul` must lie in `[0.01, 0.99]`, and any deviation is a
//! `DecisionError::Validation`, never an invented answer. Model text can
//! therefore never become an action; it can only answer questions that
//! `host_health` maps onto a typed `Action` afterwards.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{Value, json};

use crate::decision::error::DecisionError;
use crate::decision::provider::DecisionProvider;
use crate::decision::types::{DecisionAnswer, DecisionRequest, DecisionResponse};

const NOUL_MIN: f64 = 0.01;
const NOUL_MAX: f64 = 0.99;

/// A structured-decision engine backed by a DeepSeek/OpenAI-compatible
/// chat-completions endpoint.
pub struct DeepSeekProvider {
    client: Client,
    api_base: String,
    api_key: String,
    /// The primary model, then fallbacks tried in order on transport/5xx.
    models: Vec<String>,
}

impl DeepSeekProvider {
    /// `api_base` is the API root (e.g. `https://api.deepseek.com`); the
    /// chat-completions path is appended. `models` is primary-first; an
    /// empty list falls back to `deepseek-chat`.
    pub fn new(
        api_base: impl Into<String>,
        api_key: impl Into<String>,
        models: Vec<String>,
    ) -> Self {
        Self::with_timeout(api_base, api_key, models, Duration::from_secs(45))
    }

    pub fn with_timeout(
        api_base: impl Into<String>,
        api_key: impl Into<String>,
        models: Vec<String>,
        timeout: Duration,
    ) -> Self {
        Self {
            client: Client::builder()
                .timeout(timeout)
                .build()
                .unwrap_or_else(|_| Client::new()),
            api_base: api_base.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            models: if models.is_empty() {
                vec!["deepseek-chat".to_string()]
            } else {
                models
            },
        }
    }

    /// One attempt against one model.
    async fn attempt(
        &self,
        model: &str,
        request: &DecisionRequest,
    ) -> Result<DecisionResponse, DecisionError> {
        let system = "You are a structured decision engine. You will receive a JSON object \
with `state` (evidence) and `questions` (typed questions). Answer EVERY question \
in a single JSON object keyed by question id. For a `choice` question, answer \
{\"choice\": <one of the offered labels>, \"probabilities\": {label: 0..1 for \
every offered label}}. For a `noul` question, answer {\"noul\": <0.01..0.99>}. \
For a `score` question, answer {\"score\": <number>, \"legend\": {level: \
description}}. Output ONLY the JSON object.";
        let user =
            serde_json::to_string(&request).map_err(|e| DecisionError::Invalid(e.to_string()))?;

        let body = json!({
            "model": model,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user },
            ],
            "response_format": { "type": "json_object" },
            "temperature": 0.0,
            "max_tokens": 1024,
        });
        let url = format!("{}/chat/completions", self.api_base);
        let response = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| DecisionError::Unavailable(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let detail = response
                .text()
                .await
                .unwrap_or_default()
                .chars()
                .take(300)
                .collect::<String>();
            // Never include headers; the body text of an error page is safe
            // to surface truncated, and it carries no credential material.
            return Err(DecisionError::Unavailable(format!(
                "provider returned {status}: {detail}"
            )));
        }

        let payload: Value = response
            .json()
            .await
            .map_err(|e| DecisionError::Invalid(e.to_string()))?;
        let content = payload
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                DecisionError::Invalid("response lacks choices[0].message.content".into())
            })?;
        let answers: Value = serde_json::from_str(content)
            .map_err(|e| DecisionError::Invalid(format!("model output is not JSON: {e}")))?;
        let mapped = map_answers(&answers, request)?;
        // The usage block is metering, not authority: parse it when present,
        // leave it unknown when absent — never zero, never estimated (FR-003).
        let usage = parse_usage(payload.get("usage"));
        Ok(DecisionResponse {
            model: Some(model.to_string()),
            answers: mapped,
            usage,
        })
    }
}

/// Parses an OpenAI-compatible `usage` object. A missing or empty block is
/// `None` — the caller records the call's usage as unknown (spec 007 FR-003).
pub(crate) fn parse_usage(block: Option<&Value>) -> Option<crate::decision::types::TokenUsage> {
    let block = block?;
    let prompt_tokens = block.get("prompt_tokens").and_then(Value::as_u64);
    let completion_tokens = block.get("completion_tokens").and_then(Value::as_u64);
    let total_tokens = block.get("total_tokens").and_then(Value::as_u64);
    if prompt_tokens.is_none() && completion_tokens.is_none() && total_tokens.is_none() {
        return None;
    }
    Some(crate::decision::types::TokenUsage {
        prompt_tokens,
        completion_tokens,
        total_tokens,
    })
}

#[async_trait]
impl DecisionProvider for DeepSeekProvider {
    async fn decide(&self, request: DecisionRequest) -> Result<DecisionResponse, DecisionError> {
        // Primary first, then fallbacks — only transport/server failures
        // fall through; a validated answer or a validation error is final.
        let mut last: Option<DecisionError> = None;
        for model in &self.models {
            match self.attempt(model, &request).await {
                Ok(response) => return Ok(response),
                Err(e @ DecisionError::Unavailable(_)) => {
                    tracing::warn!(model = %model, error = %e, "decision model unavailable; trying next");
                    last = Some(e);
                }
                Err(e) => return Err(e),
            }
        }
        Err(last.unwrap_or_else(|| DecisionError::Unavailable("no models configured".into())))
    }
}

/// Validate the model's JSON answers against the request, fail-closed.
pub(crate) fn map_answers(
    answers: &Value,
    request: &DecisionRequest,
) -> Result<std::collections::BTreeMap<String, DecisionAnswer>, DecisionError> {
    let object = answers
        .as_object()
        .ok_or_else(|| DecisionError::Invalid("answers must be a JSON object".into()))?;
    let mut mapped = std::collections::BTreeMap::new();
    for (id, question) in &request.questions {
        let answer = object
            .get(id)
            .ok_or_else(|| DecisionError::Validation(format!("no answer for question '{id}'")))?;
        let parsed = match question {
            crate::decision::types::DecisionQuestion::Choice { criteria, .. } => {
                let choice = answer
                    .get("choice")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        DecisionError::Validation(format!("'{id}' lacks a string choice"))
                    })?;
                if !criteria.contains_key(choice) {
                    return Err(DecisionError::Validation(format!(
                        "'{id}' chose '{choice}', which is not among the offered labels"
                    )));
                }
                let probabilities = probabilities_of(answer, criteria.keys().map(String::as_str))?;
                let confidence = probabilities.get(choice).copied().unwrap_or(0.0);
                DecisionAnswer::Choice {
                    choice: choice.to_string(),
                    confidence,
                    probabilities,
                }
            }
            crate::decision::types::DecisionQuestion::Score { criteria, .. } => {
                let score = answer.get("score").and_then(Value::as_f64).ok_or_else(|| {
                    DecisionError::Validation(format!("'{id}' lacks a numeric score"))
                })?;
                let levels = criteria.len().max(1) as f64;
                let score = score.clamp(0.0, levels - 1.0);
                let legend: std::collections::BTreeMap<String, String> = answer
                    .get("legend")
                    .and_then(Value::as_object)
                    .map(|m| {
                        m.iter()
                            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                            .collect()
                    })
                    .unwrap_or_default();
                let probabilities = probabilities_of(answer, criteria.iter().map(|s| s.as_str()))?;
                let confidence = probabilities.values().cloned().fold(0.0, f64::max);
                DecisionAnswer::Score {
                    score,
                    confidence,
                    probabilities,
                    legend,
                }
            }
            crate::decision::types::DecisionQuestion::Noul { .. } => {
                let noul = answer.get("noul").and_then(Value::as_f64).ok_or_else(|| {
                    DecisionError::Validation(format!("'{id}' lacks a numeric noul"))
                })?;
                if !(NOUL_MIN..=NOUL_MAX).contains(&noul) {
                    return Err(DecisionError::Validation(format!(
                        "'{id}' noul {noul} outside [{NOUL_MIN}, {NOUL_MAX}]"
                    )));
                }
                DecisionAnswer::Noul { noul }
            }
        };
        mapped.insert(id.clone(), parsed);
    }
    Ok(mapped)
}

/// Extract a probability map restricted to the offered labels; extra labels
/// are rejected (they would smuggle unasked options).
fn probabilities_of<'a, I: Iterator<Item = &'a str>>(
    answer: &Value,
    labels: I,
) -> Result<std::collections::BTreeMap<String, f64>, DecisionError> {
    let offered: Vec<&str> = labels.collect();
    let empty = serde_json::Map::new();
    let given = answer
        .get("probabilities")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    for key in given.keys() {
        if !offered.contains(&key.as_str()) {
            return Err(DecisionError::Validation(format!(
                "probabilities carry unoffered label '{key}'"
            )));
        }
    }
    offered
        .into_iter()
        .map(|label| {
            let p = given
                .get(label)
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
                .clamp(0.0, 1.0);
            Ok((label.to_string(), p))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::types::{DecisionQuestion, NoulCriteria};
    use std::collections::BTreeMap;

    fn request() -> DecisionRequest {
        let mut questions = BTreeMap::new();
        questions.insert(
            "degraded".to_string(),
            DecisionQuestion::noul(
                "Is the service degraded?",
                NoulCriteria {
                    yes: Some("down".into()),
                    no: Some("healthy".into()),
                },
            ),
        );
        questions.insert(
            "remediation".to_string(),
            DecisionQuestion::choice(
                "Which remediation fits?",
                BTreeMap::from([("restart".to_string(), None), ("noop".to_string(), None)]),
            ),
        );
        DecisionRequest {
            model: None,
            state: serde_json::json!({ "unit": "nginx.service" }),
            questions,
        }
    }

    fn valid_answers() -> Value {
        json!({
            "degraded": { "noul": 0.93 },
            "remediation": {
                "choice": "restart",
                "probabilities": { "restart": 0.88, "noop": 0.12 }
            }
        })
    }

    #[test]
    fn valid_model_output_maps_to_typed_answers() {
        let answers = map_answers(&valid_answers(), &request()).unwrap();
        assert!(matches!(
            answers.get("degraded"),
            Some(DecisionAnswer::Noul { noul }) if (*noul - 0.93).abs() < 1e-9
        ));
        let Some(DecisionAnswer::Choice {
            choice, confidence, ..
        }) = answers.get("remediation")
        else {
            panic!("choice answer");
        };
        assert_eq!(choice, "restart");
        assert!((confidence - 0.88).abs() < 1e-9);
    }

    #[test]
    fn a_missing_question_fails_closed() {
        let bad = json!({ "degraded": { "noul": 0.5 } });
        assert!(map_answers(&bad, &request()).is_err());
    }

    #[test]
    fn an_unoffered_choice_fails_closed() {
        let bad = json!({
            "degraded": { "noul": 0.5 },
            "remediation": { "choice": "rm -rf /", "probabilities": {} }
        });
        let err = map_answers(&bad, &request()).unwrap_err();
        assert!(err.to_string().contains("not among the offered labels"));
    }

    #[test]
    fn an_out_of_range_noul_fails_closed() {
        let bad = json!({
            "degraded": { "noul": 1.0 },
            "remediation": { "choice": "noop", "probabilities": { "noop": 1.0 } }
        });
        assert!(map_answers(&bad, &request()).is_err());
    }

    #[test]
    fn smuggled_probability_labels_are_rejected() {
        let bad = json!({
            "degraded": { "noul": 0.5 },
            "remediation": {
                "choice": "restart",
                "probabilities": { "restart": 0.5, "shell": 0.5 }
            }
        });
        assert!(map_answers(&bad, &request()).is_err());
    }

    #[test]
    fn the_usage_block_is_parsed_when_present_and_unknown_when_absent() {
        let payload = json!({
            "choices": [],
            "usage": { "prompt_tokens": 120, "completion_tokens": 45, "total_tokens": 165 }
        });
        let usage = parse_usage(payload.get("usage")).expect("usage present");
        assert_eq!(usage.prompt_tokens, Some(120));
        assert_eq!(usage.completion_tokens, Some(45));
        assert_eq!(usage.total_tokens, Some(165));

        // DeepSeek may omit the block entirely: unknown, never zero (FR-003).
        let bare = json!({ "choices": [] });
        assert!(parse_usage(bare.get("usage")).is_none());
        let empty = json!({ "usage": {} });
        assert!(parse_usage(empty.get("usage")).is_none());
    }

    #[test]
    fn missing_probabilities_default_to_zero_confidence() {
        // The answer is still valid (choice + noul present); confidence
        // simply reads as uncalibrated zero, never invented.
        let answers = json!({
            "degraded": { "noul": 0.5 },
            "remediation": { "choice": "noop" }
        });
        let mapped = map_answers(&answers, &request()).unwrap();
        let Some(DecisionAnswer::Choice { confidence, .. }) = mapped.get("remediation") else {
            panic!()
        };
        assert_eq!(*confidence, 0.0);
    }

    /// A live-API smoke test: `ARGUS_DEEPSEEK_KEY=... cargo test -p
    /// argus-ai-core -- --ignored deepseek` (run on the VPS during release).
    #[tokio::test]
    #[ignore = "requires ARGUS_DEEPSEEK_KEY"]
    async fn deepseek_live_answers_the_contract() {
        let key = std::env::var("ARGUS_DEEPSEEK_KEY").unwrap();
        let provider = DeepSeekProvider::new(
            "https://api.deepseek.com",
            key,
            vec!["deepseek-chat".into()],
        );
        let response = provider.decide(request()).await.expect("live decision");
        let Some(DecisionAnswer::Choice { choice, .. }) = response.answers.get("remediation")
        else {
            panic!("expected a choice answer");
        };
        assert!(choice == "restart" || choice == "noop");
        assert!(
            response
                .model
                .as_deref()
                .is_some_and(|m| m.contains("deepseek"))
        );
    }
}
