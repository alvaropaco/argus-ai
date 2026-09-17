//! MCP runtime abstraction (the future `rmcp` integration point).

use crate::error::DomainError;
use crate::{CapabilityDescriptor, CapabilityId};

/// Abstraction over an MCP runtime.
///
/// Native MCP capabilities share the ARGUS lifecycle, security, configuration,
/// and observability (ADR-005). The concrete `rmcp`-backed implementation is
/// added in a later phase; this trait defines the integration surface so the
/// domain and daemon never depend on a specific MCP implementation.
pub trait McpRuntime {
    /// Registers a capability exposed by an MCP server or plugin.
    fn register(&mut self, descriptor: CapabilityDescriptor) -> Result<(), DomainError>;

    /// Lists the registered capability ids.
    fn capabilities(&self) -> Vec<CapabilityId>;
}
