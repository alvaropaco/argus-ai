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
    CapabilityRequest::new(
        CapabilityId::new(capability).unwrap(),
        Principal::new(Some(1000), Some(1000)),
        None,
        json!({}),
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
async fn privileged_capability_is_denied_by_policy() {
    let daemon = Daemon::init(DaemonConfig {
        state_path: temp_state("priv"),
        ..DaemonConfig::default()
    })
    .await
    .unwrap();

    let err = daemon
        .authorize_and_execute(request("host.process.signal"))
        .unwrap_err();
    assert!(matches!(err, DispatchError::Denied(PolicyOutcome::Deny)));
}
