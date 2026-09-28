//! Pure, deterministic rendering of typed artifacts into human-facing text.
//!
//! Views compose only typed domain fields with static labels. They perform no
//! IO, make no model calls, and expose no path from a view to an action
//! (ADR-0029 §4).

use std::collections::BTreeMap;

use argus_domain::{Action, Plan};
use serde_json::Value;

use crate::decision::{
    DecisionAnswer, DecisionOutcome, DecisionProvenance, DecisionResponse, EvidenceEntry,
    NoDecisionReason,
};

/// The fixed summary produced when there is nothing to render (an empty plan or
/// no evidence). It is deliberately non-blank so a missing explanation is
/// visible rather than silent.
pub const NOTHING_TO_EXPLAIN: &str = "nothing to explain";

/// Renders a human-facing explanation of a [`Plan`] from its objective, status,
/// confidence, blast radius, preconditions, expected outcomes, and ordered steps
/// (each with its rollback when present).
///
/// A plan with no objective and no steps yields [`NOTHING_TO_EXPLAIN`].
pub fn explain_plan(plan: &Plan) -> String {
    if plan.objective.is_empty() && plan.steps.is_empty() {
        return NOTHING_TO_EXPLAIN.to_string();
    }
    let mut lines = Vec::new();
    lines.push(format!("objective: {}", plan.objective));
    lines.push(format!("status: {}", enum_label(&plan.status)));
    lines.push(format!("confidence: {}", format_float(plan.confidence)));
    lines.push(format!("blast_radius: {}", enum_label(&plan.blast_radius)));
    push_list(&mut lines, "preconditions", &plan.preconditions);
    push_list(&mut lines, "expected_outcomes", &plan.expected_outcomes);
    if plan.steps.is_empty() {
        lines.push("steps: none".to_string());
    } else {
        lines.push("steps:".to_string());
        for (index, step) in plan.steps.iter().enumerate() {
            lines.push(format!("  {}. {}", index + 1, action_label(&step.action)));
            if let Some(rollback) = &step.rollback {
                lines.push(format!("     rollback: {}", action_label(rollback)));
            }
        }
    }
    lines.join("\n")
}

/// Renders a human-facing explanation of a decision outcome and its provenance.
///
/// A decided outcome names the chosen answer(s) and the model, version,
/// calibration set, and context hash that produced it; a no-decision names the
/// fail-closed reason and the context hash.
pub fn explain_decision(outcome: &DecisionOutcome, provenance: &DecisionProvenance) -> String {
    match outcome {
        DecisionOutcome::Decided(response) => explain_decided(response, provenance),
        DecisionOutcome::NoDecision(reason) => format!(
            "decision: none\nreason: {}\ncontext_hash: {}",
            no_decision_label(*reason),
            provenance.context_hash
        ),
    }
}

/// Renders a deterministic subject/attribute/value summary of a set of evidence
/// entries, ordered so that the same evidence in any order yields identical
/// output.
///
/// Control characters in any field are stripped so a value cannot inject
/// formatting. Redaction is a precondition: entries must already be redacted
/// (e.g. via [`crate::decision::ContextBuilder`]) before they reach this view.
///
/// An empty set yields [`NOTHING_TO_EXPLAIN`].
pub fn summarize_evidence(entries: &[EvidenceEntry]) -> String {
    if entries.is_empty() {
        return NOTHING_TO_EXPLAIN.to_string();
    }
    let mut ordered: Vec<&EvidenceEntry> = entries.iter().collect();
    ordered.sort_by_cached_key(|entry| {
        (
            entry.subject.clone(),
            entry.attribute.clone(),
            entry.value.to_string(),
        )
    });
    ordered
        .iter()
        .map(|entry| {
            format!(
                "{} {} = {}",
                sanitize(&entry.subject),
                sanitize(&entry.attribute),
                sanitize(&entry.value.to_string())
            )
        })
        .collect::<Vec<String>>()
        .join("\n")
}

/// Renders an enum variant by its serde snake_case wire name, so the human
/// label cannot drift from the wire encoding.
fn enum_label<T: serde::Serialize>(value: &T) -> String {
    match serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
    {
        Some(label) => label,
        None => "unknown".to_string(),
    }
}

/// Appends a labelled list of items, or a `none` marker when empty.
fn push_list(lines: &mut Vec<String>, label: &str, items: &[String]) {
    if items.is_empty() {
        lines.push(format!("{label}: none"));
    } else {
        lines.push(format!("{label}:"));
        for item in items {
            lines.push(format!("  - {item}"));
        }
    }
}

/// Renders an action as its capability, optional resource, and arguments.
fn action_label(action: &Action) -> String {
    let mut label = String::new();
    label.push_str(action.capability.as_str());
    if let Some(resource) = &action.resource {
        label.push_str(" on ");
        label.push_str(resource.as_str());
    }
    if has_arguments(&action.arguments) {
        label.push_str(" with ");
        label.push_str(&action.arguments.to_string());
    }
    label
}

/// Whether an action carries any arguments worth rendering.
fn has_arguments(arguments: &Value) -> bool {
    match arguments {
        Value::Null => false,
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
        _ => true,
    }
}

/// Renders the chosen value of a single answer, plus its confidence and, where
/// present, its probabilities and legend.
fn answer_label(answer: &DecisionAnswer) -> String {
    match answer {
        DecisionAnswer::Choice {
            choice,
            confidence,
            probabilities,
        } => format!(
            "{choice} (confidence {}, probabilities: {})",
            format_float(*confidence),
            float_map_label(probabilities)
        ),
        DecisionAnswer::Score {
            score,
            confidence,
            probabilities,
            legend,
        } => format!(
            "score {} (confidence {}, probabilities: {}, legend: {})",
            format_float(*score),
            format_float(*confidence),
            float_map_label(probabilities),
            map_label(legend)
        ),
        DecisionAnswer::Noul { noul } => format!("noul {}", format_float(*noul)),
    }
}

/// Renders a name=value map of strings in sorted key order, or `none` when empty.
fn map_label(map: &BTreeMap<String, String>) -> String {
    if map.is_empty() {
        return "none".to_string();
    }
    map.iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<String>>()
        .join(", ")
}

/// Renders a name=value map of floats in sorted key order, or `none` when empty,
/// formatting each value canonically via [`format_float`].
fn float_map_label(map: &BTreeMap<String, f64>) -> String {
    if map.is_empty() {
        return "none".to_string();
    }
    map.iter()
        .map(|(name, value)| format!("{name}={}", format_float(*value)))
        .collect::<Vec<String>>()
        .join(", ")
}

/// Formats an `f64` canonically: the shortest round-trip representation, always
/// carrying a fractional part for integral values (`3.0`, not `3`). This makes
/// golden output independent of how a value was produced or represented.
fn format_float(value: f64) -> String {
    format!("{value:?}")
}

/// Renders a no-decision reason as its fail-closed explanation.
fn no_decision_label(reason: NoDecisionReason) -> &'static str {
    match reason {
        NoDecisionReason::LowConfidence => "low confidence",
        NoDecisionReason::EngineUnavailable => "engine unavailable",
    }
}

/// Strips control characters from a free-form field so it cannot inject line
/// breaks or other formatting into a rendered view.
fn sanitize(input: &str) -> String {
    input.chars().filter(|c| !c.is_control()).collect()
}

/// Renders a decided outcome: the chosen answers, then the provenance.
///
/// A decided outcome with no answers carries nothing to explain.
fn explain_decided(response: &DecisionResponse, provenance: &DecisionProvenance) -> String {
    if response.answers.is_empty() {
        return NOTHING_TO_EXPLAIN.to_string();
    }
    let mut lines = Vec::with_capacity(response.answers.len() + 4);
    for (question, answer) in &response.answers {
        lines.push(format!("{question}: {}", answer_label(answer)));
    }
    lines.push(format!("model: {}", provenance.model_id));
    lines.push(format!("model_version: {}", provenance.model_version));
    lines.push(format!("calibration_set: {}", provenance.calibration_set));
    lines.push(format!("context_hash: {}", provenance.context_hash));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use argus_domain::{BlastRadius, CapabilityId, PlanStatus, PlanStep, ResourceId};
    use serde_json::json;
    use std::collections::BTreeMap;

    fn sample_plan() -> Plan {
        Plan {
            objective: "restore nginx".to_string(),
            steps: vec![PlanStep {
                action: Action {
                    capability: CapabilityId::new("host.service.restart").unwrap(),
                    resource: Some(ResourceId::new("host", "web-1").unwrap()),
                    arguments: json!({ "unit": "nginx.service" }),
                },
                rollback: Some(Action {
                    capability: CapabilityId::new("host.service.stop").unwrap(),
                    resource: Some(ResourceId::new("host", "web-1").unwrap()),
                    arguments: json!({}),
                }),
            }],
            preconditions: vec!["service is failed".to_string()],
            expected_outcomes: vec!["nginx running".to_string()],
            blast_radius: BlastRadius::Host,
            confidence: 0.9,
            status: PlanStatus::Proposed,
        }
    }

    fn empty_plan() -> Plan {
        Plan {
            objective: String::new(),
            steps: Vec::new(),
            preconditions: Vec::new(),
            expected_outcomes: Vec::new(),
            blast_radius: BlastRadius::None,
            confidence: 0.0,
            status: PlanStatus::Proposed,
        }
    }

    fn provenance() -> DecisionProvenance {
        DecisionProvenance {
            model_id: "laya-en".to_string(),
            model_version: "1.2.0".to_string(),
            calibration_set: "cal-2026".to_string(),
            context_hash: "0123456789abcdef".to_string(),
        }
    }

    fn decided_outcome() -> DecisionOutcome {
        DecisionOutcome::Decided(DecisionResponse {
            model: Some("laya-en".to_string()),
            answers: BTreeMap::from([(
                "remediation".to_string(),
                DecisionAnswer::Choice {
                    choice: "restart".to_string(),
                    confidence: 0.88,
                    probabilities: BTreeMap::from([
                        ("restart".to_string(), 0.88),
                        ("noop".to_string(), 0.12),
                    ]),
                },
            )]),
        })
    }

    fn evidence(attribute: &str, value: serde_json::Value) -> EvidenceEntry {
        EvidenceEntry {
            subject: "host:a".to_string(),
            attribute: attribute.to_string(),
            value,
        }
    }

    #[test]
    fn plan_explanation_is_golden() {
        let expected = r#"objective: restore nginx
status: proposed
confidence: 0.9
blast_radius: host
preconditions:
  - service is failed
expected_outcomes:
  - nginx running
steps:
  1. host.service.restart on host:web-1 with {"unit":"nginx.service"}
     rollback: host.service.stop on host:web-1"#;
        assert_eq!(explain_plan(&sample_plan()), expected);
    }

    #[test]
    fn decided_explanation_is_golden() {
        let expected = r#"remediation: restart (confidence 0.88, probabilities: noop=0.12, restart=0.88)
model: laya-en
model_version: 1.2.0
calibration_set: cal-2026
context_hash: 0123456789abcdef"#;
        assert_eq!(
            explain_decision(&decided_outcome(), &provenance()),
            expected
        );
    }

    #[test]
    fn no_decision_explanation_names_reason_and_context_hash() {
        let low = DecisionOutcome::NoDecision(NoDecisionReason::LowConfidence);
        assert_eq!(
            explain_decision(&low, &provenance()),
            "decision: none\nreason: low confidence\ncontext_hash: 0123456789abcdef"
        );

        let unavailable = DecisionOutcome::NoDecision(NoDecisionReason::EngineUnavailable);
        assert_eq!(
            explain_decision(&unavailable, &provenance()),
            "decision: none\nreason: engine unavailable\ncontext_hash: 0123456789abcdef"
        );
    }

    #[test]
    fn evidence_summary_is_golden() {
        let entries = vec![
            evidence("unit", json!("nginx.service")),
            evidence("memory.pressure", json!(0.8)),
        ];
        let expected = r#"host:a memory.pressure = 0.8
host:a unit = "nginx.service""#;
        assert_eq!(summarize_evidence(&entries), expected);
    }

    #[test]
    fn empty_plan_and_empty_evidence_render_nothing_to_explain() {
        assert_eq!(explain_plan(&empty_plan()), NOTHING_TO_EXPLAIN);
        assert_eq!(summarize_evidence(&[]), NOTHING_TO_EXPLAIN);
    }

    #[test]
    fn decided_outcome_with_no_answers_is_nothing_to_explain() {
        let empty = DecisionOutcome::Decided(DecisionResponse {
            model: Some("laya-en".to_string()),
            answers: BTreeMap::new(),
        });
        assert_eq!(explain_decision(&empty, &provenance()), NOTHING_TO_EXPLAIN);
    }

    #[test]
    fn evidence_summary_is_order_independent() {
        let first = evidence("cpu", json!(0.5));
        let second = evidence("memory.pressure", json!(0.8));
        let forward = summarize_evidence(&[first.clone(), second.clone()]);
        let reversed = summarize_evidence(&[second, first]);
        assert_eq!(forward, reversed);
    }

    #[test]
    fn answer_labels_cover_score_and_noul() {
        let score = DecisionAnswer::Score {
            score: 3.0,
            confidence: 0.9,
            probabilities: BTreeMap::from([("severe".to_string(), 0.9)]),
            legend: BTreeMap::from([("3".to_string(), "severe".to_string())]),
        };
        assert_eq!(
            answer_label(&score),
            "score 3.0 (confidence 0.9, probabilities: severe=0.9, legend: 3=severe)"
        );

        let noul = DecisionAnswer::Noul { noul: 0.95 };
        assert_eq!(answer_label(&noul), "noul 0.95");
    }

    #[test]
    fn plan_and_decision_render_deterministically() {
        assert_eq!(explain_plan(&sample_plan()), explain_plan(&sample_plan()));
        assert_eq!(
            explain_decision(&decided_outcome(), &provenance()),
            explain_decision(&decided_outcome(), &provenance())
        );

        let first = DecisionResponse {
            model: None,
            answers: BTreeMap::from([
                ("first".to_string(), DecisionAnswer::Noul { noul: 0.9 }),
                ("second".to_string(), DecisionAnswer::Noul { noul: 0.8 }),
            ]),
        };
        let second = DecisionResponse {
            model: None,
            answers: BTreeMap::from([
                ("second".to_string(), DecisionAnswer::Noul { noul: 0.8 }),
                ("first".to_string(), DecisionAnswer::Noul { noul: 0.9 }),
            ]),
        };
        assert_eq!(
            explain_decision(&DecisionOutcome::Decided(first), &provenance()),
            explain_decision(&DecisionOutcome::Decided(second), &provenance())
        );
    }

    #[test]
    fn sanitize_strips_control_characters() {
        assert_eq!(sanitize("host:a\nunit"), "host:aunit");
        assert_eq!(sanitize("a\tb\rc"), "abc");
    }

    #[test]
    fn view_module_is_pure_and_never_reaches_an_executor() {
        let source = include_str!("view.rs");
        // Exclude the test module so the guard's own assertions cannot match.
        let body = source.split("#[cfg(test)]").next().unwrap_or(source);
        const FORBIDDEN: &[&str] = &[
            "Executor",
            "ActionPort",
            "authorize_and_execute",
            "argus_executor",
            "argus_daemon",
            "ModelProvider",
            "generate",
            "crate::model",
            "std::fs",
            "std::io",
            "reqwest",
            "tokio",
            "async fn",
            ".await",
            "println",
        ];
        for token in FORBIDDEN {
            assert!(
                !body.contains(token),
                "the view module must not reference `{token}`"
            );
        }
    }
}
