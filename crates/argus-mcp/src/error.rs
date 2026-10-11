//! Typed MCP runtime failures (spec 014: fail-closed everywhere).
//!
//! A crashed, timed-out, or disconnected server is a typed error — never a
//! silent success, never a daemon crash. The executor-facing mapping puts
//! "the effect never began" (`Unavailable`) apart from "the tool reported a
//! failure" (`Failed`), so the ledger records what honestly happened.

use argus_executor::RemediationError;

/// Errors from the MCP client runtime.
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("mcp server '{0}' is not connected")]
    NotConnected(String),

    #[error("mcp server '{server}' exposes no tool '{tool}'")]
    UnknownTool { server: String, tool: String },

    #[error("mcp call '{server}.{tool}' timed out after {timeout_secs}s")]
    Timeout {
        server: String,
        tool: String,
        timeout_secs: u64,
    },

    #[error("mcp server '{server}' timed out while connecting after {timeout_secs}s")]
    ConnectTimeout { server: String, timeout_secs: u64 },

    #[error("mcp transport error: {0}")]
    Transport(String),

    #[error("mcp tool '{server}.{tool}' failed: {message}")]
    Tool {
        server: String,
        tool: String,
        message: String,
    },

    #[error("mcp capability '{0}' arguments violate its declared schema")]
    InvalidArguments(String),

    #[error("invalid mcp capability id '{0}': expected mcp.<server>.<tool>")]
    InvalidCapabilityId(String),
}

impl McpError {
    /// The executor-boundary classification: did the effect begin?
    ///
    /// `Unavailable` — not connected, timed out, or transport trouble: the
    /// effect never (or only possibly) began, which is degradation.
    /// `Failed` — the tool itself reported a failure, or the id does not name
    /// a real tool: the call was made and refused.
    pub fn to_remediation(&self) -> RemediationError {
        match self {
            Self::NotConnected(_)
            | Self::Timeout { .. }
            | Self::ConnectTimeout { .. }
            | Self::Transport(_) => RemediationError::Unavailable(self.to_string()),
            Self::UnknownTool { .. }
            | Self::Tool { .. }
            | Self::InvalidCapabilityId(_)
            | Self::InvalidArguments(_) => RemediationError::Failed(self.to_string()),
        }
    }
}
