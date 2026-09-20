//! Structured-decision authoring (FR-011).
//!
//! A [`DecisionSet`] is an ordered, validated collection of questions that
//! becomes a [`DecisionRequest`]. Authoring rules (structural levels, explicit
//! caps, decision-procedure scoring) are guidance applied when composing
//! questions; this module enforces the shape rules the engine requires.

use std::collections::BTreeMap;

use crate::decision::error::DecisionError;
use crate::decision::types::{DecisionQuestion, DecisionRequest};
use crate::decision::validate::validate_question;
use serde_json::Value;

/// An ordered, validated set of questions.
#[derive(Debug, Clone, Default)]
pub struct DecisionSet {
    questions: BTreeMap<String, DecisionQuestion>,
}

impl DecisionSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a question, validating its shape (criteria count, etc.).
    pub fn add(
        &mut self,
        id: impl Into<String>,
        question: DecisionQuestion,
    ) -> Result<(), DecisionError> {
        validate_question(&question)?;
        self.questions.insert(id.into(), question);
        Ok(())
    }

    /// Builds a request over a `state` context.
    pub fn into_request(self, state: Value) -> DecisionRequest {
        DecisionRequest {
            model: None,
            state,
            questions: self.questions,
        }
    }

    pub fn len(&self) -> usize {
        self.questions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.questions.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::types::NoulCriteria;

    #[test]
    fn add_rejects_invalid_question() {
        let mut set = DecisionSet::new();
        let bad = DecisionQuestion::choice("pick", BTreeMap::from([("a".into(), None)]));
        assert!(set.add("q", bad).is_err());
        assert!(set.is_empty());
    }

    #[test]
    fn into_request_preserves_state() {
        let mut set = DecisionSet::new();
        set.add("ok", DecisionQuestion::noul("?", NoulCriteria::default()))
            .unwrap();
        let request = set.into_request(serde_json::json!({ "unit": "nginx" }));
        assert_eq!(request.state["unit"], "nginx");
        assert_eq!(request.questions.len(), 1);
    }
}
