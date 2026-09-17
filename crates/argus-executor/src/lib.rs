//! Typed, policy-authorized execution boundary.
//!
//! The only component allowed to cross the privileged execution boundary.
//! Actions can only be executed after carrying an `Allow` policy decision;
//! the bootstrap exposes read-only capabilities only (no arbitrary shell).

mod action;
mod bootstrap;
mod executor;
mod provider;

pub use action::{AuthorizedAction, ExecutionError, ExecutionResult};
pub use bootstrap::BootstrapExecutor;
pub use executor::Executor;
pub use provider::CapabilityProvider;
