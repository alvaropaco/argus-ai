//! `argusd` binary entry point.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::net::UnixListener;

use argus_daemon::{Daemon, config::DaemonConfig};
use argus_domain::{DomainEvent, EventType, Severity};
use argus_events::{EventBus, LocalEventBus};
use argus_ipc::serve;
use argus_observability::{LogFormat, init};
use chrono::Utc;
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<()> {
    let config = parse_config()?;
    init(config.log_format).context("failed to initialize tracing")?;

    tracing::info!(socket = %config.socket_path, "starting argusd");

    let events = LocalEventBus::new(64);
    let _ = events
        .publish(&event(
            "argus.started",
            Severity::Info,
            serde_json::json!({ "version": env!("CARGO_PKG_VERSION") }),
        ))
        .await;

    prepare_state_dir(&config.state_path).await?;
    let daemon = match Daemon::init(config.clone()).await {
        Ok(daemon) => Arc::new(daemon),
        Err(err) => {
            let _ = events
                .publish(&event(
                    "argus.degraded",
                    Severity::Error,
                    serde_json::json!({ "reason": err.to_string() }),
                ))
                .await;
            return Err(err.into());
        }
    };

    let listener = bind(&config.socket_path).await?;
    let _ = events
        .publish(&event("argus.ready", Severity::Info, serde_json::json!({})))
        .await;

    tracing::info!(
        socket = %config.socket_path,
        environment = %daemon.environment_id(),
        "argusd ready"
    );

    let handler = {
        let daemon = Arc::clone(&daemon);
        move |request, principal| argus_daemon::handler::handle(&daemon, principal, request)
    };

    serve(listener, handler).await.context("IPC server failed")
}

fn event(event_type: &str, severity: Severity, payload: serde_json::Value) -> DomainEvent {
    DomainEvent::new(
        Uuid::new_v4(),
        EventType::new(event_type).expect("valid event type"),
        Utc::now(),
        "argusd",
        "argusd",
        severity,
        None,
        None,
        payload,
    )
}

async fn bind(path: &str) -> Result<UnixListener> {
    if let Some(parent) = Path::new(path).parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    if tokio::fs::metadata(path).await.is_ok() {
        tracing::warn!(socket = %path, "removing stale socket file");
        tokio::fs::remove_file(path).await?;
    }
    Ok(UnixListener::bind(path)?)
}

async fn prepare_state_dir(path: &str) -> Result<()> {
    if let Some(parent) = Path::new(path).parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    Ok(())
}

fn parse_config() -> Result<DaemonConfig> {
    let mut config = DaemonConfig::default();
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--socket" => {
                config.socket_path = args.next().context("--socket requires a value")?;
            }
            "--state" => {
                config.state_path = args.next().context("--state requires a value")?;
            }
            "--environment" => {
                config.environment_name = args.next().context("--environment requires a value")?;
            }
            "--authorized-uids" => {
                let raw = args.next().context("--authorized-uids requires a value")?;
                config.authorized_uids = Some(parse_uids(&raw)?);
            }
            "--log-format" => {
                let raw = args.next().context("--log-format requires a value")?;
                config.log_format = match raw.as_str() {
                    "text" => LogFormat::Text,
                    "json" => LogFormat::Json,
                    other => anyhow::bail!("unknown log format '{other}' (expected text|json)"),
                };
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            other => anyhow::bail!("unknown argument '{other}'"),
        }
    }

    Ok(config)
}

fn parse_uids(raw: &str) -> Result<Vec<u32>> {
    raw.split(',')
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.parse::<u32>()
                .map_err(|_| anyhow::anyhow!("invalid uid '{s}'"))
        })
        .collect()
}

fn print_usage() {
    eprintln!(
        "Usage: argusd [--socket PATH] [--state PATH] [--environment NAME] [--authorized-uids UID,..] [--log-format text|json]"
    );
}
