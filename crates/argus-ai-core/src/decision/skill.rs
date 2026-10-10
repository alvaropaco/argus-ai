//! Skill runtime: versioned, declarative skills selected over an
//! applicability-filtered catalogue (CAP-15).
//!
//! A skill is data, never authority: it declares the typed questions it poses
//! and the attributes it applies to. It cannot execute anything or name an
//! action to run — the manifest type has no such field, and unknown keys are
//! rejected at parse.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

use crate::decision::error::DecisionError;
use crate::decision::types::{DecisionAnswer, DecisionQuestion, DecisionResponse};

/// The reserved option meaning "no applicable skill": observe-only or escalate.
pub const NO_SKILL: &str = "noul";

/// A skill manifest, loaded from TOML.
///
/// Unknown keys are rejected, so a manifest cannot smuggle an executable field
/// such as `run` or `action`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillManifest {
    pub skill: SkillHeader,
    #[serde(default)]
    pub applies: Applies,
    #[serde(default)]
    pub questions: BTreeMap<String, DecisionQuestion>,
}

/// A skill's identity and compatibility.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillHeader {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub api_compat: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
}

/// The pure applicability predicate: the attributes that must be present.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Applies {
    #[serde(default)]
    pub attributes_present: Vec<String>,
}

impl SkillManifest {
    /// Whether the skill applies to the given context state.
    ///
    /// Pure and deterministic: it inspects only the state's evidence attributes.
    pub fn applies_to(&self, state: &Value) -> bool {
        let present: Vec<&str> = state
            .get("evidence")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| entry.get("attribute").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        self.applies
            .attributes_present
            .iter()
            .all(|required| present.contains(&required.as_str()))
    }
}

/// A catalogue of skills loaded from one TOML document.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SkillCatalogue {
    skills: Vec<SkillManifest>,
}

impl SkillCatalogue {
    /// Loads a catalogue from a TOML document with one `[[skills]]` table per skill.
    pub fn from_toml(source: &str) -> Result<Self, DecisionError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Doc {
            #[serde(default)]
            skills: Vec<SkillManifest>,
        }
        let doc: Doc = toml::from_str(source).map_err(|e| DecisionError::Invalid(e.to_string()))?;
        Ok(Self { skills: doc.skills })
    }

    pub fn len(&self) -> usize {
        self.skills.len()
    }

    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    /// The skills whose applicability predicate holds for `state`.
    pub fn applicable(&self, state: &Value) -> Vec<&SkillManifest> {
        self.skills
            .iter()
            .filter(|skill| skill.applies_to(state))
            .collect()
    }
}

/// Builds the `choice` that selects a skill, reserving [`NO_SKILL`].
///
/// Returns `None` when no skill applies — that is [`NO_SKILL`] without posing a
/// question (a choice needs at least two criteria).
pub fn selection_question(applicable: &[&SkillManifest]) -> Option<DecisionQuestion> {
    if applicable.is_empty() {
        return None;
    }
    let mut criteria: BTreeMap<String, Option<String>> = applicable
        .iter()
        .map(|skill| {
            (
                skill.skill.name.clone(),
                Some(format!("apply the '{}' skill", skill.skill.name)),
            )
        })
        .collect();
    criteria.insert(
        NO_SKILL.to_string(),
        Some("no skill applies; observe only".to_string()),
    );
    Some(DecisionQuestion::choice(
        "Which skill fits this situation?",
        criteria,
    ))
}

/// The skill a selection response chose, or `None` for the reserved [`NO_SKILL`].
pub fn selected_skill<'a>(
    response: &DecisionResponse,
    applicable: &[&'a SkillManifest],
) -> Option<&'a SkillManifest> {
    let choice = match response.answers.values().next() {
        Some(DecisionAnswer::Choice { choice, .. }) => choice.as_str(),
        _ => return None,
    };
    if choice == NO_SKILL {
        return None;
    }
    applicable
        .iter()
        .copied()
        .find(|skill| skill.skill.name == choice)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const CATALOGUE: &str = r#"
[[skills]]
[skills.skill]
name = "host-service-triage"
version = "1.0.0"
kind = "decision"
[skills.applies]
attributes_present = ["service.nginx.state"]
[skills.questions.degraded]
type = "noul"
instructions = "Is the service degraded?"
[skills.questions.degraded.criteria]
true = "down or failing"
false = "healthy"
"#;

    fn state_with(attribute: &str) -> Value {
        json!({ "evidence": [{ "subject": "host:a", "attribute": attribute, "value": "failed" }] })
    }

    #[test]
    fn loads_a_versioned_skill() {
        let catalogue = SkillCatalogue::from_toml(CATALOGUE).expect("loads");
        assert_eq!(catalogue.len(), 1);
        let state = state_with("service.nginx.state");
        let applicable = catalogue.applicable(&state);
        assert_eq!(applicable[0].skill.name, "host-service-triage");
        assert_eq!(applicable[0].skill.version, "1.0.0");
    }

    #[test]
    fn filters_by_applicability() {
        let catalogue = SkillCatalogue::from_toml(CATALOGUE).expect("loads");
        let missing = state_with("service.other.state");
        assert!(catalogue.applicable(&missing).is_empty());
    }

    #[test]
    fn selection_question_reserves_noul() {
        let catalogue = SkillCatalogue::from_toml(CATALOGUE).expect("loads");
        let state = state_with("service.nginx.state");
        let applicable = catalogue.applicable(&state);
        let question = selection_question(&applicable).expect("a question");
        match question {
            DecisionQuestion::Choice { criteria, .. } => {
                assert!(criteria.contains_key("host-service-triage"));
                assert!(criteria.contains_key(NO_SKILL));
            }
            other => panic!("expected a choice, got {other:?}"),
        }
        assert!(selection_question(&[]).is_none());
    }

    #[test]
    fn selection_maps_a_choice_and_noul() {
        let catalogue = SkillCatalogue::from_toml(CATALOGUE).expect("loads");
        let state = state_with("service.nginx.state");
        let applicable = catalogue.applicable(&state);
        let skill = applicable[0];

        let mut answers = BTreeMap::new();
        answers.insert(
            "selection".to_string(),
            DecisionAnswer::Choice {
                choice: "host-service-triage".into(),
                confidence: 0.9,
                probabilities: BTreeMap::new(),
            },
        );
        let chosen = DecisionResponse {
            model: None,
            usage: None,
            answers,
        };
        assert_eq!(
            selected_skill(&chosen, &applicable).map(|s| s.skill.name.clone()),
            Some(skill.skill.name.clone())
        );

        let mut no_skill_answers = BTreeMap::new();
        no_skill_answers.insert(
            "selection".to_string(),
            DecisionAnswer::Choice {
                choice: NO_SKILL.into(),
                confidence: 0.9,
                probabilities: BTreeMap::new(),
            },
        );
        let no_skill = DecisionResponse {
            model: None,
            usage: None,
            answers: no_skill_answers,
        };
        assert!(selected_skill(&no_skill, &applicable).is_none());
    }

    #[test]
    fn executable_field_is_rejected() {
        let with_run = r#"
[[skills]]
run = true
[skills.skill]
name = "x"
version = "1.0.0"
"#;
        assert!(matches!(
            SkillCatalogue::from_toml(with_run),
            Err(DecisionError::Invalid(_))
        ));
    }
}
