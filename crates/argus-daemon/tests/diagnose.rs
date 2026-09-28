//! The loop's wiring: a fake decision provider through an injected dispatch and
//! an in-memory repository, proving execution and recording without a full daemon.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use argus_ai_core::decision::adapters::FakeDecisionProvider;
use argus_ai_core::decision::context::ContextBuilder;
use argus_ai_core::decision::types::DecisionAnswer;
use argus_daemon::{DiagnoseContext, DispatchError, InMemoryDedup, diagnose_with};
use argus_domain::ObservedValue;
use argus_events::LocalEventBus;
use argus_executor::MockServiceController;
use argus_state::{DomainRepository, InMemoryRepository};
use serde_json::Value;

#[tokio::test]
async fn diagnose_executes_through_dispatch_and_records() {
    let provider = FakeDecisionProvider::new(BTreeMap::from([
        ("degraded".to_string(), DecisionAnswer::Noul { noul: 0.9 }),
        (
            "remediation".to_string(),
            DecisionAnswer::Choice {
                choice: "restart".into(),
                confidence: 0.9,
                probabilities: BTreeMap::from([
                    ("restart".to_string(), 0.9),
                    ("noop".to_string(), 0.1),
                ]),
            },
        ),
    ]));

    let calls = Arc::new(AtomicUsize::new(0));
    let calls_for_dispatch = Arc::clone(&calls);
    let arguments = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
    let arguments_for_dispatch = Arc::clone(&arguments);
    let dispatch = move |request: argus_domain::CapabilityRequest| {
        calls_for_dispatch.fetch_add(1, Ordering::SeqCst);
        arguments_for_dispatch
            .lock()
            .unwrap()
            .push(request.arguments.clone());
        Ok::<Value, DispatchError>(serde_json::json!({ "restarted": true }))
    };

    let repository = InMemoryRepository::new();
    let mut evidence = ContextBuilder::new();
    evidence.evidence("host:a", "service.nginx.state", serde_json::json!("failed"));
    evidence.evidence("host:a", "unit", serde_json::json!("nginx.service"));

    let service = MockServiceController::new();
    service.set_active("nginx.service", true);
    let bus = LocalEventBus::new(16);

    let plan = diagnose_with(
        dispatch,
        DiagnoseContext {
            repository: &repository,
            provider: &provider,
            dedup: &InMemoryDedup::new(),
            service: &service,
            events: &bus,
        },
        evidence,
        0.7,
    )
    .await
    .expect("loop runs");

    assert!(plan.is_some(), "a plan clears the threshold");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the action was dispatched once"
    );
    {
        let arguments = arguments.lock().unwrap();
        assert_eq!(arguments.len(), 1, "the restart was dispatched");
        assert_eq!(
            arguments[0]["unit"], "nginx.service",
            "the restart arguments carry the unit"
        );
    }
    let events = repository.list_audit_events().await.expect("list events");
    assert_eq!(
        events.len(),
        2,
        "the plan and its validation are both audited"
    );

    // Reconstruct the decision's provenance from the log.
    let provenance = &events[0].payload()["provenance"];
    assert_eq!(provenance["model_id"], "unknown");
    assert!(
        provenance["context_hash"]
            .as_str()
            .is_some_and(|hash| hash.len() == 16),
        "the context hash is reconstructable from the log"
    );
}

/// The manual hitl demo: run the loop against a live `laya-serve`.
///
/// `ARGUS_LAYA_URL=http://127.0.0.1:8000 cargo test -p argus-daemon -- --ignored diagnose_against_live_laya`
#[tokio::test]
#[ignore = "requires a running laya-serve (hitl)"]
async fn diagnose_against_live_laya() {
    let base_url =
        std::env::var("ARGUS_LAYA_URL").unwrap_or_else(|_| "http://127.0.0.1:8000".to_string());
    let provider = argus_ai_core::decision::adapters::LayaHttpProvider::new(base_url);

    let calls = Arc::new(AtomicUsize::new(0));
    let calls_for_dispatch = Arc::clone(&calls);
    let dispatch = move |_request: argus_domain::CapabilityRequest| {
        calls_for_dispatch.fetch_add(1, Ordering::SeqCst);
        Ok::<Value, DispatchError>(serde_json::json!({ "restarted": true }))
    };

    let repository = InMemoryRepository::new();
    let mut evidence = ContextBuilder::new();
    evidence.evidence("host:a", "service.nginx.state", serde_json::json!("failed"));
    evidence.evidence("host:a", "unit", serde_json::json!("nginx.service"));

    let service = MockServiceController::new();
    let bus = LocalEventBus::new(16);

    let plan = diagnose_with(
        dispatch,
        DiagnoseContext {
            repository: &repository,
            provider: &provider,
            dedup: &InMemoryDedup::new(),
            service: &service,
            events: &bus,
        },
        evidence,
        0.3,
    )
    .await
    .expect("loop runs against a live laya-serve");
    assert!(plan.is_some(), "laya drove a plan");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_repeated_observation_spawns_one_plan() {
    let provider = FakeDecisionProvider::new(BTreeMap::from([
        ("degraded".to_string(), DecisionAnswer::Noul { noul: 0.9 }),
        (
            "remediation".to_string(),
            DecisionAnswer::Choice {
                choice: "restart".into(),
                confidence: 0.9,
                probabilities: BTreeMap::from([
                    ("restart".to_string(), 0.9),
                    ("noop".to_string(), 0.1),
                ]),
            },
        ),
    ]));

    let calls = Arc::new(AtomicUsize::new(0));
    let repository = InMemoryRepository::new();
    let dedup = InMemoryDedup::new();
    let service = MockServiceController::new();
    service.set_active("nginx.service", true);
    let bus = LocalEventBus::new(16);

    let evidence = || {
        let mut context = ContextBuilder::new();
        context.evidence("host:a", "service.nginx.state", serde_json::json!("failed"));
        context.evidence("host:a", "unit", serde_json::json!("nginx.service"));
        context
    };

    let dispatch = {
        let calls = Arc::clone(&calls);
        move |_request: argus_domain::CapabilityRequest| {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok::<Value, DispatchError>(serde_json::json!({ "restarted": true }))
        }
    };

    let first = diagnose_with(
        dispatch.clone(),
        DiagnoseContext {
            repository: &repository,
            provider: &provider,
            dedup: &dedup,
            service: &service,
            events: &bus,
        },
        evidence(),
        0.7,
    )
    .await
    .expect("loop runs");
    assert!(first.is_some(), "the first observation plans");

    let second = diagnose_with(
        dispatch,
        DiagnoseContext {
            repository: &repository,
            provider: &provider,
            dedup: &dedup,
            service: &service,
            events: &bus,
        },
        evidence(),
        0.7,
    )
    .await
    .expect("loop runs");
    assert!(second.is_none(), "the repeat spawns no plan");

    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "exactly one execution across two identical observations"
    );

    let events = repository.list_audit_events().await.expect("list events");
    assert_eq!(
        events.len(),
        3,
        "the plan, its validation, and its dedup are all audited"
    );
    assert!(
        events
            .iter()
            .any(|e| e.event_type().as_str() == "brain.observation.deduped"),
        "the deduplicated observation is audited"
    );
}

/// A decision provider that always proposes a restart of `nginx.service`.
fn restart_provider() -> FakeDecisionProvider {
    FakeDecisionProvider::new(BTreeMap::from([
        ("degraded".to_string(), DecisionAnswer::Noul { noul: 0.9 }),
        (
            "remediation".to_string(),
            DecisionAnswer::Choice {
                choice: "restart".into(),
                confidence: 0.9,
                probabilities: BTreeMap::from([
                    ("restart".to_string(), 0.9),
                    ("noop".to_string(), 0.1),
                ]),
            },
        ),
    ]))
}

/// Standard degraded-nginx evidence naming exactly one usable unit.
fn evidence() -> ContextBuilder {
    let mut context = ContextBuilder::new();
    context.evidence("host:a", "service.nginx.state", serde_json::json!("failed"));
    context.evidence("host:a", "unit", serde_json::json!("nginx.service"));
    context
}

/// A dispatch that records the call and reports a successful restart.
fn restarting_dispatch() -> impl Fn(argus_domain::CapabilityRequest) -> Result<Value, DispatchError>
{
    |_request| Ok(serde_json::json!({ "restarted": true }))
}

/// A [`argus_executor::ServiceController`] whose live-state read always fails,
/// to drive the fail-closed validation path.
struct FailingStateController;

impl argus_executor::ServiceController for FailingStateController {
    fn restart(&self, _unit: &str) -> Result<(), argus_executor::ServiceError> {
        Ok(())
    }
    fn stop(&self, _unit: &str) -> Result<(), argus_executor::ServiceError> {
        Ok(())
    }
    fn start(&self, _unit: &str) -> Result<(), argus_executor::ServiceError> {
        Ok(())
    }
    fn is_active(&self, _unit: &str) -> Result<bool, argus_executor::ServiceError> {
        Err(argus_executor::ServiceError::Failed(
            "systemd unavailable".into(),
        ))
    }
}

#[tokio::test]
async fn validation_passes_and_records_when_observed_matches_desired() {
    let repository = InMemoryRepository::new();
    let service = MockServiceController::new();
    service.set_active("nginx.service", true);
    let bus = LocalEventBus::new(16);
    let mut rx = bus.subscribe();

    let plan = diagnose_with(
        restarting_dispatch(),
        DiagnoseContext {
            repository: &repository,
            provider: &restart_provider(),
            dedup: &InMemoryDedup::new(),
            service: &service,
            events: &bus,
        },
        evidence(),
        0.7,
    )
    .await
    .expect("loop runs");
    assert!(plan.is_some(), "a plan clears the threshold");

    // The re-observed active state is recorded as an observation.
    let observations = repository
        .list_observations()
        .await
        .expect("list observations");
    assert!(
        observations
            .iter()
            .any(|o| o.attribute() == "service.active" && o.value() == &ObservedValue::Bool(true)),
        "the re-observed active state is recorded"
    );

    // The validation.passed event is published.
    let event = rx.try_recv().expect("a validation event is published");
    assert_eq!(event.event_type().as_str(), "validation.passed");

    // ...and persisted so the read-only learning pass can read it.
    let audit = repository
        .list_audit_events()
        .await
        .expect("list audit events");
    assert!(
        audit
            .iter()
            .any(|e| e.event_type().as_str() == "validation.passed"),
        "the validation outcome is persisted as an audit event"
    );
}

#[tokio::test]
async fn validation_fails_when_observed_differs_from_desired() {
    let repository = InMemoryRepository::new();
    // The unit is not active, but a restart moves toward active: a mismatch.
    let service = MockServiceController::new();
    let bus = LocalEventBus::new(16);
    let mut rx = bus.subscribe();

    let plan = diagnose_with(
        restarting_dispatch(),
        DiagnoseContext {
            repository: &repository,
            provider: &restart_provider(),
            dedup: &InMemoryDedup::new(),
            service: &service,
            events: &bus,
        },
        evidence(),
        0.7,
    )
    .await
    .expect("loop runs");
    assert!(plan.is_some(), "a plan clears the threshold");

    let event = rx.try_recv().expect("a validation event is published");
    assert_eq!(event.event_type().as_str(), "validation.failed");
}

#[tokio::test]
async fn a_live_state_read_failure_fails_closed() {
    let repository = InMemoryRepository::new();
    let service = FailingStateController;
    let bus = LocalEventBus::new(16);
    let mut rx = bus.subscribe();

    let plan = diagnose_with(
        restarting_dispatch(),
        DiagnoseContext {
            repository: &repository,
            provider: &restart_provider(),
            dedup: &InMemoryDedup::new(),
            service: &service,
            events: &bus,
        },
        evidence(),
        0.7,
    )
    .await
    .expect("loop runs");
    assert!(plan.is_some(), "the plan itself executed");

    // No validation event is published (no fabricated pass).
    assert!(
        rx.try_recv().is_err(),
        "a failed read publishes no validation event"
    );

    // The failure is recorded as an audit event.
    let events = repository.list_audit_events().await.expect("list events");
    assert!(
        events
            .iter()
            .any(|e| e.event_type().as_str() == "brain.validation.read.failed"),
        "the read failure is recorded"
    );
}
