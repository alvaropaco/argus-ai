//! Decision-authoring regression harness (research.md §7).
//!
//! Re-runs a fixed fixture through the deterministic fake provider to catch
//! regressions when question authoring changes. No live model is involved.

use std::collections::BTreeMap;
use std::sync::Arc;

use argus_ai_core::decision::adapters::FakeDecisionProvider;
use argus_ai_core::decision::author::DecisionSet;
use argus_ai_core::decision::context::ContextBuilder;
use argus_ai_core::decision::gateway::{ReasoningGateway, aggregate_confidence};
use argus_ai_core::decision::types::{DecisionAnswer, DecisionQuestion, NoulCriteria};

/// Authors the standard service-degradation decisions.
fn degradation_questions() -> DecisionSet {
    let mut set = DecisionSet::new();
    set.add(
        "degraded",
        DecisionQuestion::noul(
            "Is the target service currently degraded?",
            NoulCriteria {
                yes: Some("service is down, failed, or unhealthy".into()),
                no: Some("service is running and healthy".into()),
            },
        ),
    )
    .unwrap();
    set.add(
        "remediation",
        DecisionQuestion::choice(
            "Which remediation best matches the observed condition?",
            BTreeMap::from([
                (
                    "restart".to_string(),
                    Some("restart the service".to_string()),
                ),
                ("noop".to_string(), Some("do nothing".to_string())),
            ]),
        ),
    )
    .unwrap();
    set
}

#[tokio::test]
async fn degraded_service_yields_restart_remediation() {
    let mut ctx = ContextBuilder::new();
    ctx.evidence("host:test", "unit.state", serde_json::json!("failed"));

    let provider = FakeDecisionProvider::new(BTreeMap::from([
        ("degraded".to_string(), DecisionAnswer::Noul { noul: 0.95 }),
        (
            "remediation".to_string(),
            DecisionAnswer::Choice {
                choice: "restart".into(),
                confidence: 0.9,
                probabilities: [("restart".into(), 0.9), ("noop".into(), 0.1)].into(),
            },
        ),
    ]));
    let gateway = ReasoningGateway::new(Arc::new(provider), 0.8);

    let request = degradation_questions().into_request(ctx.into_state());
    let response = gateway.decide(request).await.expect("valid response");

    let confidence = aggregate_confidence(&response);
    assert!(confidence >= gateway.confidence_threshold());

    // Regression: the fixture must keep selecting "restart".
    match response.answers.get("remediation") {
        Some(DecisionAnswer::Choice { choice, .. }) => assert_eq!(choice, "restart"),
        other => panic!("expected choice answer, got {other:?}"),
    }
}

#[tokio::test]
async fn healthy_service_yields_noop_remediation() {
    let provider = FakeDecisionProvider::new(BTreeMap::from([
        ("degraded".to_string(), DecisionAnswer::Noul { noul: 0.1 }),
        (
            "remediation".to_string(),
            DecisionAnswer::Choice {
                choice: "noop".into(),
                confidence: 0.85,
                probabilities: [("noop".into(), 0.85), ("restart".into(), 0.15)].into(),
            },
        ),
    ]));
    let gateway = ReasoningGateway::new(Arc::new(provider), 0.8);

    let ctx = ContextBuilder::new();
    let request = degradation_questions().into_request(ctx.into_state());
    let response = gateway.decide(request).await.expect("valid response");

    match response.answers.get("remediation") {
        Some(DecisionAnswer::Choice { choice, .. }) => assert_eq!(choice, "noop"),
        other => panic!("expected choice answer, got {other:?}"),
    }
}
