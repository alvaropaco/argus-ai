//! Capability dispatch boundary tests: policy + executor wiring in the daemon,
//! and environment-identity persistence.

use argus_daemon::{Daemon, DispatchError, config::DaemonConfig};
use argus_domain::{CapabilityId, CapabilityRequest, PolicyOutcome, Principal, RequestContext};
use chrono::Utc;
use semver::Version;
use serde_json::json;
use uuid::Uuid;

fn temp_state(name: &str) -> String {
    std::env::temp_dir()
        .join(format!("argus-cap-{name}-{}.db", std::process::id()))
        .to_string_lossy()
        .into_owned()
}

fn context() -> RequestContext {
    RequestContext::new(
        Uuid::new_v4(),
        Version::new(0, 1, 0),
        Principal::new(Some(1000), Some(1000)),
        Utc::now(),
    )
}

fn request(capability: &str) -> CapabilityRequest {
    request_with_args(capability, json!({}))
}

fn request_with_args(capability: &str, arguments: serde_json::Value) -> CapabilityRequest {
    CapabilityRequest::new(
        CapabilityId::new(capability).unwrap(),
        Principal::new(Some(1000), Some(1000)),
        None,
        arguments,
        context(),
    )
}

#[tokio::test]
async fn environment_identity_is_persisted_across_restarts() {
    let path = temp_state("persist");
    let _ = std::fs::remove_file(&path);

    let id = {
        let first = Daemon::init(DaemonConfig {
            state_path: path.clone(),
            ..DaemonConfig::default()
        })
        .await
        .unwrap();
        first.environment_id()
    };

    // A second daemon over the same state path reuses the persisted identity.
    let second = Daemon::init(DaemonConfig {
        state_path: path.clone(),
        ..DaemonConfig::default()
    })
    .await
    .unwrap();
    assert_eq!(second.environment_id(), id);

    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn read_only_capability_flows_through_policy_and_executor() {
    let daemon = Daemon::init(DaemonConfig {
        state_path: temp_state("ro"),
        ..DaemonConfig::default()
    })
    .await
    .unwrap();

    let evidence = daemon
        .authorize_and_execute(request("argus.health.read"))
        .unwrap();
    assert!(evidence["health"]["state"].is_string());
}

#[tokio::test]
async fn service_capability_with_invalid_input_is_refused_before_policy() {
    let daemon = Daemon::init(DaemonConfig {
        state_path: temp_state("invalid-input"),
        ..DaemonConfig::default()
    })
    .await
    .unwrap();

    // `host.service.restart` requires a `unit`, so empty arguments violate its
    // declared input schema and must be refused before policy or the executor.
    let err = daemon
        .authorize_and_execute(request("host.service.restart"))
        .unwrap_err();
    assert!(
        matches!(
            &err,
            DispatchError::InvalidInput(id) if id.as_str() == "host.service.restart"
        ),
        "an input that violates the declared schema must be refused before policy: {err:?}"
    );
}

#[tokio::test]
async fn service_capability_schema_accepts_the_unit_and_rejects_empty_arguments() {
    // The brain emits `arguments = {"unit": <value>}` for a restart. This pins
    // the producer's shape to the daemon's registered `input_schema`, so a drift
    // between the two cannot ship silently. No action is executed.
    let daemon = Daemon::init(DaemonConfig {
        state_path: temp_state("schema"),
        ..DaemonConfig::default()
    })
    .await
    .unwrap();

    let descriptor = daemon
        .registry()
        .get(&CapabilityId::new("host.service.restart").unwrap())
        .expect("host.service.restart is registered");
    let schema = descriptor.input_schema();

    assert!(
        argus_domain::input_matches(schema, &json!({ "unit": "nginx.service" })),
        "the registered schema must accept a unit argument"
    );
    assert!(
        !argus_domain::input_matches(schema, &json!({})),
        "the registered schema must reject arguments missing the required unit"
    );
}

#[tokio::test]
async fn privileged_capability_is_denied_by_policy() {
    let daemon = Daemon::init(DaemonConfig {
        state_path: temp_state("priv"),
        ..DaemonConfig::default()
    })
    .await
    .unwrap();

    // Schema-valid arguments: the refusal must come from policy (the daemon's
    // default policy does not permit the remediation capabilities), not from
    // the earlier input-schema gate.
    let err = daemon
        .authorize_and_execute(request_with_args(
            "host.process.signal",
            json!({ "pid": 4242, "signal": "term" }),
        ))
        .unwrap_err();
    assert!(matches!(err, DispatchError::Denied(PolicyOutcome::Deny)));
}
