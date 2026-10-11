//! The MCP integration wiring (spec 014, ADR-0044; live reconcile per spec 015).
//!
//! The composition root between the daemon and the `argus-mcp` runtime: the
//! startup connect-with-timeout whose discovered tools register beside the
//! bootstrap set, the executor port the `mcp.*` capabilities execute
//! through, the per-server lifecycle loop (probe, disconnect, reconnect with
//! backoff — one concurrent visit per server, never blocking the loop or the
//! other servers), and — spec 015 — the reconcile entry a delivered `mcp`
//! configuration applies through: added servers connect, removed or changed
//! ones disconnect, unchanged ones are untouched.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

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

/// One server's reconnect state: how many times the current outage has
/// retried, and when the next attempt may run. Shared between the lifecycle
/// loop and [`McpWiring::apply_servers`], so a fresh delivery clears the
/// stale delay of a server it touched instead of inheriting it.
#[derive(Debug, Clone, Copy)]
struct RetryState {
    attempt: u32,
    not_before: Instant,
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
    /// The desired ENABLED servers with their OWNER (file vs cloud) —
    /// replaced in place by [`McpWiring::apply_servers`] (spec 015), so the
    /// lifecycle loop adopts the reconciled topology on its next pass
    /// without a restart. Ownership is what keeps a delivery from severing
    /// file-declared servers the document omits (ADR-0045 §4).
    servers: Arc<std::sync::RwLock<Vec<DesiredServer>>>,
    /// Per-server reconnect state for the lifecycle (see [`RetryState`]).
    retries: Arc<std::sync::Mutex<HashMap<String, RetryState>>>,
    /// Serializes a delivery's reconcile against the lifecycle's passes: an
    /// in-flight pass must be done before anything is disconnected, so a
    /// removed server cannot be reconnected by a stale pass task (and left
    /// connected-but-unmanaged) and a changed server cannot have its new-spec
    /// session replaced by an old-spec one.
    apply_order: Arc<tokio::sync::Mutex<()>>,
}

/// One server in the desired topology, and who declared it.
#[derive(Debug, Clone, PartialEq)]
pub struct DesiredServer {
    pub server: McpServerConfig,
    /// `true` — a cloud delivery declared this name: the NEXT delivery may
    /// remove or replace it. `false` — the operator's file declared it: it
    /// survives every delivery that does not re-declare its name.
    pub from_cloud: bool,
}

/// The live per-server state behind config-state (spec 015 FR-004): name,
/// coarse state, and the adopted tool count — presence and counts only,
/// never tool payloads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerRuntimeState {
    pub name: String,
    /// `connected` | `retrying` | `connecting` — a desired server without a
    /// live slot is on the lifecycle's retry path (`retrying` once a connect
    /// has failed, `connecting` before the first attempt).
    pub state: &'static str,
    pub tools: usize,
}

/// What changed between the running topology and a delivered document
/// (spec 015 FR-002 with ADR-0045 §4's ownership rule): the names to
/// connect (added, or changed — after their old slot is disconnected), the
/// names to remove, and the desired list the wiring adopts. Unchanged names
/// appear in neither list — no session churn. Ownership: a delivery
/// replaces or removes only CLOUD-declared servers (by name); a
/// FILE-declared server the document omits is carried forward untouched,
/// and a re-declared name becomes cloud-owned (cloud supersedes file).
fn reconcile_plan(
    current: &[DesiredServer],
    delivered: &[McpServerConfig],
) -> (
    Vec<McpServerConfig>,
    Vec<McpServerConfig>,
    Vec<DesiredServer>,
) {
    let mut to_connect = Vec::new();
    let mut to_remove = Vec::new();
    let mut desired = Vec::new();

    for held in current {
        match delivered
            .iter()
            .find(|server| server.name == held.server.name)
        {
            // The cloud re-declares the name: it owns the server from here
            // on. A config delta is "changed" — any transport, command, url,
            // args, timeout, allowlist, or override difference — and the old
            // slot is disconnected before the new spec connects.
            Some(server) => {
                if *server != held.server {
                    to_remove.push(held.server.clone());
                    to_connect.push(server.clone());
                }
                desired.push(DesiredServer {
                    server: server.clone(),
                    from_cloud: true,
                });
            }
            // The document omits the name: only a cloud-declared server is
            // removed by that. A file-declared server survives untouched —
            // it was never the delivery's to take away.
            None => {
                if held.from_cloud {
                    to_remove.push(held.server.clone());
                } else {
                    desired.push(held.clone());
                }
            }
        }
    }
    // A name the running topology has never seen is added.
    for server in delivered {
        if !current.iter().any(|held| held.server.name == server.name) {
            to_connect.push(server.clone());
            desired.push(DesiredServer {
                server: server.clone(),
                from_cloud: true,
            });
        }
    }

    (to_connect, to_remove, desired)
}

/// The per-server result of one reconcile (spec 015 FR-002): the
/// configuration is applied even when a server is down — a failed connect
/// lands in `failed` and the lifecycle retries with backoff while the rest
/// go live. The operator sees the state, not a refusal.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct McpReconcileReport {
    pub connected: Vec<String>,
    pub removed: Vec<String>,
    pub failed: Vec<(String, String)>,
}

impl McpWiring {
    /// The production wiring: one rmcp client runtime serves as both the
    /// domain seam and the executor port. All `servers` are file-declared.
    pub fn production(servers: Vec<McpServerConfig>) -> Arc<Self> {
        Self::production_with_ownership(servers, Vec::new())
    }

    /// The production wiring with the startup ownership split (spec 015
    /// FR-005): the file-declared servers and the cloud-delivered ones
    /// (already merged per `McpConfig::merged_over` — a name in both is
    /// cloud-owned, which is what makes cloud supersede file).
    pub fn production_with_ownership(
        file: Vec<McpServerConfig>,
        cloud: Vec<McpServerConfig>,
    ) -> Arc<Self> {
        let client = Arc::new(McpClientRuntime::new());
        let seam: Arc<dyn argus_domain::McpRuntime> = client.clone();
        let controller: Arc<dyn McpToolController> = client.clone();
        Self::injected(seam, controller, Some(client), file, cloud)
    }

    /// A wiring around injected implementations — the daemon tests' seam: a
    /// deterministic fake runtime and controller, no subprocess.
    pub fn fake(
        seam: Arc<dyn argus_domain::McpRuntime>,
        controller: Arc<dyn McpToolController>,
    ) -> Arc<Self> {
        Self::injected(seam, controller, None, Vec::new(), Vec::new())
    }

    /// The tests' ownership seam: file- and cloud-declared seeds over an
    /// injected runtime.
    fn injected(
        seam: Arc<dyn argus_domain::McpRuntime>,
        controller: Arc<dyn McpToolController>,
        client: Option<Arc<McpClientRuntime>>,
        file: Vec<McpServerConfig>,
        cloud: Vec<McpServerConfig>,
    ) -> Arc<Self> {
        // A name in both lists takes the CLOUD config (cloud supersedes file
        // — the same supersede the restart's merge performs) and is
        // cloud-owned; the rest keep their owner. Order: file first, then
        // cloud-only names.
        let mut desired: Vec<DesiredServer> = file
            .into_iter()
            .map(|server| DesiredServer {
                server,
                from_cloud: false,
            })
            .collect();
        for server in cloud {
            match desired
                .iter_mut()
                .find(|held| held.server.name == server.name)
            {
                Some(held) => {
                    held.server = server;
                    held.from_cloud = true;
                }
                None => desired.push(DesiredServer {
                    server,
                    from_cloud: true,
                }),
            }
        }
        Arc::new(Self {
            seam,
            controller,
            client,
            servers: Arc::new(std::sync::RwLock::new(desired)),
            retries: Arc::new(std::sync::Mutex::new(HashMap::new())),
            apply_order: Arc::new(tokio::sync::Mutex::new(())),
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

    /// The desired enabled servers (a clone — the caller never holds the
    /// lock). The startup connect seeds it; [`McpWiring::apply_servers`]
    /// replaces it; the lifecycle reads a fresh snapshot every pass.
    pub fn desired_servers(&self) -> Vec<McpServerConfig> {
        self.desired().into_iter().map(|held| held.server).collect()
    }

    /// The desired servers with their ownership.
    fn desired(&self) -> Vec<DesiredServer> {
        self.servers
            .read()
            .expect("mcp desired-servers lock is not poisoned")
            .clone()
    }

    /// The live per-server state (spec 015 FR-004): for every desired
    /// server, whether its slot is live, whether it is on the retry path,
    /// and how many adopted tools it currently contributes. Reads-only, so
    /// it is safe at any moment; the lifecycle owns the truth it reports.
    pub async fn server_states(&self) -> Vec<McpServerRuntimeState> {
        let mut states = Vec::new();
        for held in self.desired_servers() {
            let connected = match self.client.as_ref() {
                Some(client) => client.has_connection(&held.name).await,
                None => false,
            };
            let prefix = format!("mcp.{}.", held.name);
            let tools = self
                .seam
                .capabilities()
                .iter()
                .filter(|id| id.as_str().starts_with(&prefix))
                .count();
            let state = if connected {
                "connected"
            } else if self.retry_state(&held.name).is_some() {
                "retrying"
            } else {
                "connecting"
            };
            states.push(McpServerRuntimeState {
                name: held.name,
                state,
                tools,
            });
        }
        states
    }

    fn retry_state(&self, server: &str) -> Option<RetryState> {
        self.retries
            .lock()
            .expect("mcp retry lock is not poisoned")
            .get(server)
            .copied()
    }

    fn bump_retry(&self, server: &str) {
        let mut retries = self.retries.lock().expect("mcp retry lock is not poisoned");
        let attempt = retries.get(server).map(|state| state.attempt).unwrap_or(0) + 1;
        retries.insert(
            server.to_string(),
            RetryState {
                attempt,
                not_before: Instant::now() + backoff_delay(attempt),
            },
        );
    }

    fn clear_retry(&self, server: &str) {
        self.retries
            .lock()
            .expect("mcp retry lock is not poisoned")
            .remove(server);
    }

    /// How long the lifecycle loop may sleep before its next pass: a pending
    /// reconnect fires as soon as its backoff elapses (never hot-looping —
    /// the pass is the unit of work), otherwise the probe cadence rules.
    fn pass_pause(&self) -> Duration {
        let now = Instant::now();
        let earliest = self
            .retries
            .lock()
            .expect("mcp retry lock is not poisoned")
            .values()
            .map(|state| state.not_before)
            .min();
        match earliest {
            Some(due) if due > now => (due - now).min(PROBE_INTERVAL),
            _ => PROBE_INTERVAL,
        }
    }

    /// Applies a delivered servers document to the live topology (spec 015
    /// FR-001/FR-002): diff per server NAME against the running slots, then
    /// — removals first — disconnect and unregister exactly the removed or
    /// changed ones, connect the added or changed ones concurrently (each
    /// bounded by its own configured timeout, so a failing server never
    /// blocks another's apply), and adopt the reconciled desired topology
    /// the lifecycle reconciles toward. Unchanged names with identical
    /// config are untouched — no session churn.
    ///
    /// Ownership (ADR-0045 §4): a delivery removes or replaces only the
    /// CLOUD-declared servers, by name. A file-declared server the document
    /// omits survives live — exactly as it survives the restart's merge — so
    /// the running topology never flip-flops against the persisted one. A
    /// name the cloud (re-)declares becomes cloud-owned.
    ///
    /// The whole reconcile is serialized against the lifecycle's passes (the
    /// `apply_order` mutex): an in-flight pass finishes before anything is
    /// disconnected, so a removed server cannot be reconnected behind the
    /// apply's back by a stale pass task, and a changed server's new-spec
    /// session cannot be replaced by an old-spec one.
    ///
    /// A server that fails to connect does not fail the apply: it lands in
    /// the report's `failed` and the lifecycle retries with backoff (spec
    /// 015 FR-002 — the operator sees the state, not a refusal).
    ///
    /// With an injected fake (no client) the bookkeeping still runs — the
    /// desired list adopts, removals unregister — and the test's own seeds
    /// stand in for discoveries; deterministic, no I/O.
    pub async fn apply_servers(
        &self,
        delivered: Vec<McpServerConfig>,
        registry: &mut argus_domain::CapabilityRegistry,
    ) -> McpReconcileReport {
        // One ordered operation against the lifecycle: the pass in flight
        // (if any) drains before the first disconnect.
        let _order = self.apply_order.lock().await;

        // An unlisted or disabled server contributes nothing (spec 014
        // FR-001): the diff runs against the ENABLED delivered servers.
        let enabled: Vec<McpServerConfig> = delivered
            .into_iter()
            .filter(|server| server.enabled)
            .collect();
        let current = self.desired();
        let (to_connect, to_remove, desired) = reconcile_plan(&current, &enabled);

        let mut report = McpReconcileReport::default();

        // Removals first (a changed server is removed and re-added): drop the
        // connection, the runtime's adopted tools, and the authorization
        // registry's namespace — each server independent of the others. The
        // retry state goes with the server, so the map never accumulates
        // ghosts.
        for server in &to_remove {
            if let Some(client) = &self.client {
                client.disconnect(&server.name).await;
            }
            self.seam.unregister_server(&server.name);
            let unregistered = registry.unregister_server(&server.name);
            self.clear_retry(&server.name);
            report.removed.push(server.name.clone());
            tracing::info!(
                server = %server.name,
                tools = unregistered.len(),
                "mcp server removed by delivery; its tools are unregistered"
            );
        }

        if !to_connect.is_empty()
            && let Some(client) = &self.client
        {
            let mut pass = tokio::task::JoinSet::new();
            for server in to_connect {
                let client = Arc::clone(client);
                pass.spawn(async move {
                    let spec = server.spec();
                    let risk_overrides = server.risk_overrides();
                    let result = client
                        .connect(&spec, &risk_overrides, server.tools.as_deref())
                        .await;
                    (server, result)
                });
            }
            while let Some(joined) = pass.join_next().await {
                let (server, result) = joined.expect("the mcp reconcile task cannot panic");
                match result {
                    Ok(descriptors) => {
                        let mut adopted = 0;
                        for descriptor in descriptors {
                            // Already-registered ids keep their ladder and
                            // audit state by capability id (spec 014).
                            if registry.contains(descriptor.id()) {
                                continue;
                            }
                            if registry.register(descriptor).is_ok() {
                                adopted += 1;
                            }
                        }
                        report.connected.push(server.name.clone());
                        // A fresh delivery must not inherit a stale delay.
                        self.clear_retry(&server.name);
                        tracing::info!(
                            server = %server.name,
                            tools = adopted,
                            "mcp server connected by delivery; tools adopted"
                        );
                    }
                    Err(error) => {
                        report.failed.push((server.name.clone(), error.to_string()));
                        self.bump_retry(&server.name);
                        tracing::warn!(
                            server = %server.name,
                            error = %error,
                            "mcp server failed to connect on apply; the lifecycle task will retry"
                        );
                    }
                }
            }
        }

        // Adopt the reconciled topology: the lifecycle loop reads this list
        // every pass, so the slots it maintains are exactly the reconciled
        // ones from here on.
        *self
            .servers
            .write()
            .expect("mcp desired-servers lock is not poisoned") = desired;
        report
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
        let servers = self.desired_servers();
        let handles: Vec<_> = servers
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
                    self.bump_retry(&server.name);
                    tracing::warn!(
                        server = %server.name,
                        error = %error,
                        "mcp server failed to connect at startup; the lifecycle task will retry"
                    );
                }
            }
        }
    }

    /// Spawns the per-server lifecycle loop (spec 014): every pass visits
    /// each desired server concurrently — probing the live slots, connecting
    /// the dead ones once their backoff has elapsed — so one slow or failing
    /// server never delays another and the loop never waits on a server's
    /// I/O beyond that server's own timeout. Late-discovered tools register
    /// through the daemon so the capability surface stays truthful. The
    /// default lifecycle policy is keep-registered-but-failing: a down
    /// server's tools stay registered and fail typed (visible failure, spec
    /// 014 FR-004). Each pass re-reads the desired list, which is how a
    /// delivered `mcp` document (spec 015) is adopted without a restart.
    pub fn spawn_lifecycle(self: &Arc<Self>, daemon: Arc<crate::runtime::Daemon>) {
        let wiring = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                // The pass holds the apply-order lock for its whole body: a
                // delivery waits for the pass to drain before it disconnects
                // anything, so no stale pass task can resurrect a removed
                // server or reconnect an old spec over a new one.
                let _order = wiring.apply_order.lock().await;
                wiring.lifecycle_pass(&daemon).await;
                drop(_order);
                tokio::time::sleep(wiring.pass_pause()).await;
            }
        });
    }

    /// One lifecycle pass over the current desired servers.
    async fn lifecycle_pass(&self, daemon: &Arc<crate::runtime::Daemon>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let servers = self.desired_servers();
        if servers.is_empty() {
            return;
        }
        let now = Instant::now();

        enum PassOutcome {
            /// A dead slot reconnected (or a live slot re-probed): freshly
            /// discovered descriptors to adopt; the flag says whether this
            /// was a fresh connect (the log-worthy event) or a routine probe.
            Descriptors(String, Vec<argus_domain::CapabilityDescriptor>, bool),
            /// The connect failed; backoff before the next attempt.
            Unreachable(String, String),
            /// A live server went down mid-probe; its slot is disconnected.
            WentDown(String, String),
            /// Waiting out a backoff.
            Waiting,
        }

        let mut pass = tokio::task::JoinSet::new();
        for server in servers {
            let client = Arc::clone(&client);
            let not_before = self.retry_state(&server.name).map(|state| state.not_before);
            pass.spawn(async move {
                if client.has_connection(&server.name).await {
                    // Probe until the server goes down; a probe failure
                    // disconnects the slot so calls fail typed from that
                    // moment.
                    let risk_overrides = server.risk_overrides();
                    match client
                        .refresh(&server.name, &risk_overrides, server.tools.as_deref())
                        .await
                    {
                        Ok(descriptors) => {
                            PassOutcome::Descriptors(server.name, descriptors, false)
                        }
                        Err(error) => {
                            client.disconnect(&server.name).await;
                            PassOutcome::WentDown(server.name, error.to_string())
                        }
                    }
                } else {
                    // A dead slot reconnects only once its backoff has
                    // elapsed — the flapping-server guard.
                    if not_before.is_some_and(|due| now < due) {
                        return PassOutcome::Waiting;
                    }
                    let spec = server.spec();
                    let risk_overrides = server.risk_overrides();
                    match client
                        .connect(&spec, &risk_overrides, server.tools.as_deref())
                        .await
                    {
                        Ok(descriptors) => PassOutcome::Descriptors(server.name, descriptors, true),
                        Err(error) => PassOutcome::Unreachable(server.name, error.to_string()),
                    }
                }
            });
        }
        while let Some(joined) = pass.join_next().await {
            match joined.expect("the mcp lifecycle task cannot panic") {
                PassOutcome::Descriptors(name, descriptors, connected) => {
                    self.clear_retry(&name);
                    let fresh = daemon.register_mcp_descriptors(descriptors);
                    if fresh > 0 {
                        // Late-discovered tools reach the cloud surface
                        // without a restart (spec 014).
                        daemon.refresh_capability_surface();
                    }
                    if connected {
                        tracing::info!(server = %name, fresh_tools = fresh, "mcp server connected");
                    }
                }
                PassOutcome::Unreachable(name, error) => {
                    self.bump_retry(&name);
                    tracing::warn!(
                        server = %name,
                        error = %error,
                        "mcp server unreachable; reconnecting with backoff"
                    );
                }
                PassOutcome::WentDown(name, error) => {
                    self.bump_retry(&name);
                    tracing::warn!(
                        server = %name,
                        error = %error,
                        "mcp server went down; its tools fail typed until it returns"
                    );
                }
                PassOutcome::Waiting => {}
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use argus_domain::{CapabilityDescriptor, CapabilityId};
    use std::sync::Mutex;

    fn stdio_server(name: &str, command: &str, enabled: bool) -> McpServerConfig {
        McpServerConfig {
            name: name.to_string(),
            transport: McpTransportConfig::Stdio {
                command: command.to_string(),
                args: Vec::new(),
            },
            enabled,
            timeout_seconds: 30,
            tools: None,
            overrides: Vec::new(),
        }
    }

    /// The reconcile plan is the whole diff contract (spec 015 FR-002 with
    /// ADR-0045 §4): added connects, removed disconnects, changed does both,
    /// unchanged appears in neither list — no session churn — and ownership
    /// decides what a delivery may take away.
    #[test]
    fn the_reconcile_plan_diffs_per_server_name() {
        let alpha = stdio_server("alpha", "/bin/alpha", true);
        let mut alpha_v2 = alpha.clone();
        alpha_v2.timeout_seconds = 42;
        let beta = stdio_server("beta", "/bin/beta", true);
        let gamma = stdio_server("gamma", "/bin/gamma", true);
        let cloud = |server: McpServerConfig| DesiredServer {
            server,
            from_cloud: true,
        };
        let plan = |current: Vec<DesiredServer>, delivered: Vec<McpServerConfig>| {
            let (connect, remove, desired) = reconcile_plan(&current, &delivered);
            let names = desired
                .iter()
                .map(|held| held.server.name.clone())
                .collect::<Vec<String>>();
            (connect, remove, names)
        };

        // Added.
        let (connect, remove, desired) = plan(Vec::new(), vec![alpha.clone()]);
        assert_eq!(connect, vec![alpha.clone()]);
        assert!(remove.is_empty());
        assert_eq!(desired, vec!["alpha"]);

        // Removed — a cloud-declared server an omitting delivery takes away.
        let (connect, remove, desired) = plan(vec![cloud(alpha.clone())], Vec::new());
        assert!(connect.is_empty());
        assert_eq!(remove, vec![alpha.clone()]);
        assert!(desired.is_empty());

        // Changed: the old slot is removed and the new spec connected.
        let (connect, remove, desired) = plan(vec![cloud(alpha.clone())], vec![alpha_v2.clone()]);
        assert_eq!(connect, vec![alpha_v2.clone()]);
        assert_eq!(remove, vec![alpha]);
        assert_eq!(desired, vec!["alpha"]);

        // Unchanged: untouched — no churn.
        let (connect, remove, desired) =
            plan(vec![cloud(alpha_v2.clone())], vec![alpha_v2.clone()]);
        assert!(connect.is_empty() && remove.is_empty());
        assert_eq!(desired, vec!["alpha"]);

        // Unchanged names stay untouched beside the churn of others.
        let held = vec![
            cloud(stdio_server("alpha", "/bin/alpha", true)),
            cloud(gamma.clone()),
        ];
        let delivered = vec![alpha_v2, beta, gamma];
        let (connect, remove, desired) = plan(held, delivered);
        assert_eq!(
            connect.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            vec!["alpha", "beta"]
        );
        assert_eq!(
            remove.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            vec!["alpha"]
        );
        assert_eq!(desired, vec!["alpha", "gamma", "beta"]);
        assert!(!connect.iter().any(|s| s.name == "gamma"));
    }

    /// THE ownership rule (ADR-0045 §4, the flip-flop guard): a file-declared
    /// server survives — live and desired — every delivery that does not
    /// re-declare its name.
    #[test]
    fn a_file_declared_server_survives_a_delivery_that_omits_it() {
        let file = stdio_server("file-server", "/bin/file", true);
        let delivered = vec![stdio_server("cloud-only", "/bin/cloud", true)];
        let held = vec![DesiredServer {
            server: file,
            from_cloud: false,
        }];

        let (connect, remove, desired) = reconcile_plan(&held, &delivered);

        assert_eq!(
            connect.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            vec!["cloud-only"]
        );
        assert!(
            remove.is_empty(),
            "the file server is not the delivery's to take"
        );
        assert_eq!(desired.len(), 2);
        let survivor = desired
            .iter()
            .find(|held| held.server.name == "file-server")
            .unwrap();
        assert!(!survivor.from_cloud, "it stays file-owned");
    }

    /// A re-declared name becomes cloud-owned: the delivery that declares it
    /// is the delivery that may later take it away.
    #[test]
    fn a_redelivered_file_name_becomes_cloud_owned() {
        let file = stdio_server("shared", "/bin/file", true);
        let held = vec![DesiredServer {
            server: file,
            from_cloud: false,
        }];
        let delivered = vec![stdio_server("shared", "/bin/cloud", true)];

        let (_, remove, desired) = reconcile_plan(&held, &delivered);

        assert_eq!(remove, vec![stdio_server("shared", "/bin/file", true)]);
        assert_eq!(desired.len(), 1);
        assert!(desired[0].from_cloud, "cloud supersedes file");
    }

    /// A disabled delivery is a removal, not a connect (spec 014 FR-001: a
    /// disabled server contributes nothing). The enabled filter is
    /// `apply_servers`' job, so the test goes through it.
    #[tokio::test]
    async fn a_disabled_server_reads_as_removed() {
        let seam = Arc::new(RecordingSeam::default());
        let descriptor = seam.seed("mcp.alpha.probe");
        let wiring = wiring_with(
            Arc::clone(&seam),
            vec![stdio_server("alpha", "/bin/alpha", true)],
        );
        let mut registry = argus_domain::CapabilityRegistry::new();
        registry.register(descriptor).unwrap();

        let mut alpha_off = stdio_server("alpha", "/bin/alpha", true);
        alpha_off.enabled = false;
        let report = wiring.apply_servers(vec![alpha_off], &mut registry).await;

        assert!(connect_empty(&report));
        assert_eq!(report.removed, vec!["alpha".to_string()]);
        assert!(wiring.desired_servers().is_empty());
        assert!(!registry.contains(&CapabilityId::new("mcp.alpha.probe").unwrap()));
    }

    fn connect_empty(report: &McpReconcileReport) -> bool {
        report.connected.is_empty() && report.failed.is_empty()
    }

    /// A recording seam so the tests observe the unregister calls without a
    /// subprocess.
    #[derive(Default)]
    struct RecordingSeam {
        registered:
            std::sync::Mutex<std::collections::BTreeMap<CapabilityId, CapabilityDescriptor>>,
        unregistered: Mutex<Vec<String>>,
    }

    impl RecordingSeam {
        fn seed(&self, id: &str) -> CapabilityDescriptor {
            let descriptor = tool_descriptor(id);
            self.registered
                .lock()
                .expect("recording seam")
                .insert(descriptor.id().clone(), descriptor.clone());
            descriptor
        }
    }

    impl argus_domain::McpRuntime for RecordingSeam {
        fn register(
            &self,
            descriptor: CapabilityDescriptor,
        ) -> Result<(), argus_domain::DomainError> {
            self.registered
                .lock()
                .expect("recording seam")
                .insert(descriptor.id().clone(), descriptor);
            Ok(())
        }

        fn capabilities(&self) -> Vec<CapabilityId> {
            self.registered
                .lock()
                .expect("recording seam")
                .keys()
                .cloned()
                .collect()
        }

        fn descriptors(&self) -> Vec<CapabilityDescriptor> {
            self.registered
                .lock()
                .expect("recording seam")
                .values()
                .cloned()
                .collect()
        }

        fn unregister_server(&self, server: &str) -> Vec<CapabilityId> {
            self.unregistered
                .lock()
                .expect("recording seam")
                .push(server.to_string());
            let prefix = format!("mcp.{server}.");
            let mut table = self.registered.lock().expect("recording seam");
            let removed: Vec<CapabilityId> = table
                .keys()
                .filter(|id| id.as_str().starts_with(&prefix))
                .cloned()
                .collect();
            for id in &removed {
                table.remove(id);
            }
            removed
        }
    }

    fn tool_descriptor(id: &str) -> CapabilityDescriptor {
        CapabilityDescriptor::new(
            CapabilityId::new(id).unwrap(),
            "external",
            id,
            argus_domain::RiskClass::HighRisk,
            semver::Version::new(0, 1, 0),
            serde_json::json!({}),
            serde_json::json!({}),
            argus_domain::Reversibility::None,
        )
    }

    /// A wiring seeded with CLOUD-declared servers — the delivery-owned
    /// seeds the removal tests rely on.
    fn wiring_with(seam: Arc<RecordingSeam>, servers: Vec<McpServerConfig>) -> Arc<McpWiring> {
        wiring_with_ownership(seam, Vec::new(), servers)
    }

    /// A wiring with the startup ownership split: `file` seeds file-declared,
    /// `cloud` cloud-declared.
    fn wiring_with_ownership(
        seam: Arc<RecordingSeam>,
        file: Vec<McpServerConfig>,
        cloud: Vec<McpServerConfig>,
    ) -> Arc<McpWiring> {
        let controller: Arc<dyn McpToolController> = Arc::new(NoTools);
        McpWiring::injected(seam, controller, None, file, cloud)
    }

    /// A stand-in executor port: reconcile tests never invoke a tool.
    struct NoTools;

    impl McpToolController for NoTools {
        fn call(
            &self,
            _capability: &CapabilityId,
            _arguments: &serde_json::Value,
        ) -> Result<serde_json::Value, argus_executor::RemediationError> {
            Err(argus_executor::RemediationError::Unavailable(
                "no tools in this test".to_string(),
            ))
        }
    }

    #[tokio::test]
    async fn apply_servers_adopts_the_enabled_document_and_skips_disabled() {
        let seam = Arc::new(RecordingSeam::default());
        let wiring = wiring_with(
            Arc::clone(&seam),
            vec![stdio_server("old", "/bin/old", true)],
        );

        let delivered = vec![
            stdio_server("alpha", "/bin/alpha", true),
            stdio_server("quiet", "/bin/quiet", false),
        ];
        let mut registry = argus_domain::CapabilityRegistry::new();
        let report = wiring.apply_servers(delivered, &mut registry).await;

        assert!(report.connected.is_empty(), "a fake wiring connects no one");
        let desired = wiring.desired_servers();
        assert_eq!(
            desired.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            vec!["alpha"],
            "the disabled server contributes nothing; the old topology is replaced"
        );
        assert!(
            seam.unregistered
                .lock()
                .unwrap()
                .contains(&"old".to_string())
        );
    }

    #[tokio::test]
    async fn a_removed_server_is_disconnected_and_unregistered_everywhere() {
        let seam = Arc::new(RecordingSeam::default());
        let descriptor = seam.seed("mcp.alpha.probe");
        let wiring = wiring_with(
            Arc::clone(&seam),
            vec![stdio_server("alpha", "/bin/alpha", true)],
        );
        let mut registry = argus_domain::CapabilityRegistry::new();
        registry.register(descriptor).unwrap();
        assert!(registry.contains(&CapabilityId::new("mcp.alpha.probe").unwrap()));

        let report = wiring.apply_servers(Vec::new(), &mut registry).await;

        assert_eq!(report.removed, vec!["alpha".to_string()]);
        assert_eq!(
            seam.unregistered.lock().unwrap().as_slice(),
            ["alpha"],
            "the runtime's adopted table drops the server"
        );
        assert!(
            !registry.contains(&CapabilityId::new("mcp.alpha.probe").unwrap()),
            "the authorization surface drops the server's tools"
        );
        assert!(wiring.desired_servers().is_empty());
    }

    #[tokio::test]
    async fn a_changed_server_is_replaced_without_touching_unchanged_ones() {
        let seam = Arc::new(RecordingSeam::default());
        let alpha_descriptor = seam.seed("mcp.alpha.probe");
        let same_descriptor = seam.seed("mcp.same.probe");
        let mut alpha_v2 = stdio_server("alpha", "/bin/alpha", true);
        alpha_v2.timeout_seconds = 42;
        let wiring = wiring_with(
            Arc::clone(&seam),
            vec![
                stdio_server("alpha", "/bin/alpha", true),
                stdio_server("same", "/bin/same", true),
            ],
        );
        let mut registry = argus_domain::CapabilityRegistry::new();
        registry.register(alpha_descriptor).unwrap();
        registry.register(same_descriptor).unwrap();

        let report = wiring
            .apply_servers(
                vec![alpha_v2, stdio_server("same", "/bin/same", true)],
                &mut registry,
            )
            .await;

        assert_eq!(report.removed, vec!["alpha".to_string()]);
        // With no client the connect is bookkeeping-only; the removed slot is
        // unregistered and the unchanged server's tool stays exactly as it was.
        assert_eq!(seam.unregistered.lock().unwrap().as_slice(), ["alpha"]);
        assert!(!registry.contains(&CapabilityId::new("mcp.alpha.probe").unwrap()));
        assert!(registry.contains(&CapabilityId::new("mcp.same.probe").unwrap()));
        assert_eq!(wiring.desired_servers()[0].timeout_seconds, 42);
    }

    #[tokio::test]
    async fn an_apply_is_idempotent_for_an_identical_document() {
        let seam = Arc::new(RecordingSeam::default());
        let document = vec![
            stdio_server("alpha", "/bin/alpha", true),
            stdio_server("beta", "/bin/beta", true),
        ];
        let wiring = wiring_with(Arc::clone(&seam), Vec::new());
        let mut registry = argus_domain::CapabilityRegistry::new();

        let _ = wiring.apply_servers(document.clone(), &mut registry).await;
        let report = wiring.apply_servers(document, &mut registry).await;

        assert!(report.removed.is_empty() && report.connected.is_empty());
        assert!(
            seam.unregistered.lock().unwrap().is_empty(),
            "an identical re-delivery must not churn sessions"
        );
    }

    #[tokio::test]
    async fn a_failing_connect_lands_in_the_report_and_the_apply_still_lands() {
        // The production runtime against a command that cannot spawn: the
        // per-server failure is isolated into the report while the topology
        // adopts the delivery (spec 015 FR-002 — applied, retry state visible).
        let wiring = McpWiring::production(Vec::new());
        let mut registry = argus_domain::CapabilityRegistry::new();

        let report = wiring
            .apply_servers(
                vec![stdio_server(
                    "doomed",
                    "/nonexistent-argus-test-binary",
                    true,
                )],
                &mut registry,
            )
            .await;

        assert_eq!(report.connected.len(), 0);
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].0, "doomed");
        assert!(
            !report.failed[0].1.is_empty(),
            "the failure carries its reason"
        );
        assert_eq!(
            wiring
                .desired_servers()
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            vec!["doomed"],
            "the document is applied; the server sits in retry state"
        );

        // FR-004: the retry state is VISIBLE — the wiring reports the server
        // as retrying with zero tools, not as an applied-and-fine blank.
        let states = wiring.server_states().await;
        assert_eq!(states.len(), 1);
        assert_eq!(states[0].name, "doomed");
        assert_eq!(states[0].state, "retrying");
        assert_eq!(states[0].tools, 0);

        // Removing the server clears its retry state with it: no ghost entry
        // keeps the loop waking at the failed server's short backoff once
        // nothing desires it (the pause returns to the probe cadence).
        assert!(
            wiring.pass_pause() <= PROBE_INTERVAL,
            "sanity: the pause is bounded by the probe interval"
        );
        let _ = wiring.apply_servers(Vec::new(), &mut registry).await;
        assert_eq!(
            wiring.pass_pause(),
            PROBE_INTERVAL,
            "a removed server's backoff entry goes with it"
        );
    }

    #[tokio::test]
    async fn a_file_declared_server_stays_desired_when_a_delivery_omits_it() {
        // THE flip-flop guard, live: the file-declared server survives a
        // delivery that names only another server — connected slot untouched,
        // desired list still carrying it, nothing unregistered (ADR-0045 §4).
        let seam = Arc::new(RecordingSeam::default());
        let descriptor = seam.seed("mcp.file-server.probe");
        let wiring = wiring_with_ownership(
            Arc::clone(&seam),
            vec![stdio_server("file-server", "/bin/file", true)],
            Vec::new(),
        );
        let mut registry = argus_domain::CapabilityRegistry::new();
        registry.register(descriptor).unwrap();

        let report = wiring
            .apply_servers(
                vec![stdio_server("cloud-only", "/bin/cloud", true)],
                &mut registry,
            )
            .await;

        assert!(report.removed.is_empty(), "{report:?}");
        assert!(seam.unregistered.lock().unwrap().is_empty());
        assert!(registry.contains(&CapabilityId::new("mcp.file-server.probe").unwrap()));
        assert_eq!(
            wiring
                .desired_servers()
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            vec!["file-server", "cloud-only"],
        );
        // And the file server SURVIVES the next omitting delivery too — the
        // cloud-only server is the one that goes.
        let report = wiring.apply_servers(Vec::new(), &mut registry).await;
        assert_eq!(report.removed, vec!["cloud-only".to_string()]);
        assert_eq!(
            wiring
                .desired_servers()
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            vec!["file-server"],
        );
    }

    #[tokio::test]
    async fn server_states_report_the_seed_count_for_a_fake_wiring() {
        // With no client the slot is never live: a desired server reads as
        // connecting (its tool count still comes from the seam's table).
        let seam = Arc::new(RecordingSeam::default());
        seam.seed("mcp.alpha.probe");
        let wiring = wiring_with(
            Arc::clone(&seam),
            vec![stdio_server("alpha", "/bin/alpha", true)],
        );

        let states = wiring.server_states().await;

        assert_eq!(states.len(), 1);
        assert_eq!(states[0].name, "alpha");
        assert_eq!(states[0].state, "connecting");
        assert_eq!(states[0].tools, 1);
    }
}
