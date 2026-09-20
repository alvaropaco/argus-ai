//! Native structured-decision types (JEV "System One" `choice`/`score`/`noul`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Optional `true`/`false` descriptions for a [`DecisionQuestion::Noul`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoulCriteria {
    #[serde(rename = "true", skip_serializing_if = "Option::is_none")]
    pub yes: Option<String>,
    #[serde(rename = "false", skip_serializing_if = "Option::is_none")]
    pub no: Option<String>,
}

/// A structured question posed to a decision engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DecisionQuestion {
    /// Pick one of 2–50 candidate labels; each label may carry a description.
    Choice {
        instructions: String,
        criteria: BTreeMap<String, Option<String>>,
    },
    /// Order over 2–50 rubric levels, lowest → highest.
    Score {
        instructions: String,
        criteria: Vec<String>,
    },
    /// Truth/support judgment (0.01–0.99).
    Noul {
        instructions: String,
        criteria: NoulCriteria,
    },
}

impl DecisionQuestion {
    pub fn choice(
        instructions: impl Into<String>,
        criteria: BTreeMap<String, Option<String>>,
    ) -> Self {
        Self::Choice {
            instructions: instructions.into(),
            criteria,
        }
    }

    pub fn score(instructions: impl Into<String>, levels: Vec<String>) -> Self {
        Self::Score {
            instructions: instructions.into(),
            criteria: levels,
        }
    }

    pub fn noul(instructions: impl Into<String>, criteria: NoulCriteria) -> Self {
        Self::Noul {
            instructions: instructions.into(),
            criteria,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Choice { .. } => "choice",
            Self::Score { .. } => "score",
            Self::Noul { .. } => "noul",
        }
    }
}

/// A scored answer returned for a question.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DecisionAnswer {
    Choice {
        choice: String,
        confidence: f64,
        probabilities: BTreeMap<String, f64>,
    },
    Score {
        score: f64,
        confidence: f64,
        probabilities: BTreeMap<String, f64>,
        legend: BTreeMap<String, String>,
    },
    Noul {
        noul: f64,
    },
}

impl DecisionAnswer {
    /// The largest label probability (or the `noul` value). Uncalibrated.
    pub fn confidence(&self) -> f64 {
        match self {
            Self::Choice { confidence, .. } | Self::Score { confidence, .. } => *confidence,
            Self::Noul { noul } => *noul,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Choice { .. } => "choice",
            Self::Score { .. } => "score",
            Self::Noul { .. } => "noul",
        }
    }
}

/// A full decision request: a shared context plus a set of questions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub state: Value,
    pub questions: BTreeMap<String, DecisionQuestion>,
}

/// A full decision response: one answer per requested question.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub answers: BTreeMap<String, DecisionAnswer>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example_request() -> DecisionRequest {
        let mut questions = BTreeMap::new();
        questions.insert(
            "degraded".to_string(),
            DecisionQuestion::noul(
                "Is the service degraded?",
                NoulCriteria {
                    yes: Some("service is down or unhealthy".into()),
                    no: Some("service is healthy".into()),
                },
            ),
        );
        questions.insert(
            "remediation".to_string(),
            DecisionQuestion::choice(
                "Which remediation fits?",
                [("restart".to_string(), None), ("noop".to_string(), None)].into(),
            ),
        );
        DecisionRequest {
            model: Some("test-model".into()),
            state: serde_json::json!({ "unit": "nginx.service", "state": "failed" }),
            questions,
        }
    }

    #[test]
    fn request_wire_format_matches_contract() {
        let req = example_request();
        let json = serde_json::to_value(&req).unwrap();

        assert_eq!(json["state"]["unit"], "nginx.service");
        assert_eq!(json["questions"]["degraded"]["type"], "noul");
        assert_eq!(
            json["questions"]["degraded"]["criteria"]["true"],
            "service is down or unhealthy"
        );
        assert_eq!(json["questions"]["remediation"]["type"], "choice");
        assert_eq!(
            json["questions"]["remediation"]["criteria"],
            serde_json::json!({ "restart": null, "noop": null })
        );
    }

    #[test]
    fn answer_wire_format_matches_contract() {
        let answers = BTreeMap::from([
            ("degraded".to_string(), DecisionAnswer::Noul { noul: 0.93 }),
            (
                "remediation".to_string(),
                DecisionAnswer::Choice {
                    choice: "restart".into(),
                    confidence: 0.88,
                    probabilities: [("restart".into(), 0.88), ("noop".into(), 0.12)].into(),
                },
            ),
        ]);
        let resp = DecisionResponse {
            model: Some("test-model".into()),
            answers,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["answers"]["degraded"]["noul"], 0.93);
        assert_eq!(json["answers"]["remediation"]["choice"], "restart");
        assert_eq!(json["answers"]["remediation"]["confidence"], 0.88);
    }

    #[test]
    fn confidence_is_largest_label_probability() {
        let a = DecisionAnswer::Choice {
            choice: "a".into(),
            confidence: 0.9,
            probabilities: BTreeMap::new(),
        };
        assert_eq!(a.confidence(), 0.9);
        let n = DecisionAnswer::Noul { noul: 0.7 };
        assert_eq!(n.confidence(), 0.7);
    }
}
