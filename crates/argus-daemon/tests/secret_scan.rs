//! End-to-end secret-hygiene scan (T100).
//!
//! Asserts that nothing secret reaches a non-secret surface an operator can read:
//! not the IPC responses, and not the SQLite state file. This is the automated
//! half of SC-011. The other half — that a credential *delivered* by the cloud
//! lands only in the secret store — is covered by the daemon's own unit tests,
//! which can plant a credential and inspect the settings file and the store.

use std::sync::Arc;

use argus_daemon::{Daemon, config::DaemonConfig};
use argus_ipc::{Client, Operation, serve};
use serde_json::{Value, json};
use tokio::net::UnixListener;
use uuid::Uuid;

fn temp_socket(name: &str) -> String {
    std::env::temp_dir()
        .join(format!("argus-secret-{name}-{}.sock", std::process::id()))
        .to_string_lossy()
        .into_owned()
}

fn temp_state(name: &str) -> String {
    std::env::temp_dir()
        .join(format!("argus-secret-{name}-{}.db", std::process::id()))
        .to_string_lossy()
        .into_owned()
}

fn test_config(name: &str) -> DaemonConfig {
    DaemonConfig {
        socket_path: temp_socket(name),
        state_path: temp_state(name),
        ..DaemonConfig::default()
    }
}

async fn spawn_server(config: DaemonConfig) -> String {
    let path = config.socket_path.clone();
    let daemon = Arc::new(Daemon::init(config).await.expect("daemon init"));
    let listener = UnixListener::bind(&path).expect("bind test socket");
    let handler = {
        let daemon = Arc::clone(&daemon);
        move |request, principal| {
            let daemon = Arc::clone(&daemon);
            async move { argus_daemon::handler::handle(daemon.as_ref(), principal, request).await }
        }
    };
    tokio::spawn(async move {
        let _ = serve(listener, handler).await;
    });
    path
}

fn cleanup(path: &str) {
    let _ = std::fs::remove_file(path);
}

/// Field names that would hold a credential if one ever crossed this surface.
const SECRET_FIELDS: [&str; 4] = [
    "session_token",
    "session_proof",
    "rotation_token",
    "api_token",
];

/// Whether a string has the shape of a credential rather than of configuration.
///
/// Deliberately shape-based: a credential leaks just as badly under an
/// innocuous key, so scanning values catches what scanning key names would miss.
fn looks_like_a_credential(text: &str) -> bool {
    if text.starts_with("sk-") || text.starts_with("ARGUS-") {
        return true;
    }
    // A long, dense, opaque string is more likely a token than a setting.
    text.len() >= 64
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '+')
}

fn collect_credential_shaped(value: &Value, path: &str, findings: &mut Vec<String>) {
    match value {
        Value::String(text) => {
            if looks_like_a_credential(text) {
                findings.push(format!("{path} = {text}"));
            }
        }
        Value::Object(map) => {
            for (key, nested) in map {
                if SECRET_FIELDS.contains(&key.as_str()) {
                    findings.push(format!("{path}.{key} is a secret-named field"));
                }
                collect_credential_shaped(nested, &format!("{path}.{key}"), findings);
            }
        }
        Value::Array(items) => {
            for (index, nested) in items.iter().enumerate() {
                collect_credential_shaped(nested, &format!("{path}[{index}]"), findings);
            }
        }
        _ => {}
    }
}

#[tokio::test]
async fn no_ipc_response_carries_a_credential() {
    let config = test_config("ipc-scan");
    let socket = spawn_server(config.clone()).await;

    let mut client = Client::connect(&socket).await.expect("connect");

    for operation in [
        Operation::CloudStatus,
        Operation::ConfigGet,
        Operation::StatusGet,
        Operation::CapabilitiesList,
    ] {
        let response = client
            .request(operation, Uuid::new_v4(), json!({}))
            .await
            .expect("request");

        let mut findings = Vec::new();
        collect_credential_shaped(
            &serde_json::to_value(&response).expect("serialise"),
            &format!("{operation:?}"),
            &mut findings,
        );

        assert!(
            findings.is_empty(),
            "a response must be safe to paste into a ticket, but {operation:?} exposed: {findings:?}"
        );
    }

    cleanup(&socket);
    cleanup(&config.state_path);
}

#[tokio::test]
async fn the_state_file_holds_no_credential() {
    let config = test_config("state-scan");
    let socket = spawn_server(config.clone()).await;

    let mut client = Client::connect(&socket).await.expect("connect");
    let _ = client
        .request(Operation::CloudStatus, Uuid::new_v4(), json!({}))
        .await;

    let bytes = std::fs::read(&config.state_path).expect("state file exists");
    let text = String::from_utf8_lossy(&bytes);

    for marker in SECRET_FIELDS.iter().chain(["sk-", "ARGUS-"].iter()) {
        assert!(
            !text.contains(marker),
            "the state file contains `{marker}`; credentials belong in the secret store (mode 0600), not in SQLite"
        );
    }

    cleanup(&socket);
    cleanup(&config.state_path);
}

#[tokio::test]
async fn status_reports_absence_rather_than_an_error_when_nothing_is_enrolled() {
    // FR-056: an operator asking a question must get an answer, not a failure.
    let config = test_config("status-answer");
    let socket = spawn_server(config.clone()).await;

    let mut client = Client::connect(&socket).await.expect("connect");
    let response = client
        .request(Operation::CloudStatus, Uuid::new_v4(), json!({}))
        .await
        .expect("request");

    assert!(response.ok, "status must succeed: {:?}", response.error);
    let result = response.result.expect("a result");
    assert_eq!(result["state"], "not_configured");
    assert_eq!(result["enabled"], false);

    cleanup(&socket);
    cleanup(&config.state_path);
}
