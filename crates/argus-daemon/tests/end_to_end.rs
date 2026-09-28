//! The closing end-to-end suite: the whole loop on the deterministic fake-engine
//! path — evidence, context, typed decision, validated plan, policy, executor,
//! recorded evidence — with no live model, cloud, Kubernetes, NATS, eBPF, or
//! network. Proves the epic's Done-when #5 (a deterministic loop without a live
//! model) headless.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use argus_ai_core::decision::adapters::FakeDecisionProvider;
use argus_ai_core::decision::context::ContextBuilder;
use argus_ai_core::decision::error::DecisionError;
use argus_ai_core::decision::provider::DecisionProvider;
use argus_ai_core::decision::types::{DecisionAnswer, DecisionRequest, DecisionResponse};
use argus_daemon::{DiagnoseContext, DispatchError, InMemoryDedup, diagnose_with};
use argus_events::LocalEventBus;
use argus_executor::MockServiceController;
use argus_state::{DomainRepository, InMemoryRepository};
use serde_json::Value;

const UNIT: &str = "nginx.service";

fn answers(confidence: f64) -> BTreeMap<String, DecisionAnswer> {
    BTreeMap::from([
        (
            "degraded".to_string(),
            DecisionAnswer::Noul { noul: confidence },
        ),
        (
            "remediation".to_string(),
            DecisionAnswer::Choice {
                choice: "restart".into(),
                confidence,
                probabilities: BTreeMap::from([
                    ("restart".to_string(), confidence),
                    ("noop".to_string(), 1.0 - confidence),
                ]),
            },
        ),
    ])
}

fn evidence() -> ContextBuilder {
    let mut evidence = ContextBuilder::new();
    evidence.evidence("host:a", "service.nginx.state", serde_json::json!("failed"));
    evidence.evidence("host:a", "unit", serde_json::json!(UNIT));
    evidence
}

/// A decision engine that cannot be reached (no model API available).
struct UnavailableProvider;

#[async_trait::async_trait]
impl DecisionProvider for UnavailableProvider {
    async fn decide(&self, _request: DecisionRequest) -> Result<DecisionResponse, DecisionError> {
        Err(DecisionError::Unavailable("engine down".into()))
    }
}

#[tokio::test]
async fn the_whole_loop_runs_deterministically_on_the_fake_engine() {
    let provider = FakeDecisionProvider::new(answers(0.9));
    let repository = InMemoryRepository::new();
    let service = MockServiceController::new();
    service.set_active(UNIT, true);
    let events = LocalEventBus::new(64);
    let dedup = InMemoryDedup::new();

    let calls = Arc::new(AtomicUsize::new(0));
    let first_calls = Arc::clone(&calls);
    let first_dispatch = move |_request: argus_domain::CapabilityRequest| {
        first_calls.fetch_add(1, Ordering::SeqCst);
        Ok::<Value, DispatchError>(serde_json::json!({ "restarted": true }))
    };

    let first = diagnose_with(
        first_dispatch,
        DiagnoseContext {
            repository: &repository,
            provider: &provider,
            dedup: &dedup,
            service: &service,
            events: &events,
        },
        evidence(),
        0.7,
    )
    .await
    .expect("the loop runs headless");

    let plan = first.expect("a schema-validated plan is produced");
    assert_eq!(plan.steps.len(), 1, "one typed step");
    assert_eq!(
        plan.steps[0].action.capability.as_str(),
        "host.service.restart",
        "the step names the restart capability"
    );
    assert_eq!(
        plan.steps[0].action.arguments["unit"], UNIT,
        "the step carries the target unit"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the action executed once through the policy/executor dispatch boundary"
    );

    // Evidence and audit are recorded.
    let audit = repository
        .list_audit_events()
        .await
        .expect("list audit events");
    assert!(
        audit.len() >= 2,
        "the plan and its validation are both audited, got {}",
        audit.len()
    );
    assert!(
        !repository
            .list_observations()
            .await
            .expect("list observations")
            .is_empty(),
        "the re-observed state is recorded as evidence"
    );

    // Determinism and no duplicate trigger: the same observation does not spawn
    // a second plan or a second execution.
    let second_calls = Arc::clone(&calls);
    let second_dispatch = move |_request: argus_domain::CapabilityRequest| {
        second_calls.fetch_add(1, Ordering::SeqCst);
        Ok::<Value, DispatchError>(serde_json::json!({ "restarted": true }))
    };
    let second = diagnose_with(
        second_dispatch,
        DiagnoseContext {
            repository: &repository,
            provider: &provider,
            dedup: &dedup,
            service: &service,
            events: &events,
        },
        evidence(),
        0.7,
    )
    .await
    .expect("the loop runs again");
    assert!(
        second.is_none(),
        "a repeated observation spawns no second plan"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "still exactly one execution (deduplicated)"
    );
    let after_repeat = repository
        .list_audit_events()
        .await
        .expect("list audit events");
    assert!(
        after_repeat.len() > audit.len(),
        "the deduplicated repeat is recorded in the audit trail"
    );
}

#[tokio::test]
async fn the_same_input_produces_the_same_outcome_on_a_fresh_run() {
    let mut outcomes = Vec::new();
    for _ in 0..2 {
        let provider = FakeDecisionProvider::new(answers(0.9));
        let repository = InMemoryRepository::new();
        let service = MockServiceController::new();
        service.set_active(UNIT, true);
        let events = LocalEventBus::new(16);
        let dedup = InMemoryDedup::new();

        let calls = Arc::new(AtomicUsize::new(0));
        let dispatch_calls = Arc::clone(&calls);
        let dispatch = move |_request: argus_domain::CapabilityRequest| {
            dispatch_calls.fetch_add(1, Ordering::SeqCst);
            Ok::<Value, DispatchError>(serde_json::json!({ "restarted": true }))
        };

        let plan = diagnose_with(
            dispatch,
            DiagnoseContext {
                repository: &repository,
                provider: &provider,
                dedup: &dedup,
                service: &service,
                events: &events,
            },
            evidence(),
            0.7,
        )
        .await
        .expect("the loop runs headless")
        .expect("the same input proposes a plan on a fresh run");

        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "one execution per fresh run"
        );
        outcomes.push(plan.steps[0].action.capability.as_str().to_string());
    }
    assert_eq!(
        outcomes[0], outcomes[1],
        "the same input produces the same outcome"
    );
}

#[tokio::test]
async fn a_low_confidence_decision_runs_nothing() {
    let provider = FakeDecisionProvider::new(answers(0.4));
    let repository = InMemoryRepository::new();
    let service = MockServiceController::new();
    let events = LocalEventBus::new(16);

    let calls = Arc::new(AtomicUsize::new(0));
    let dispatch_calls = Arc::clone(&calls);
    let dispatch = move |_request: argus_domain::CapabilityRequest| {
        dispatch_calls.fetch_add(1, Ordering::SeqCst);
        Ok::<Value, DispatchError>(serde_json::json!({ "restarted": true }))
    };

    let plan = diagnose_with(
        dispatch,
        DiagnoseContext {
            repository: &repository,
            provider: &provider,
            dedup: &InMemoryDedup::new(),
            service: &service,
            events: &events,
        },
        evidence(),
        0.7,
    )
    .await
    .expect("the loop runs");

    assert!(plan.is_none(), "below threshold proposes nothing");
    assert_eq!(calls.load(Ordering::SeqCst), 0, "nothing executes");
}

#[tokio::test]
async fn an_unavailable_engine_fails_closed() {
    let repository = InMemoryRepository::new();
    let service = MockServiceController::new();
    let events = LocalEventBus::new(16);

    let calls = Arc::new(AtomicUsize::new(0));
    let dispatch_calls = Arc::clone(&calls);
    let dispatch = move |_request: argus_domain::CapabilityRequest| {
        dispatch_calls.fetch_add(1, Ordering::SeqCst);
        Ok::<Value, DispatchError>(serde_json::json!({ "restarted": true }))
    };

    let plan = diagnose_with(
        dispatch,
        DiagnoseContext {
            repository: &repository,
            provider: &UnavailableProvider,
            dedup: &InMemoryDedup::new(),
            service: &service,
            events: &events,
        },
        evidence(),
        0.7,
    )
    .await
    .expect("the loop fails closed rather than erroring");

    assert!(plan.is_none(), "an unavailable engine produces no decision");
    assert_eq!(calls.load(Ordering::SeqCst), 0, "nothing executes");
}
