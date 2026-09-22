//! `argusd` binary entry point.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use argus_cloud::buffer::ReportQueue;
use argus_cloud::client::supervisor::WssFactory;
use tokio::net::UnixListener;
use tokio::sync::{Mutex, watch};

use argus_daemon::{Daemon, cloud::SupervisorDeps, config::DaemonConfig};
use argus_domain::{DomainEvent, EventType, Severity};
use argus_events::{EventBus, LocalEventBus};
use argus_ipc::serve;
use argus_observability::{LogFormat, init};
use chrono::Utc;
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<()> {
    let config = parse_config()?;
    init(config.log_format, config.otel_endpoint.as_deref())
        .context("failed to initialize tracing")?;

    tracing::info!(socket = %config.socket_path, "starting argusd");

    let events = Arc::new(LocalEventBus::new(64));
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

    let _cloud_stop = spawn_cloud_supervisor(&daemon, &config, Arc::clone(&events));

    let handler = {
        let daemon = Arc::clone(&daemon);
        move |request, principal| {
            let daemon = Arc::clone(&daemon);
            async move { argus_daemon::handler::handle(&daemon, principal, request).await }
        }
    };

    serve(listener, handler).await.context("IPC server failed")
}

/// Starts cloud supervision as an isolated task.
///
/// The returned sender stops it, and dropping it stops supervision too. Nothing
/// here is awaited by the daemon, so an absent, unreachable, or misconfigured
/// cloud can never delay startup or local operation (FR-013).
fn spawn_cloud_supervisor(
    daemon: &Arc<Daemon>,
    config: &DaemonConfig,
    events: Arc<LocalEventBus>,
) -> watch::Sender<bool> {
    let (stop, stop_rx) = watch::channel(false);
    let deps = SupervisorDeps {
        config: config.cloud.clone(),
        secrets: daemon.secrets().clone(),
        repository: Arc::clone(daemon.repository()),
        tracker: daemon.tracker(),
        factory: Arc::new(WssFactory),
        events,
        queue: Arc::new(Mutex::new(ReportQueue::new(
            config.cloud.report_buffer_max_records,
        ))),
        capabilities: Arc::new(daemon.registry().list().cloned().collect()),
        managed_settings: Arc::new(daemon.managed_settings().clone()),
        environment_id: daemon.environment_id(),
        policy: daemon.cloud_policy(),
        executor: daemon.cloud_executor(),
        approvals: Arc::new(argus_policy::ApprovalStore::new()),
        limiter: Arc::new(argus_daemon::privileged::PrivilegedLimiter::new(
            config.cloud.max_concurrent_privileged,
        )),
    };
    tokio::spawn(argus_daemon::cloud::supervise(deps, stop_rx));
    stop
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
    let args: Vec<String> = std::env::args().skip(1).collect();

    let loaded = argus_daemon::config::load(explicit_config_path(&args)?.as_deref())
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    for warning in &loaded.warnings {
        eprintln!("warning: {warning}");
    }
    if let Some(path) = &loaded.source_path {
        eprintln!("loaded configuration from {}", path.display());
    }

    // Flags are applied on top of the file, so an operator always retains a
    // local override even when a cloud-managed value is in effect.
    let mut config = loaded.config;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--config" => i += 2,
            "--socket" => {
                config.socket_path = value_at(&args, i, "--socket")?;
                i += 2;
            }
            "--state" => {
                config.state_path = value_at(&args, i, "--state")?;
                i += 2;
            }
            "--environment" => {
                config.environment_name = value_at(&args, i, "--environment")?;
                i += 2;
            }
            "--authorized-uids" => {
                config.authorized_uids =
                    Some(parse_uids(&value_at(&args, i, "--authorized-uids")?)?);
                i += 2;
            }
            "--otel-endpoint" => {
                config.otel_endpoint = Some(value_at(&args, i, "--otel-endpoint")?);
                i += 2;
            }
            "--log-format" => {
                let raw = value_at(&args, i, "--log-format")?;
                config.log_format = match raw.as_str() {
                    "text" => LogFormat::Text,
                    "json" => LogFormat::Json,
                    other => anyhow::bail!("unknown log format '{other}' (expected text|json)"),
                };
                i += 2;
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

fn explicit_config_path(args: &[String]) -> Result<Option<PathBuf>> {
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--config" {
            let value = args.get(i + 1).context("--config requires a value")?;
            return Ok(Some(PathBuf::from(value)));
        }
        i += 1;
    }
    Ok(None)
}

fn value_at(args: &[String], index: usize, flag: &str) -> Result<String> {
    args.get(index + 1)
        .cloned()
        .with_context(|| format!("{flag} requires a value"))
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
        "Usage: argusd [--config PATH] [--socket PATH] [--state PATH] [--environment NAME] \
         [--authorized-uids UID,..] [--otel-endpoint URL] [--log-format text|json]\n\
         \n\
         Without --config, ./argus.toml then /etc/argus/argus.toml are used if present."
    );
}
