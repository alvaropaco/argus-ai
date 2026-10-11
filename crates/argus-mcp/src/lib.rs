//! ARGUS MCP client runtime (spec 014, ADR-0044).
//!
//! The `rmcp`-backed implementation behind the domain's [`McpRuntime`] seam
//! and the executor's [`McpToolController`] port: it connects the daemon to
//! configured MCP servers (stdio subprocess or streamable HTTP), discovers
//! their tools, adopts each as a typed capability descriptor —
//! `mcp.<server>.<tool>`, the tool's own JSON Schema, **HighRisk by
//! default** — and invokes them with a timeout and a bounded-evidence cap.
//!
//! The boundary is the boundary (ADR-0044 §3): nothing here authorizes a
//! call. The runtime only ever executes what an already-authorized step asks
//! it to, and its failures are typed.
//!
//! - `McpError` — the typed failures (fail-closed everywhere).
//! - `McpClientRuntime` — the runtime; one per daemon.
//! - `ServerSpec` / `TransportSpec` / `ToolOverride` — the daemon's `[mcp]`
//!   config, mapped onto the runtime's vocabulary.

mod error;
mod runtime;

pub use error::McpError;
pub use runtime::{EVIDENCE_CAP_BYTES, McpClientRuntime, ServerSpec, ToolOverride, TransportSpec};
