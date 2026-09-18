//! Data source for the bootstrap read-only capabilities.

use argus_domain::{CapabilityId, HealthStatus};
use serde_json::Value;

/// Supplies the read-only data backing the bootstrap capabilities.
///
/// Implemented by the daemon; the executor depends only on this trait so it
/// stays decoupled from the daemon's concrete state.
pub trait CapabilityProvider: Send + Sync {
    fn health(&self) -> HealthStatus;
    fn config(&self) -> Value;
    fn plugins(&self) -> Value;
    fn host_status(&self) -> Value;
    fn list_capabilities(&self) -> Vec<CapabilityId>;
}
