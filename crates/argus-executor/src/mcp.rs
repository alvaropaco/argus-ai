//! The MCP tool-execution port (spec 014, ADR-0044).
//!
//! An external MCP tool is a capability of unknown provenance: it enters the
//! registry like any capability and executes only through
//! [`AuthorizedAction`](crate::AuthorizedAction) like any effect. This port is
//! the executor-side seam the `rmcp`-backed `argus-mcp` runtime implements;
//! [`RemediationExecutor`](crate::RemediationExecutor) routes every
//! `mcp.<server>.<tool>` capability through it.

use argus_domain::CapabilityId;
use serde_json::Value;

use crate::remediation::RemediationError;

/// Invokes an external MCP tool backing an `mcp.<server>.<tool>` capability.
///
/// The port is synchronous like its sibling ports; the `rmcp`-backed
/// implementation bridges the async transport at the call site (the same
/// hand-off the daemon's D-Bus journal controller uses on the multi-threaded
/// runtime). The returned value is the already-bounded execution evidence:
/// the implementation serializes the tool's output and caps its size, so a
/// crashed or malicious server cannot exhaust memory past this boundary
/// (spec 014 NFR).
pub trait McpToolController: Send + Sync {
    /// Calls the tool behind `capability` with the authorized step's
    /// `arguments`.
    ///
    /// `Err(RemediationError::Unavailable)` means the effect never began — the
    /// server is disconnected, the call timed out, or no runtime is wired.
    /// `Err(RemediationError::Failed)` means the tool itself reported a
    /// failure.
    fn call(&self, capability: &CapabilityId, arguments: &Value)
    -> Result<Value, RemediationError>;
}

/// The fail-closed stand-in wired until a runtime is connected: every MCP
/// effect reports `Unavailable` rather than pretending to succeed.
#[derive(Debug, Default)]
pub struct NoMcpTools;

impl McpToolController for NoMcpTools {
    fn call(
        &self,
        capability: &CapabilityId,
        _arguments: &Value,
    ) -> Result<Value, RemediationError> {
        Err(RemediationError::Unavailable(format!(
            "no MCP runtime is wired; '{}' cannot execute",
            capability.as_str()
        )))
    }
}

/// Whether `capability` names an external MCP tool (spec 014). The `mcp.`
/// prefix is the namespace contract: `mcp.<server>.<tool>`, collision-free by
/// construction (ADR-0044 §2).
pub fn is_mcp_capability(capability: &CapabilityId) -> bool {
    capability.as_str().starts_with("mcp.")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capability(path: &str) -> CapabilityId {
        CapabilityId::new(path).expect("valid capability id")
    }

    #[test]
    fn only_mcp_prefixed_ids_are_mcp_capabilities() {
        assert!(is_mcp_capability(&capability("mcp.metrics.query")));
        assert!(is_mcp_capability(&capability("mcp.my-server.get_weather")));
        assert!(!is_mcp_capability(&capability("host.service.restart")));
        assert!(!is_mcp_capability(&capability("k8s.pod.restart")));
        // A prefix is not a substring: the check is anchored at the front.
        assert!(!is_mcp_capability(&capability("xmcp.metrics.query")));
    }

    #[test]
    fn no_mcp_tools_is_the_fail_closed_stand_in() {
        let controller = NoMcpTools;
        let error = controller
            .call(&capability("mcp.metrics.query"), &serde_json::json!({}))
            .unwrap_err();
        assert!(
            matches!(error, RemediationError::Unavailable(_)),
            "an unwired runtime must degrade, never fake success: {error:?}"
        );
    }
}
