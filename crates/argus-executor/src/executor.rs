//! The executor boundary trait.

use crate::action::{AuthorizedAction, ExecutionError, ExecutionResult};

/// Executes policy-authorized actions.
pub trait Executor: Send + Sync {
    fn execute(&self, action: &AuthorizedAction) -> Result<ExecutionResult, ExecutionError>;
}
