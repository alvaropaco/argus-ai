//! The `rmcp`-backed MCP client runtime (spec 014, ADR-0044).
//!
//! One runtime manages every configured server. Each server connects and
//! reconnects independently (a per-server slot; the daemon's lifecycle task
//! drives `connect`/`refresh`/`disconnect` with backoff), and a server's
//! failure never touches the loop, the daemon, or the other servers.
//!
//! The trust gradient is config, not the wire (ADR-0044 §1): a discovered
//! tool registers **HighRisk by default** with per-invocation approval, and
//! the ONLY thing that lowers either is the operator's per-tool config
//! override. Server self-reports — the tool's own annotations and hints —
//! are never read here: hints inform, config decides.

use std::collections::{BTreeMap, BTreeSet};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use argus_domain::{
    BlastRadius, CapabilityDescriptor, CapabilityId, DomainError, McpRuntime, Reversibility,
    RiskClass,
};
use argus_executor::McpToolController;
use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, ContentBlock, Tool};
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::{
    IntoTransport, StreamableHttpClientTransport, TokioChildProcess,
    streamable_http_client::StreamableHttpClientTransportConfig,
};
use semver::Version;
use serde_json::{Value, json};

pub use crate::error::McpError;

/// The upper bound on a tool call's execution evidence: the tool's output,
/// serialized, truncated to this many bytes with a `truncated: true` marker.
/// Bounded memory against a malicious or buggy server (spec 014 NFR).
pub const EVIDENCE_CAP_BYTES: usize = 8 * 1024;

/// What transport a configured server speaks (the daemon maps its `[mcp]`
/// config onto this; the runtime holds no config types of its own).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportSpec {
    /// A subprocess the runtime owns — the transport, not a shell (spec 014
    /// boundaries): the daemon's policy/execution boundary still gates every
    /// call.
    Stdio { command: String, args: Vec<String> },
    /// A streamable-HTTP endpoint.
    Http { url: String },
}

/// One configured server: identity, transport, and the per-call timeout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerSpec {
    /// The namespace segment: `mcp.<name>.<tool>`. Must already satisfy the
    /// capability-id segment grammar (the daemon validates its config).
    pub name: String,
    pub transport: TransportSpec,
    /// The per-call and connect timeout (default 30s, config clamped 5–300).
    pub timeout: Duration,
}

/// The per-tool trust override the OPERATOR wrote (ADR-0044 §1): lowering a
/// tool's risk is the operator writing law. The server's annotations never
/// reach the runtime, so they can never change authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOverride {
    /// The MCP tool name exactly as the server advertises it.
    pub tool: String,
    pub risk: RiskClass,
}

impl ToolOverride {
    pub fn new(tool: impl Into<String>, risk: RiskClass) -> Self {
        Self {
            tool: tool.into(),
            risk,
        }
    }
}

/// Whether a discovered tool is adopted: the operator's allowlist, when
/// configured, admits only the named tools (absent means all).
type Allowlist<'a> = Option<&'a [String]>;

/// A live connection to one server: the running rmcp client service, the tool
/// names it advertised at the last successful discovery, and its timeout.
struct ServerConnection {
    service: RunningService<RoleClient, rmcp::model::ClientConfig>,
    tools: BTreeSet<String>,
    timeout: Duration,
}

/// One server's slot: the connection (or its absence — a disconnected server's
/// slot stays behind so its tools fail typed, never silently vanish).
#[derive(Default)]
struct ServerSlot {
    connection: tokio::sync::Mutex<Option<ServerConnection>>,
}

/// The rmcp-backed MCP client runtime.
///
/// Shared between the daemon's executor (through
/// [`McpToolController`]) and its lifecycle tasks (through `connect`,
/// `refresh`, `disconnect`); implements the domain's [`McpRuntime`] seam for
/// the adopted-descriptor bookkeeping.
pub struct McpClientRuntime {
    /// The client identity advertised at initialize.
    client_name: String,
    /// Per-server slots, keyed by the configured server name.
    servers: std::sync::Mutex<BTreeMap<String, Arc<ServerSlot>>>,
    /// Every adopted descriptor (the [`McpRuntime`] table). A disconnected
    /// server's descriptors stay — the tools remain registered and failing
    /// (visible failure, spec 014 FR-004 default) until the server returns.
    registered: std::sync::Mutex<BTreeMap<CapabilityId, CapabilityDescriptor>>,
}

impl Default for McpClientRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl McpClientRuntime {
    pub fn new() -> Self {
        Self::with_client_name("argusd")
    }

    pub fn with_client_name(client_name: impl Into<String>) -> Self {
        Self {
            client_name: client_name.into(),
            servers: std::sync::Mutex::new(BTreeMap::new()),
            registered: std::sync::Mutex::new(BTreeMap::new()),
        }
    }

    /// Connects to a configured server, discovers its tools, and adopts them
    /// as capability descriptors.
    ///
    /// A reconnect replaces the previous connection (and kills the replaced
    /// stdio subprocess); discovery failure is a typed error and leaves the
    /// previous state untouched — the lifecycle task retries with backoff.
    pub async fn connect(
        &self,
        spec: &ServerSpec,
        overrides: &[ToolOverride],
        allow: Allowlist<'_>,
    ) -> Result<Vec<CapabilityDescriptor>, McpError> {
        match &spec.transport {
            TransportSpec::Stdio { command, args } => {
                let mut cmd = tokio::process::Command::new(command);
                cmd.args(args)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    // MCP stdio servers log on stderr; the daemon logs its own
                    // view, and an unread pipe would block a chatty server.
                    .stderr(Stdio::null());
                let transport = TokioChildProcess::new(cmd)
                    .map_err(|e| McpError::Transport(format!("spawn '{command}': {e}")))?;
                self.attach(spec, overrides, allow, transport).await
            }
            TransportSpec::Http { url } => {
                let transport = StreamableHttpClientTransport::with_client(
                    reqwest::Client::new(),
                    StreamableHttpClientTransportConfig::with_uri(url.clone()),
                );
                self.attach(spec, overrides, allow, transport).await
            }
        }
    }

    /// Attaches an already-built transport — the seam the in-process unit
    /// tests drive (client and server in one tokio runtime, no subprocess).
    pub(crate) async fn attach<T, E, A>(
        &self,
        spec: &ServerSpec,
        overrides: &[ToolOverride],
        allow: Allowlist<'_>,
        transport: T,
    ) -> Result<Vec<CapabilityDescriptor>, McpError>
    where
        T: IntoTransport<RoleClient, E, A>,
        E: std::error::Error + Send + Sync + 'static,
    {
        let service: RunningService<RoleClient, rmcp::model::ClientConfig> =
            tokio::time::timeout(spec.timeout, self.client().serve(transport))
                .await
                .map_err(|_| McpError::ConnectTimeout {
                    server: spec.name.clone(),
                    timeout_secs: spec.timeout.as_secs(),
                })?
                .map_err(|e| McpError::Transport(format!("initialize '{}': {e}", spec.name)))?;

        let listed = tokio::time::timeout(spec.timeout, service.peer().list_tools(None))
            .await
            .map_err(|_| McpError::ConnectTimeout {
                server: spec.name.clone(),
                timeout_secs: spec.timeout.as_secs(),
            })?
            .map_err(|e| McpError::Transport(format!("list_tools '{}': {e}", spec.name)))?;

        let descriptors = adopt_tools(spec, listed.tools, overrides, allow);
        let mut tools = BTreeSet::new();
        for descriptor in &descriptors {
            tools.insert(tool_of(descriptor.id()).to_string());
            // The trait's refresh semantics make a reconnect idempotent; the
            // in-memory table cannot refuse.
            McpRuntime::register(self, descriptor.clone())
                .map_err(|e| McpError::Transport(e.to_string()))?;
        }

        let connection = ServerConnection {
            service,
            tools,
            timeout: spec.timeout,
        };
        let slot = self.slot(&spec.name);
        let mut current = slot.connection.lock().await;
        // Replacing, not merging: the newest successful discovery is the
        // truth, and dropping the old connection kills its subprocess.
        *current = Some(connection);
        Ok(descriptors)
    }

    /// Re-discovers a connected server's tools (the lifecycle probe). A typed
    /// error means the server is down: the caller disconnects the slot and
    /// schedules the backoff reconnect.
    pub async fn refresh(
        &self,
        server: &str,
        overrides: &[ToolOverride],
        allow: Allowlist<'_>,
    ) -> Result<Vec<CapabilityDescriptor>, McpError> {
        let Some(slot) = self
            .servers
            .lock()
            .expect("slot table")
            .get(server)
            .cloned()
        else {
            return Err(McpError::NotConnected(server.to_string()));
        };
        let mut current = slot.connection.lock().await;
        let Some(connection) = current.as_mut() else {
            return Err(McpError::NotConnected(server.to_string()));
        };
        let listed = tokio::time::timeout(
            connection.timeout,
            connection.service.peer().list_tools(None),
        )
        .await
        .map_err(|_| McpError::Timeout {
            server: server.to_string(),
            tool: "*list_tools".to_string(),
            timeout_secs: connection.timeout.as_secs(),
        })?
        .map_err(|e| McpError::Transport(format!("list_tools '{server}': {e}")))?;

        let spec = ServerSpec {
            name: server.to_string(),
            // The transport is already connected; only the metadata matters
            // for descriptor building.
            transport: TransportSpec::Http { url: String::new() },
            timeout: connection.timeout,
        };
        let descriptors = adopt_tools(&spec, listed.tools, overrides, allow);
        connection.tools = descriptors
            .iter()
            .map(|d| tool_of(d.id()).to_string())
            .collect();
        for descriptor in &descriptors {
            McpRuntime::register(self, descriptor.clone())
                .map_err(|e| McpError::Transport(e.to_string()))?;
        }
        Ok(descriptors)
    }

    /// Drops a server's connection: its tools stay registered but fail typed
    /// until it returns (spec 014 FR-004 default — visible failure).
    pub async fn disconnect(&self, server: &str) {
        // The slot-table guard must not live across the await below.
        let slot = self
            .servers
            .lock()
            .expect("slot table")
            .get(server)
            .cloned();
        if let Some(slot) = slot {
            slot.connection.lock().await.take();
        }
    }

    /// Whether a server currently has a live connection.
    pub fn is_connected(&self, server: &str) -> bool {
        match self.servers.lock().expect("slot table").get(server) {
            Some(slot) => slot
                .connection
                .try_lock()
                .is_ok_and(|guard| guard.is_some()),
            None => false,
        }
    }

    /// Whether a live connection exists, awaiting the slot lock: unlike
    /// [`Self::is_connected`], an in-flight call does not read as absent, so
    /// the lifecycle's takeover gate never reconnects a busy healthy session.
    pub async fn has_connection(&self, server: &str) -> bool {
        let Some(slot) = self
            .servers
            .lock()
            .expect("slot table")
            .get(server)
            .cloned()
        else {
            return false;
        };
        slot.connection.lock().await.is_some()
    }

    /// Invokes the tool behind `mcp.<server>.<tool>` with `arguments` and
    /// returns the bounded execution evidence.
    ///
    /// Fail-closed by construction: not connected, unknown tool, timeout, and
    /// tool-reported failure are all typed errors. The arguments are gated
    /// HERE as well as at the registry: the plan-execution path builds
    /// authorized actions without a registry in hand, so the runtime
    /// re-validates them against the adopted descriptor's input schema and
    /// refuses non-object arguments outright — never a silent empty-object
    /// call (ADR-0027 §4). The evidence never exceeds
    /// [`EVIDENCE_CAP_BYTES`] of serialized output.
    pub async fn invoke(
        &self,
        capability: &CapabilityId,
        arguments: &Value,
    ) -> Result<Value, McpError> {
        let (server, tool) = split_capability(capability.as_str())
            .ok_or_else(|| McpError::InvalidCapabilityId(capability.as_str().to_string()))?;

        // The adopted descriptor is the gate: absent means the tool was never
        // adopted (fail closed), present means its schema decides.
        let descriptor = self
            .registered
            .lock()
            .expect("mcp descriptor table")
            .get(capability)
            .cloned();
        let Some(descriptor) = descriptor else {
            return Err(McpError::UnknownTool {
                server: server.to_string(),
                tool: tool.to_string(),
            });
        };
        let Some(argument_map) = arguments.as_object() else {
            return Err(McpError::InvalidArguments(capability.as_str().to_string()));
        };
        if !argus_domain::input_matches(descriptor.input_schema(), arguments) {
            return Err(McpError::InvalidArguments(capability.as_str().to_string()));
        }

        let slot = self
            .servers
            .lock()
            .expect("slot table")
            .get(server)
            .cloned();
        let Some(slot) = slot else {
            return Err(McpError::NotConnected(server.to_string()));
        };
        let guard = slot.connection.lock().await;
        let Some(connection) = guard.as_ref() else {
            return Err(McpError::NotConnected(server.to_string()));
        };
        if !connection.tools.contains(tool) {
            return Err(McpError::UnknownTool {
                server: server.to_string(),
                tool: tool.to_string(),
            });
        }

        let params =
            CallToolRequestParams::new(tool.to_string()).with_arguments(argument_map.clone());
        let result = tokio::time::timeout(
            connection.timeout,
            connection.service.peer().call_tool(params),
        )
        .await
        .map_err(|_| McpError::Timeout {
            server: server.to_string(),
            tool: tool.to_string(),
            timeout_secs: connection.timeout.as_secs(),
        })?
        .map_err(|e| McpError::Transport(format!("call '{server}.{tool}': {e}")))?;

        evidence_from_result(server, tool, result)
    }

    /// The client identity the runtime initializes with: an honest name and
    /// version, no capabilities declared — ARGUS observes, it does not offer
    /// sampling or roots to a server.
    fn client(&self) -> rmcp::model::ClientConfig {
        rmcp::model::ClientConfig::new(
            rmcp::model::ClientCapabilities::default(),
            rmcp::model::Implementation::new(self.client_name.clone(), env!("CARGO_PKG_VERSION")),
        )
    }

    fn slot(&self, name: &str) -> Arc<ServerSlot> {
        self.servers
            .lock()
            .expect("slot table")
            .entry(name.to_string())
            .or_default()
            .clone()
    }
}

/// Splits `mcp.<server>.<tool>` — server and tool are both single segments
/// (the config validates the server; non-conforming tool names are never
/// adopted), so the split is unambiguous.
fn split_capability(id: &str) -> Option<(&str, &str)> {
    let rest = id.strip_prefix("mcp.")?;
    let (server, tool) = rest.split_once('.')?;
    if tool.contains('.') || tool.is_empty() || server.is_empty() {
        return None;
    }
    Some((server, tool))
}

/// The trust decision for one discovered tool (ADR-0044 §1): HighRisk by
/// default with approval required; the operator's override lowers the risk
/// AND lifts the per-invocation approval gate, because a `controlled`
/// capability is governed by the autonomy matrix and the blast-radius budget
/// instead — never auto-executing below L4. This is the override's whole
/// semantics; nothing else (and never the server) changes authority.
fn tool_trust(overrides: &[ToolOverride], tool: &str) -> (RiskClass, bool) {
    match overrides.iter().find(|o| o.tool == tool) {
        Some(override_) => (override_.risk, false),
        None => (RiskClass::HighRisk, true),
    }
}

/// Builds the descriptors for a discovery result: the deterministic
/// `mcp.<server>.<tool>` id, the tool's own input JSON Schema verbatim, risk
/// per the operator's override or HighRisk, blast radius `Host`,
/// reversibility `None`, and the server's timeout. Names that cannot satisfy
/// the capability-id grammar are skipped (fail-closed: an unrepresentable
/// tool is an unregistered tool, never a mutated one).
fn adopt_tools(
    spec: &ServerSpec,
    tools: Vec<Tool>,
    overrides: &[ToolOverride],
    allow: Allowlist<'_>,
) -> Vec<CapabilityDescriptor> {
    let mut descriptors = Vec::new();
    for tool in tools {
        let name = tool.name.trim();
        if name.is_empty() || name.contains('.') {
            tracing::warn!(
                server = %spec.name,
                tool = %name,
                "mcp tool name does not fit the capability-id grammar; skipping"
            );
            continue;
        }
        if let Some(allow) = allow
            && !allow.iter().any(|a| a == name)
        {
            tracing::info!(
                server = %spec.name,
                tool = %name,
                "mcp tool is not on the configured allowlist; skipping"
            );
            continue;
        }
        let Ok(id) = CapabilityId::new(&format!("mcp.{}.{}", spec.name, name)) else {
            tracing::warn!(
                server = %spec.name,
                tool = %name,
                "mcp tool name is not a valid capability segment; skipping"
            );
            continue;
        };
        let (risk, requires_approval) = tool_trust(overrides, name);
        // The tool's schema, verbatim (ADR-0044 §1): spec 010's
        // construction-time validation applies unchanged.
        let input_schema = Value::Object(tool.input_schema.as_ref().clone());
        let descriptor = CapabilityDescriptor::new(
            id,
            format!("mcp:{}", spec.name),
            name.to_string(),
            risk,
            Version::new(0, 1, 0),
            input_schema,
            json!({}),
            Reversibility::None,
        )
        .with_blast_radius(BlastRadius::Host)
        .requiring_approval_if(requires_approval)
        .with_timeout_seconds(spec.timeout.as_secs().max(1));
        descriptors.push(descriptor);
    }
    descriptors
}

fn tool_of(id: &CapabilityId) -> &str {
    split_capability(id.as_str())
        .map(|(_, tool)| tool)
        .unwrap_or_default()
}

/// Builds the bounded execution evidence from a tool result (spec 014
/// FR-003): a tool-reported error is a typed failure; the output is the
/// structured content when present, the text content otherwise — serialized
/// and truncated to [`EVIDENCE_CAP_BYTES`] with a `truncated: true` marker.
fn evidence_from_result(
    server: &str,
    tool: &str,
    result: rmcp::model::CallToolResult,
) -> Result<Value, McpError> {
    if result.is_error == Some(true) {
        let message = text_of(&result).unwrap_or_else(|| "unknown tool error".to_string());
        return Err(McpError::Tool {
            server: server.to_string(),
            tool: tool.to_string(),
            message,
        });
    }

    let output = match result.structured_content {
        Some(value) => value,
        None => match text_of(&result) {
            Some(text) => Value::String(text),
            None if result.content.is_empty() => Value::Null,
            None => serde_json::to_value(&result.content).unwrap_or(Value::Null),
        },
    };
    Ok(cap_evidence(output))
}

/// The joined text content of a result, when it has any.
fn text_of(result: &rmcp::model::CallToolResult) -> Option<String> {
    let texts: Vec<&str> = result
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect();
    match texts.len() {
        0 => None,
        _ => Some(texts.join("\n")),
    }
}

/// Caps the tool output at [`EVIDENCE_CAP_BYTES`] (spec 014 I/O matrix:
/// oversized output is truncated to the cap with a `truncated: true` marker
/// in evidence). Truncation is on the serialized form and never splits a
/// UTF-8 code point.
pub(crate) fn cap_evidence(output: Value) -> Value {
    let serialized = match serde_json::to_string(&output) {
        Ok(serialized) => serialized,
        Err(_) => return json!({ "output": Value::Null }),
    };
    if serialized.len() <= EVIDENCE_CAP_BYTES {
        return json!({ "output": output });
    }
    let mut end = EVIDENCE_CAP_BYTES.min(serialized.len());
    while end > 0 && !serialized.is_char_boundary(end) {
        end -= 1;
    }
    json!({
        "output": &serialized[..end],
        "truncated": true,
    })
}

impl McpRuntime for McpClientRuntime {
    fn register(&self, descriptor: CapabilityDescriptor) -> Result<(), DomainError> {
        self.registered
            .lock()
            .expect("mcp descriptor table")
            .insert(descriptor.id().clone(), descriptor);
        Ok(())
    }

    fn capabilities(&self) -> Vec<CapabilityId> {
        self.registered
            .lock()
            .expect("mcp descriptor table")
            .keys()
            .cloned()
            .collect()
    }

    fn descriptors(&self) -> Vec<CapabilityDescriptor> {
        self.registered
            .lock()
            .expect("mcp descriptor table")
            .values()
            .cloned()
            .collect()
    }

    fn unregister_server(&self, server: &str) -> Vec<CapabilityId> {
        let prefix = format!("mcp.{server}.");
        let mut registered = self.registered.lock().expect("mcp descriptor table");
        let removed: Vec<CapabilityId> = registered
            .keys()
            .filter(|id| id.as_str().starts_with(&prefix))
            .cloned()
            .collect();
        for id in &removed {
            registered.remove(id);
        }
        removed
    }
}

/// The executor port over the runtime (spec 014): the call is the invocation,
/// the result is the bounded evidence. The synchronous port bridges the async
/// transport with the daemon's own hand-off (`block_in_place`), exactly like
/// the D-Bus journal controller — production runs the multi-threaded runtime.
impl McpToolController for McpClientRuntime {
    fn call(
        &self,
        capability: &CapabilityId,
        arguments: &Value,
    ) -> Result<Value, argus_executor::RemediationError> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.invoke(capability, arguments))
        })
        .map_err(|error: McpError| error.to_remediation())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::ErrorData as ServerError;
    use rmcp::ServerHandler;
    use rmcp::model::{CallToolResponse, ListToolsResult, ServerCapabilities, ServerConfig};
    use rmcp::service::RequestContext;
    use rmcp::service::RoleServer;

    /// The in-process MCP server pair (rmcp's test seam): a tiny tool server
    /// served over an in-memory duplex transport — no subprocess in unit
    /// tests. Tools: `echo` (returns text), `big` (returns an oversized
    /// string), `fail` (reports a tool error), `bad.name` (a name the
    /// capability-id grammar cannot represent).
    struct EchoServer;

    impl ServerHandler for EchoServer {
        fn get_info(&self) -> ServerConfig {
            ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
                .with_server_info(rmcp::model::Implementation::new("echo-test", "0.1.0"))
        }

        async fn list_tools(
            &self,
            _request: Option<rmcp::model::PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListToolsResult, ServerError> {
            let schema = |value: serde_json::Value| {
                Arc::new(value.as_object().cloned().expect("object schema"))
            };
            Ok(ListToolsResult::with_all_items(vec![
                Tool::new(
                    "echo",
                    "echoes its message",
                    schema(json!({
                        "type": "object",
                        "required": ["message"],
                        "properties": { "message": { "type": "string" } },
                    })),
                ),
                Tool::new(
                    "big",
                    "returns an oversized string",
                    schema(json!({ "type": "object", "properties": {} })),
                ),
                Tool::new(
                    "fail",
                    "always reports a tool error",
                    schema(json!({ "type": "object", "properties": {} })),
                ),
                Tool::new(
                    "bad.name",
                    "a name the grammar cannot represent",
                    schema(json!({ "type": "object", "properties": {} })),
                ),
            ]))
        }

        async fn call_tool(
            &self,
            request: CallToolRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<CallToolResponse, ServerError> {
            use rmcp::model::CallToolResult;
            let arguments = request.arguments.clone().unwrap_or_default();
            match request.name.as_ref() {
                "echo" => {
                    let message = arguments
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    Ok(CallToolResponse::from(CallToolResult::success(vec![
                        ContentBlock::text(format!("echo: {message}")),
                    ])))
                }
                "big" => Ok(CallToolResponse::from(CallToolResult::success(vec![
                    ContentBlock::text("x".repeat(EVIDENCE_CAP_BYTES * 4)),
                ]))),
                "fail" => Ok(CallToolResponse::from(CallToolResult::error(vec![
                    ContentBlock::text("the tool refused".to_string()),
                ]))),
                other => Err(ServerError::internal_error(
                    format!("unknown tool {other}"),
                    None,
                )),
            }
        }
    }

    fn echo_spec() -> ServerSpec {
        ServerSpec {
            name: "echo".to_string(),
            transport: TransportSpec::Http { url: String::new() },
            timeout: Duration::from_secs(5),
        }
    }

    /// Serves [`EchoServer`] on one half of a duplex transport and connects a
    /// runtime slot to the other — the in-memory pair. The returned server
    /// service must stay alive: dropping it closes the transport.
    async fn connected(
        overrides: &[ToolOverride],
        allow: Allowlist<'_>,
    ) -> (
        McpClientRuntime,
        rmcp::service::RunningService<RoleServer, EchoServer>,
    ) {
        let runtime = McpClientRuntime::with_client_name("argus-test");
        let (client_half, server_half) = tokio::io::duplex(64 * 1024);
        let spec = echo_spec();
        let (server, _result) = tokio::join!(EchoServer.serve(server_half), async {
            runtime.attach(&spec, overrides, allow, client_half).await
        });
        (runtime, server.expect("the handshake succeeds"))
    }

    fn descriptor_for<'a>(
        descriptors: &'a [CapabilityDescriptor],
        tool: &str,
    ) -> &'a CapabilityDescriptor {
        descriptors
            .iter()
            .find(|d| d.id().as_str() == format!("mcp.echo.{tool}"))
            .unwrap_or_else(|| panic!("no descriptor for mcp.echo.{tool}"))
    }

    #[tokio::test]
    async fn connect_discovers_and_adopts_high_risk_descriptors() {
        let (runtime, server) = connected(&[], None).await;

        let descriptors = runtime.descriptors();
        let echo = descriptor_for(&descriptors, "echo");
        assert_eq!(echo.risk_class(), RiskClass::HighRisk, "unknown provenance");
        assert!(echo.requires_approval());
        assert_eq!(echo.effective_blast_radius(), BlastRadius::Host);
        assert_eq!(echo.reversibility(), Reversibility::None);
        assert_eq!(
            echo.input_schema(),
            &json!({
                "type": "object",
                "required": ["message"],
                "properties": { "message": { "type": "string" } },
            }),
            "the tool's schema is adopted verbatim"
        );
        assert_eq!(echo.provider(), "mcp:echo");
        assert!(
            descriptors
                .iter()
                .any(|d| d.id().as_str() == "mcp.echo.big")
        );
        // The unrepresentable name never becomes a capability.
        assert!(
            !descriptors
                .iter()
                .any(|d| d.id().as_str() == "mcp.echo.bad.name"),
            "a dotted tool name must be skipped, never folded into the id"
        );
        assert!(runtime.is_connected("echo"));
        drop(server);
    }

    #[tokio::test]
    async fn the_operator_override_lowers_risk_and_lifts_the_approval_gate() {
        let overrides = vec![ToolOverride::new("echo", RiskClass::Controlled)];
        let (runtime, server) = connected(&overrides, None).await;

        let descriptors = runtime.descriptors();
        let echo = descriptor_for(&descriptors, "echo");
        assert_eq!(echo.risk_class(), RiskClass::Controlled);
        assert!(
            !echo.requires_approval(),
            "an overridden tool is matrix-governed, not approval-gated (ADR-0044 §1)"
        );
        // A tool without the override keeps the fail-closed default.
        let big = descriptor_for(&descriptors, "big");
        assert_eq!(big.risk_class(), RiskClass::HighRisk);
        assert!(big.requires_approval());
        drop(server);
    }

    #[tokio::test]
    async fn the_allowlist_admits_only_the_named_tools() {
        let allow = vec!["echo".to_string()];
        let (runtime, server) = connected(&[], Some(&allow)).await;

        let ids: Vec<String> = runtime
            .capabilities()
            .iter()
            .map(|id| id.as_str().to_string())
            .collect();
        assert_eq!(ids, vec!["mcp.echo.echo".to_string()]);
        drop(server);
    }

    #[tokio::test]
    async fn invoke_returns_the_tool_output_as_bounded_evidence() {
        let (runtime, server) = connected(&[], None).await;

        let evidence = runtime
            .invoke(
                &CapabilityId::new("mcp.echo.echo").unwrap(),
                &json!({ "message": "hi" }),
            )
            .await
            .expect("the tool call succeeds");
        assert_eq!(evidence["output"], "echo: hi");
        assert!(evidence.get("truncated").is_none(), "nothing was truncated");
        drop(server);
    }

    #[tokio::test]
    async fn schema_violating_arguments_are_a_typed_failure_never_a_call() {
        let (runtime, server) = connected(&[], None).await;

        // The adopted schema requires `message`; an empty argument object
        // must be refused at the runtime, before any bytes reach the wire.
        let error = runtime
            .invoke(&CapabilityId::new("mcp.echo.echo").unwrap(), &json!({}))
            .await
            .unwrap_err();
        assert!(
            matches!(&error, McpError::InvalidArguments(id) if id == "mcp.echo.echo"),
            "{error:?}"
        );
        assert!(matches!(
            error.to_remediation(),
            argus_executor::RemediationError::Failed(_)
        ));
        drop(server);
    }

    #[tokio::test]
    async fn non_object_arguments_are_a_typed_failure_never_a_silent_empty_call() {
        let (runtime, server) = connected(&[], None).await;

        for arguments in [json!(null), json!("hi"), json!(["hi"])] {
            let error = runtime
                .invoke(&CapabilityId::new("mcp.echo.echo").unwrap(), &arguments)
                .await
                .unwrap_err();
            assert!(
                matches!(error, McpError::InvalidArguments(_)),
                "non-object arguments {arguments} must be refused: {error:?}"
            );
        }
        drop(server);
    }

    #[tokio::test]
    async fn oversized_output_is_truncated_to_the_cap_with_a_marker() {
        let (runtime, server) = connected(&[], None).await;

        let evidence = runtime
            .invoke(&CapabilityId::new("mcp.echo.big").unwrap(), &json!({}))
            .await
            .expect("the tool call succeeds");
        assert_eq!(evidence["truncated"], true);
        let output = evidence["output"].as_str().expect("string output");
        assert_eq!(output.len(), EVIDENCE_CAP_BYTES);
        drop(server);
    }

    #[tokio::test]
    async fn a_tool_reported_error_is_a_typed_failure() {
        let (runtime, server) = connected(&[], None).await;

        let error = runtime
            .invoke(&CapabilityId::new("mcp.echo.fail").unwrap(), &json!({}))
            .await
            .unwrap_err();
        assert!(
            matches!(error, McpError::Tool { .. }),
            "a tool error is typed, never evidence: {error:?}"
        );
        assert!(matches!(
            error.to_remediation(),
            argus_executor::RemediationError::Failed(_)
        ));
        drop(server);
    }

    #[tokio::test]
    async fn an_unknown_tool_is_a_typed_failure() {
        let (runtime, server) = connected(&[], None).await;

        let error = runtime
            .invoke(&CapabilityId::new("mcp.echo.nope").unwrap(), &json!({}))
            .await
            .unwrap_err();
        assert!(matches!(error, McpError::UnknownTool { .. }), "{error:?}");
        drop(server);
    }

    #[tokio::test]
    async fn a_disconnected_server_fails_typed_and_its_tools_stay_registered() {
        let (runtime, server) = connected(&[], None).await;

        runtime.disconnect("echo").await;
        assert!(!runtime.is_connected("echo"));
        // Schema-valid arguments, so the failure proven here is the
        // disconnection itself, not the argument gate.
        let error = runtime
            .invoke(
                &CapabilityId::new("mcp.echo.echo").unwrap(),
                &json!({ "message": "x" }),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(&error, McpError::NotConnected(name) if name == "echo"),
            "a disconnected server is a typed error: {error:?}"
        );
        assert!(matches!(
            error.to_remediation(),
            argus_executor::RemediationError::Unavailable(_)
        ));
        // Visible failure: the tools stay registered while the server is down.
        assert!(
            runtime
                .capabilities()
                .iter()
                .any(|id| id.as_str() == "mcp.echo.echo")
        );
        drop(server);
    }

    #[tokio::test]
    async fn a_reconnect_replaces_the_connection_and_the_tools_return() {
        let (runtime, server) = connected(&[], None).await;
        runtime.disconnect("echo").await;
        drop(server);

        // The server "returns": a fresh connection over a fresh pair.
        let (_server2, result) = {
            let (client_half, server_half) = tokio::io::duplex(64 * 1024);
            let spec = echo_spec();
            tokio::join!(EchoServer.serve(server_half), async {
                runtime.attach(&spec, &[], None, client_half).await
            })
        };
        let descriptors = result.expect("the reconnect succeeds");
        assert!(!descriptors.is_empty());
        assert!(runtime.is_connected("echo"));
        let evidence = runtime
            .invoke(
                &CapabilityId::new("mcp.echo.echo").unwrap(),
                &json!({ "message": "back" }),
            )
            .await
            .expect("the tool call succeeds after the reconnect");
        assert_eq!(evidence["output"], "echo: back");
    }

    #[tokio::test]
    async fn refresh_updates_the_table_and_is_idempotent_in_the_seam() {
        let (runtime, server) = connected(&[], None).await;

        let before = runtime.capabilities().len();
        let descriptors = runtime
            .refresh("echo", &[], None)
            .await
            .expect("refresh works");
        assert!(!descriptors.is_empty());
        assert_eq!(
            runtime.capabilities().len(),
            before,
            "a refresh of the same tools adds nothing"
        );
        drop(server);
    }

    #[tokio::test]
    async fn unregister_server_removes_only_that_servers_ids() {
        let runtime = McpClientRuntime::new();
        let id = CapabilityId::new("mcp.echo.echo").unwrap();
        McpRuntime::register(
            &runtime,
            CapabilityDescriptor::new(
                id.clone(),
                "mcp:echo",
                "echo",
                RiskClass::HighRisk,
                Version::new(0, 1, 0),
                json!({}),
                json!({}),
                Reversibility::None,
            ),
        )
        .unwrap();
        assert_eq!(runtime.capabilities(), vec![id.clone()]);

        assert!(runtime.unregister_server("other").is_empty());
        let removed = runtime.unregister_server("echo");
        assert_eq!(removed, vec![id.clone()]);
        assert!(runtime.capabilities().is_empty());
    }

    #[test]
    fn split_capability_is_exact() {
        assert_eq!(
            split_capability("mcp.metrics.query"),
            Some(("metrics", "query"))
        );
        assert_eq!(
            split_capability("mcp.my-server.get_weather"),
            Some(("my-server", "get_weather"))
        );
        assert_eq!(split_capability("host.status.read"), None);
        assert_eq!(split_capability("mcp.server"), None);
        assert_eq!(split_capability("mcp.a.b.c"), None, "dots in the tool part");
        assert_eq!(split_capability("mcp..tool"), None);
    }

    #[test]
    fn cap_evidence_passes_small_outputs_through() {
        let evidence = cap_evidence(json!({ "answer": 42 }));
        assert_eq!(evidence["output"]["answer"], 42);
        assert!(evidence.get("truncated").is_none());
    }

    #[test]
    fn cap_evidence_truncates_oversized_outputs_with_a_marker() {
        let output = json!("y".repeat(EVIDENCE_CAP_BYTES * 2));
        let evidence = cap_evidence(output);
        assert_eq!(evidence["truncated"], true);
        let output = evidence["output"].as_str().expect("a string");
        assert_eq!(output.len(), EVIDENCE_CAP_BYTES);
        // The cap cuts the serialized form (quotes included) without ever
        // splitting a UTF-8 code point.
        assert!(
            output.chars().all(|c| c == 'y' || c == '"'),
            "no split code points"
        );
    }
}
