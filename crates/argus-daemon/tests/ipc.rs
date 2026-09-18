//! End-to-end IPC tests: full request/response round-trips over a Unix socket.

use std::sync::Arc;

use argus_daemon::{Daemon, config::DaemonConfig};
use argus_ipc::{Client, ErrorCode, Operation, serve};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use uuid::Uuid;

fn temp_socket(name: &str) -> String {
    std::env::temp_dir()
        .join(format!("argus-test-{name}-{}.sock", std::process::id()))
        .to_string_lossy()
        .into_owned()
}

fn temp_state(name: &str) -> String {
    std::env::temp_dir()
        .join(format!("argus-test-{name}-{}.db", std::process::id()))
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
        move |request, principal| argus_daemon::handler::handle(daemon.as_ref(), principal, request)
    };
    tokio::spawn(async move {
        let _ = serve(listener, handler).await;
    });
    path
}

fn cleanup(path: &str) {
    let _ = std::fs::remove_file(path);
}

/// Sends a raw frame and reads a single response line.
async fn raw_round_trip(path: &str, line: &str) -> Value {
    let stream = UnixStream::connect(path).await.expect("connect");
    let mut writer = stream;
    writer
        .write_all(format!("{line}\n").as_bytes())
        .await
        .unwrap();
    // Reuse the same stream for reading; a half must be read via a reader.
    // Since we wrote into `writer`, wrap it in a reader by taking ownership.
    let mut reader = BufReader::new(writer);
    let mut buf = String::new();
    reader.read_line(&mut buf).await.unwrap();
    serde_json::from_str(&buf).expect("valid JSON response")
}

#[tokio::test]
async fn health_round_trip() {
    let path = spawn_server(test_config("health")).await;

    let mut client = Client::connect(&path).await.unwrap();
    let resp = client
        .request(Operation::HealthGet, Uuid::new_v4(), json!({}))
        .await
        .unwrap();

    assert!(resp.ok);
    assert_eq!(resp.result.unwrap()["state"].as_str(), Some("ready"));
    cleanup(&path);
}

#[tokio::test]
async fn status_contains_environment_id_and_uptime() {
    let path = spawn_server(test_config("status")).await;

    let mut client = Client::connect(&path).await.unwrap();
    let resp = client
        .request(Operation::StatusGet, Uuid::new_v4(), json!({}))
        .await
        .unwrap();

    assert!(resp.ok);
    let result = resp.result.unwrap();
    assert!(result["environment_id"].is_string());
    assert!(result["uptime_seconds"].is_u64());
    cleanup(&path);
}

#[tokio::test]
async fn capabilities_lists_bootstrap_set() {
    let path = spawn_server(test_config("caps")).await;

    let mut client = Client::connect(&path).await.unwrap();
    let resp = client
        .request(Operation::CapabilitiesList, Uuid::new_v4(), json!({}))
        .await
        .unwrap();

    assert!(resp.ok);
    let mut caps: Vec<String> = resp
        .result
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    caps.sort();
    assert_eq!(
        caps,
        vec![
            "argus.config.read".to_string(),
            "argus.health.read".to_string(),
            "argus.plugins.list".to_string(),
            "host.status.read".to_string(),
        ]
    );
    cleanup(&path);
}

#[tokio::test]
async fn unknown_operation_is_rejected() {
    let path = spawn_server(test_config("unknown")).await;

    let line = json!({
        "protocol_version": "0.1.0",
        "correlation_id": Uuid::new_v4(),
        "operation": "host.process.signal",
        "payload": {}
    })
    .to_string();

    let resp = raw_round_trip(&path, &line).await;
    assert_eq!(resp["ok"], json!(false));
    assert_eq!(resp["error"]["code"], json!("UNKNOWN_OPERATION"));
    cleanup(&path);
}

#[tokio::test]
async fn unauthorized_peer_is_rejected() {
    let config = DaemonConfig {
        socket_path: temp_socket("unauth"),
        state_path: temp_state("unauth"),
        authorized_uids: Some(vec![9999]),
        ..DaemonConfig::default()
    };
    let path = spawn_server(config).await;

    let mut client = Client::connect(&path).await.unwrap();
    let resp = client
        .request(Operation::HealthGet, Uuid::new_v4(), json!({}))
        .await
        .unwrap();

    assert!(!resp.ok);
    assert_eq!(resp.error.unwrap().code, ErrorCode::Unauthorized);
    cleanup(&path);
}

#[tokio::test]
async fn malformed_frame_is_rejected() {
    let path = spawn_server(test_config("malformed")).await;

    let resp = raw_round_trip(&path, "{not json").await;
    assert_eq!(resp["ok"], json!(false));
    assert_eq!(resp["error"]["code"], json!("MALFORMED"));
    cleanup(&path);
}

#[tokio::test]
async fn unsupported_version_is_rejected() {
    let path = spawn_server(test_config("version")).await;

    let line = json!({
        "protocol_version": "99.0.0",
        "correlation_id": Uuid::new_v4(),
        "operation": "health.get",
        "payload": {}
    })
    .to_string();

    let resp = raw_round_trip(&path, &line).await;
    assert_eq!(resp["ok"], json!(false));
    assert_eq!(resp["error"]["code"], json!("UNSUPPORTED_VERSION"));
    cleanup(&path);
}
