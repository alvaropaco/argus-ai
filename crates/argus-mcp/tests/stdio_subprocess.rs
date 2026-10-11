//! The ONE subprocess integration test (spec 014): a real stdio MCP server —
//! this test binary re-invoked as a child — proving
//! connect → register → call → evidence → crash → typed failure → recovery
//! over the true transport.
//!
//! The target is `harness = false` so the child process's stdin/stdout carry
//! nothing but the MCP protocol: libtest's own stdout chatter would corrupt
//! the JSON-RPC framing. The role is an ARGV marker the parent passes through
//! `ServerSpec`'s args: `--argus-mcp-stdio-server` and the binary serves a
//! tiny echo server on stdio; without it, it runs the client-side scenarios
//! against itself as the subprocess.

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use argus_domain::CapabilityId;
use argus_executor::McpToolController;
use argus_mcp::{McpClientRuntime, ServerSpec, ToolOverride, TransportSpec};
use rmcp::ErrorData as ServerError;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ListToolsResult,
    ServerCapabilities, ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::service::RoleServer;
use rmcp::{ServerHandler, ServiceExt};
use serde_json::json;

/// The child's role marker: present in argv, the process serves MCP over
/// stdio. It rides `ServerSpec.args`, exactly like a real server's flags.
const SERVER_FLAG: &str = "--argus-mcp-stdio-server";

/// The echo server the child serves: `echo` returns its message, `die`
/// aborts the process mid-flight (the crash the recovery path must survive).
struct EchoServer;

impl ServerHandler for EchoServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(rmcp::model::Implementation::new("stdio-echo", "0.1.0"))
    }

    async fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ServerError> {
        let schema = |value: serde_json::Value| {
            std::sync::Arc::new(value.as_object().cloned().expect("object schema"))
        };
        Ok(ListToolsResult::with_all_items(vec![
            rmcp::model::Tool::new(
                "echo",
                "echoes its message",
                schema(json!({
                    "type": "object",
                    "required": ["message"],
                    "properties": { "message": { "type": "string" } },
                })),
            ),
            rmcp::model::Tool::new(
                "die",
                "crashes the server process",
                schema(json!({ "type": "object", "properties": {} })),
            ),
        ]))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ServerError> {
        let arguments = request.arguments.clone().unwrap_or_default();
        match request.name.as_ref() {
            "echo" => {
                let message = arguments
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                Ok(CallToolResponse::from(CallToolResult::success(vec![
                    ContentBlock::text(format!("echo: {message}")),
                ])))
            }
            "die" => {
                // A genuine crash, not a clean shutdown: the transport dies
                // mid-life and every later call must fail typed.
                std::process::abort();
            }
            other => Err(ServerError::internal_error(
                format!("unknown tool {other}"),
                None,
            )),
        }
    }
}

fn main() {
    if std::env::args().any(|arg| arg == SERVER_FLAG) {
        serve_forever();
        return;
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("test runtime")
        .block_on(connect_register_call_crash_recover());
}

/// The child half: serve the echo server on stdin/stdout until the parent
/// goes away. Stderr stays inherited — MCP stdio servers log there, and the
/// runtime nulls it on its side.
fn serve_forever() {
    tokio::runtime::Runtime::new()
        .expect("server runtime")
        .block_on(async {
            let service = EchoServer
                .serve((tokio::io::stdin(), tokio::io::stdout()))
                .await
                .expect("serve over stdio");
            let _ = service.waiting().await;
        });
}

fn echo_command() -> String {
    std::env::current_exe()
        .expect("the test binary")
        .display()
        .to_string()
}

fn spec(name: &str) -> ServerSpec {
    ServerSpec {
        name: name.to_string(),
        transport: TransportSpec::Stdio {
            command: echo_command(),
            args: vec![SERVER_FLAG.to_string()],
        },
        timeout: Duration::from_secs(10),
    }
}

/// The full story, over a real stdio subprocess:
/// connect → register → call → evidence → crash → typed failure → recovery.
async fn connect_register_call_crash_recover() {
    let spec = spec("echo");

    let runtime = Arc::new(McpClientRuntime::with_client_name("argus-test"));

    // 1. Connect + discover over the real transport.
    let descriptors = runtime
        .connect(
            &spec,
            &[ToolOverride::new(
                "echo",
                argus_domain::RiskClass::Controlled,
            )],
            None,
        )
        .await
        .expect("connect to the stdio subprocess");
    let ids: Vec<String> = descriptors
        .iter()
        .map(|d| d.id().as_str().to_string())
        .collect();
    assert!(
        ids.contains(&"mcp.echo.echo".to_string()),
        "the echo tool registers: {ids:?}"
    );
    assert!(
        ids.contains(&"mcp.echo.die".to_string()),
        "the die tool registers: {ids:?}"
    );
    // The operator's override rides the descriptor; `die` stays HighRisk.
    let echo = descriptors
        .iter()
        .find(|d| d.id().as_str() == "mcp.echo.echo")
        .unwrap();
    assert_eq!(echo.risk_class(), argus_domain::RiskClass::Controlled);
    let die = descriptors
        .iter()
        .find(|d| d.id().as_str() == "mcp.echo.die")
        .unwrap();
    assert_eq!(die.risk_class(), argus_domain::RiskClass::HighRisk);
    assert!(die.requires_approval());

    // 2. A boundary-shaped call: through the executor port, evidence back.
    let controller: Arc<dyn McpToolController> = runtime.clone();
    let evidence = controller
        .call(
            &CapabilityId::new("mcp.echo.echo").unwrap(),
            &json!({ "message": "over-the-wire" }),
        )
        .expect("the tool call succeeds over stdio");
    assert_eq!(evidence["output"], "echo: over-the-wire");

    // 3. The crash: `die` aborts the child mid-life. The aborting call may
    //    return or observe the dying transport — either is honest.
    let _ = controller.call(&CapabilityId::new("mcp.echo.die").unwrap(), &json!({}));

    // 4. The next call is a typed failure — the daemon stays healthy.
    let error = tokio::time::timeout(
        Duration::from_secs(10),
        runtime.invoke(
            &CapabilityId::new("mcp.echo.echo").unwrap(),
            &json!({ "message": "x" }),
        ),
    )
    .await
    .expect("the call itself does not hang")
    .expect_err("a crashed server cannot answer");
    let message = error.to_string();
    assert!(
        matches!(
            error.to_remediation(),
            argus_executor::RemediationError::Unavailable(_)
        ),
        "the effect never began: {message}"
    );

    // 5. Recovery: the server "returns" (a fresh child), the runtime
    //    reconnects, and the SAME capability ids work again.
    runtime.disconnect("echo").await;
    let descriptors = runtime
        .connect(
            &spec,
            &[ToolOverride::new(
                "echo",
                argus_domain::RiskClass::Controlled,
            )],
            None,
        )
        .await
        .expect("reconnect after the crash");
    assert!(
        descriptors
            .iter()
            .any(|d| d.id().as_str() == "mcp.echo.echo"),
        "the tools are re-adopted"
    );
    let evidence = controller
        .call(
            &CapabilityId::new("mcp.echo.echo").unwrap(),
            &json!({ "message": "back" }),
        )
        .expect("the tool call succeeds after recovery");
    assert_eq!(evidence["output"], "echo: back");

    let _ = std::io::stdout().flush();
}
