//! The loop's wiring: a fake decision provider through an injected dispatch and
//! an in-memory repository, proving execution and recording without a full daemon.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use argus_ai_core::decision::adapters::FakeDecisionProvider;
use argus_ai_core::decision::context::ContextBuilder;
use argus_ai_core::decision::types::DecisionAnswer;
use argus_daemon::{DispatchError, diagnose_with};
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

    let plan = diagnose_with(dispatch, &repository, &provider, evidence, 0.7)
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
    assert_eq!(events.len(), 1, "an audit event was recorded");

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

    let plan = diagnose_with(dispatch, &repository, &provider, evidence, 0.3)
        .await
        .expect("loop runs against a live laya-serve");
    assert!(plan.is_some(), "laya drove a plan");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
