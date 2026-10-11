//! MCP runtime abstraction (the `rmcp` integration point; ADR-005, spec 014).

use crate::error::DomainError;
use crate::{CapabilityDescriptor, CapabilityId};

/// Abstraction over an MCP runtime.
///
/// Native MCP capabilities share the ARGUS lifecycle, security, configuration,
/// and observability (ADR-005). The concrete `rmcp`-backed implementation is
/// the `argus-mcp` crate (ADR-0044 §4); the domain and daemon depend only on
/// this trait, so the protocol implementation is swappable and the daemon's
/// tests run against a deterministic fake.
///
/// Every method takes `&self`: a runtime is shared between the daemon's
/// executor and its per-server lifecycle tasks, so implementations provide
/// interior mutability.
pub trait McpRuntime: Send + Sync {
    /// Registers (or refreshes) a capability exposed by an MCP server.
    ///
    /// Registering an id that is already known refreshes the stored descriptor
    /// in place: a reconnecting server may re-advertise its tools and the
    /// refresh must be idempotent. (The [`CapabilityRegistry`](crate::CapabilityRegistry)'s
    /// duplicate rejection is a separate, stricter contract — this trait's
    /// table is the runtime's own bookkeeping, not the authorization surface.)
    fn register(&self, descriptor: CapabilityDescriptor) -> Result<(), DomainError>;

    /// Lists the registered capability ids.
    fn capabilities(&self) -> Vec<CapabilityId>;

    /// The registered descriptors, in unspecified order.
    fn descriptors(&self) -> Vec<CapabilityDescriptor>;

    /// Forgets every capability of the named server (`metrics` in
    /// `mcp.metrics.*`) and returns the removed ids. A server that is disabled
    /// or removed from the configuration contributes nothing after this.
    fn unregister_server(&self, server: &str) -> Vec<CapabilityId>;
}
