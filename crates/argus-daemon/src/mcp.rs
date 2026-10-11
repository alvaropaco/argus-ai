//! The MCP integration wiring (spec 014, ADR-0044).
//!
//! The composition root between the daemon and the `argus-mcp` runtime: the
//! startup connect-with-timeout whose discovered tools register beside the
//! bootstrap set, the executor port the `mcp.*` capabilities execute
//! through, and the per-server lifecycle task (probe, disconnect, reconnect
//! with backoff — one task per server, never blocking the loop or the other
//! servers).

use std::sync::Arc;
use std::time::Duration;

use argus_executor::McpToolController;
use argus_mcp::McpClientRuntime;

use crate::config::{McpServerConfig, McpTransportConfig};

/// How often a connected server is probed (a cheap `tools/list`) so a crash
/// is noticed without waiting for a call to arrive.
const PROBE_INTERVAL: Duration = Duration::from_secs(10);

/// The reconnect backoff: 1, 2, 4, … seconds, capped — a flapping server
/// retries forever but never hot-loops.
fn backoff_delay(attempt: u32) -> Duration {
    let shift = attempt.min(6);
    Duration::from_secs(1u64 << shift)
}

/// The daemon's MCP handle: the domain seam, the executor port, and — for a
/// production daemon — the concrete rmcp client that owns the transports.
pub struct McpWiring {
    /// The domain [`argus_domain::McpRuntime`] seam: the descriptor
    /// bookkeeping every registration flows through.
    seam: Arc<dyn argus_domain::McpRuntime>,
    /// The executor port: what an authorized `mcp.*` step actually calls.
    controller: Arc<dyn McpToolController>,
    /// The concrete client, when this wiring owns the real transports (a
    /// test's injected fake runs without one).
    client: Option<Arc<McpClientRuntime>>,
    /// The ENABLED configured servers.
    servers: Vec<McpServerConfig>,
}

impl McpWiring {
    /// The production wiring: one rmcp client runtime serves as both the
    /// domain seam and the executor port.
    pub fn production(servers: Vec<McpServerConfig>) -> Arc<Self> {
        let client = Arc::new(McpClientRuntime::new());
        let seam: Arc<dyn argus_domain::McpRuntime> = client.clone();
        let controller: Arc<dyn McpToolController> = client.clone();
        Self::injected(seam, controller, Some(client), servers)
    }

    /// A wiring around injected implementations — the daemon tests' seam: a
    /// deterministic fake runtime and controller, no subprocess.
    pub fn fake(
        seam: Arc<dyn argus_domain::McpRuntime>,
        controller: Arc<dyn McpToolController>,
    ) -> Arc<Self> {
        Self::injected(seam, controller, None, Vec::new())
    }

    fn injected(
        seam: Arc<dyn argus_domain::McpRuntime>,
        controller: Arc<dyn McpToolController>,
        client: Option<Arc<McpClientRuntime>>,
        servers: Vec<McpServerConfig>,
    ) -> Arc<Self> {
        Arc::new(Self {
            seam,
            controller,
            client,
            servers,
        })
    }

    /// The domain seam, for surfaces that only read the adopted table.
    pub fn seam(&self) -> Arc<dyn argus_domain::McpRuntime> {
        self.seam.clone()
    }

    /// The executor port the remediation executor routes `mcp.*` steps to.
    pub fn controller(&self) -> Arc<dyn McpToolController> {
        self.controller.clone()
    }

    /// The startup connect: every enabled server connects CONCURRENTLY, each
    /// bounded by its own configured timeout, and the adopted sessions stay
    /// live in the runtime's slots — the lifecycle task inherits them
    /// (probing and reconnects) without a redundant second connect. A server
    /// that fails is logged and retried by the lifecycle, never fatal for
    /// startup (spec 014 FR-004); its discovered tools register into
    /// `registry` here.
    ///
    /// With an injected fake (no client) the seam's own descriptor table IS
    /// the startup result — deterministic, no I/O.
    pub async fn connect_startup(&self, registry: &mut argus_domain::CapabilityRegistry) {
        let Some(client) = &self.client else {
            for descriptor in self.seam.descriptors() {
                if let Err(error) = registry.register(descriptor) {
                    tracing::warn!(error = %error, "an mcp tool could not register");
                }
            }
            return;
        };
        let handles: Vec<_> = self
            .servers
            .iter()
            .map(|server| {
                let client = client.clone();
                let server = server.clone();
                tokio::spawn(async move {
                    let spec = server.spec();
                    let risk_overrides = server.risk_overrides();
                    let result = client
                        .connect(&spec, &risk_overrides, server.tools.as_deref())
                        .await;
                    (server, result)
                })
            })
            .collect();
        for handle in handles {
            let (server, result) = handle.await.expect("the mcp startup task cannot panic");
            match result {
                Ok(descriptors) => {
                    tracing::info!(
                        server = %server.name,
                        tools = descriptors.len(),
                        "mcp server connected; tools adopted"
                    );
                    for descriptor in descriptors {
                        if let Err(error) = registry.register(descriptor) {
                            tracing::warn!(error = %error, "an mcp tool could not register");
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        server = %server.name,
                        error = %error,
                        "mcp server failed to connect at startup; the lifecycle task will retry"
                    );
                }
            }
        }
    }

    /// Spawns one lifecycle task per enabled server: connect → probe while
    /// healthy → disconnect → reconnect with backoff. Late-discovered tools
    /// register through the daemon so the capability surface stays truthful.
    /// The default lifecycle policy is keep-registered-but-failing: a down
    /// server's tools stay registered and fail typed (visible failure, spec
    /// 014 FR-004).
    pub fn spawn_lifecycle(self: &Arc<Self>, daemon: Arc<crate::runtime::Daemon>) {
        for server in self.servers.clone() {
            let client = self
                .client
                .clone()
                .expect("the lifecycle task only runs on a production wiring");
            let daemon = Arc::clone(&daemon);
            tokio::spawn(async move {
                let mut attempt: u32 = 0;
                loop {
                    // The startup connect usually left this slot live: inherit
                    // it and go straight to probing — never a second connect
                    // over a healthy session.
                    if !client.has_connection(&server.name).await {
                        let spec = server.spec();
                        let risk_overrides = server.risk_overrides();
                        match client
                            .connect(&spec, &risk_overrides, server.tools.as_deref())
                            .await
                        {
                            Ok(descriptors) => {
                                let fresh = daemon.register_mcp_descriptors(descriptors);
                                if fresh > 0 {
                                    daemon.refresh_capability_surface();
                                }
                                tracing::info!(
                                    server = %server.name,
                                    fresh_tools = fresh,
                                    "mcp server connected"
                                );
                                attempt = 0;
                            }
                            Err(error) => {
                                tracing::warn!(
                                    server = %server.name,
                                    error = %error,
                                    "mcp server unreachable; reconnecting with backoff"
                                );
                                attempt += 1;
                                tokio::time::sleep(backoff_delay(attempt)).await;
                                continue;
                            }
                        }
                    }
                    // Probe until the server goes down; a probe failure
                    // disconnects the slot so calls fail typed from that
                    // moment.
                    loop {
                        tokio::time::sleep(PROBE_INTERVAL).await;
                        let risk_overrides = server.risk_overrides();
                        match client
                            .refresh(&server.name, &risk_overrides, server.tools.as_deref())
                            .await
                        {
                            Ok(descriptors) => {
                                let fresh = daemon.register_mcp_descriptors(descriptors);
                                if fresh > 0 {
                                    // Late-discovered tools reach the cloud
                                    // surface without a restart (spec 014).
                                    daemon.refresh_capability_surface();
                                }
                            }
                            Err(error) => {
                                tracing::warn!(
                                    server = %server.name,
                                    error = %error,
                                    "mcp server went down; its tools fail typed until it returns"
                                );
                                client.disconnect(&server.name).await;
                                break;
                            }
                        }
                    }
                    attempt += 1;
                    tokio::time::sleep(backoff_delay(attempt)).await;
                }
            });
        }
    }
}

impl McpServerConfig {
    /// The runtime-facing server spec.
    pub fn spec(&self) -> argus_mcp::ServerSpec {
        argus_mcp::ServerSpec {
            name: self.name.clone(),
            transport: match &self.transport {
                McpTransportConfig::Stdio { command, args } => argus_mcp::TransportSpec::Stdio {
                    command: command.clone(),
                    args: args.clone(),
                },
                McpTransportConfig::Http { url } => {
                    argus_mcp::TransportSpec::Http { url: url.clone() }
                }
            },
            timeout: Duration::from_secs(u64::from(self.timeout_seconds)),
        }
    }

    /// The per-tool trust overrides (the operator's config, never the
    /// server's self-report).
    pub fn risk_overrides(&self) -> Vec<argus_mcp::ToolOverride> {
        self.overrides
            .iter()
            .map(|tool| argus_mcp::ToolOverride::new(tool.name.clone(), tool.risk))
            .collect()
    }
}

/// The enabled servers only — an unlisted or disabled server contributes
/// nothing (spec 014 FR-001).
pub fn enabled_servers(config: &crate::config::McpConfig) -> Vec<McpServerConfig> {
    config
        .servers
        .iter()
        .filter(|server| server.enabled)
        .cloned()
        .collect()
}
