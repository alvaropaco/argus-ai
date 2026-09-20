//! Validation of decision questions and responses (FR-001).

use crate::decision::error::DecisionError;
use crate::decision::types::{DecisionAnswer, DecisionQuestion, DecisionRequest, DecisionResponse};

/// The JEV contract limits criteria to 2–50 labels/levels.
pub const MIN_CRITERIA: usize = 2;
pub const MAX_CRITERIA: usize = 50;

/// Validates a single question's shape before it is sent.
pub fn validate_question(question: &DecisionQuestion) -> Result<(), DecisionError> {
    match question {
        DecisionQuestion::Choice { criteria, .. } => {
            let n = criteria.len();
            if !(MIN_CRITERIA..=MAX_CRITERIA).contains(&n) {
                return Err(DecisionError::Validation(format!(
                    "choice must have {MIN_CRITERIA}..={MAX_CRITERIA} criteria, got {n}"
                )));
            }
        }
        DecisionQuestion::Score { criteria, .. } => {
            let n = criteria.len();
            if !(MIN_CRITERIA..=MAX_CRITERIA).contains(&n) {
                return Err(DecisionError::Validation(format!(
                    "score must have {MIN_CRITERIA}..={MAX_CRITERIA} levels, got {n}"
                )));
            }
        }
        DecisionQuestion::Noul { .. } => {}
    }
    Ok(())
}

/// Validates that a response is complete and consistent with its request.
pub fn validate_response(
    request: &DecisionRequest,
    response: &DecisionResponse,
) -> Result<(), DecisionError> {
    for (id, question) in &request.questions {
        let answer = response
            .answers
            .get(id)
            .ok_or_else(|| DecisionError::Validation(format!("missing answer for '{id}'")))?;

        if answer.kind() != question.kind() {
            return Err(DecisionError::Validation(format!(
                "answer kind '{}' does not match question kind '{}' for '{id}'",
                answer.kind(),
                question.kind()
            )));
        }

        match (question, answer) {
            (
                DecisionQuestion::Choice { criteria, .. },
                DecisionAnswer::Choice {
                    choice,
                    confidence,
                    probabilities,
                },
            ) => {
                if !criteria.contains_key(choice) {
                    return Err(DecisionError::Validation(format!(
                        "choice '{choice}' not among criteria for '{id}'"
                    )));
                }
                validate_confidence(*confidence, id)?;
                validate_distribution(probabilities, id)?;
            }
            (
                DecisionQuestion::Score { criteria, .. },
                DecisionAnswer::Score {
                    score,
                    confidence,
                    probabilities,
                    ..
                },
            ) => {
                let max = (criteria.len() as f64) - 1.0;
                if *score < 0.0 || *score > max {
                    return Err(DecisionError::Validation(format!(
                        "score {score} out of range [0, {max}] for '{id}'"
                    )));
                }
                validate_confidence(*confidence, id)?;
                validate_distribution(probabilities, id)?;
            }
            (DecisionQuestion::Noul { .. }, DecisionAnswer::Noul { noul }) => {
                if !(0.01..=0.99).contains(noul) {
                    return Err(DecisionError::Validation(format!(
                        "noul {noul} out of range [0.01, 0.99] for '{id}'"
                    )));
                }
            }
            _ => unreachable!("kind mismatch already checked"),
        }
    }

    Ok(())
}

fn validate_confidence(confidence: f64, id: &str) -> Result<(), DecisionError> {
    if !(0.0..=1.0).contains(&confidence) {
        return Err(DecisionError::Validation(format!(
            "confidence {confidence} out of range [0, 1] for '{id}'"
        )));
    }
    Ok(())
}

fn validate_distribution(
    probabilities: &std::collections::BTreeMap<String, f64>,
    id: &str,
) -> Result<(), DecisionError> {
    for (label, p) in probabilities {
        if !(0.0..=1.0).contains(p) {
            return Err(DecisionError::Validation(format!(
                "probability {p} for label '{label}' out of range for '{id}'"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::types::NoulCriteria;
    use std::collections::BTreeMap;

    #[test]
    fn question_rejects_too_few_choice_criteria() {
        let q = DecisionQuestion::choice("pick", BTreeMap::from([("a".into(), None)]));
        assert!(validate_question(&q).is_err());
    }

    #[test]
    fn question_accepts_two_choice_criteria() {
        let q = DecisionQuestion::choice(
            "pick",
            BTreeMap::from([("a".into(), None), ("b".into(), None)]),
        );
        assert!(validate_question(&q).is_ok());
    }

    #[test]
    fn question_accepts_score_levels() {
        let q = DecisionQuestion::score("rate", vec!["Low".into(), "High".into()]);
        assert!(validate_question(&q).is_ok());
    }

    #[test]
    fn response_rejects_missing_answer() {
        let req = DecisionRequest {
            model: None,
            state: serde_json::json!({}),
            questions: BTreeMap::from([(
                "q".into(),
                DecisionQuestion::noul("?", NoulCriteria::default()),
            )]),
        };
        let resp = DecisionResponse {
            model: None,
            answers: BTreeMap::new(),
        };
        assert!(validate_response(&req, &resp).is_err());
    }

    #[test]
    fn response_rejects_choice_not_in_criteria() {
        let req = DecisionRequest {
            model: None,
            state: serde_json::json!({}),
            questions: BTreeMap::from([(
                "q".into(),
                DecisionQuestion::choice(
                    "pick",
                    BTreeMap::from([("a".into(), None), ("b".into(), None)]),
                ),
            )]),
        };
        let resp = DecisionResponse {
            model: None,
            answers: BTreeMap::from([(
                "q".into(),
                DecisionAnswer::Choice {
                    choice: "zzz".into(),
                    confidence: 0.9,
                    probabilities: BTreeMap::new(),
                },
            )]),
        };
        assert!(validate_response(&req, &resp).is_err());
    }

    #[test]
    fn response_accepts_valid_choice() {
        let req = DecisionRequest {
            model: None,
            state: serde_json::json!({}),
            questions: BTreeMap::from([(
                "q".into(),
                DecisionQuestion::choice(
                    "pick",
                    BTreeMap::from([("a".into(), None), ("b".into(), None)]),
                ),
            )]),
        };
        let resp = DecisionResponse {
            model: None,
            answers: BTreeMap::from([(
                "q".into(),
                DecisionAnswer::Choice {
                    choice: "a".into(),
                    confidence: 0.9,
                    probabilities: BTreeMap::new(),
                },
            )]),
        };
        assert!(validate_response(&req, &resp).is_ok());
    }
}
