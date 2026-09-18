//! Authorized actions and execution results.

use argus_domain::{CapabilityId, CapabilityRequest, PolicyDecision, PolicyOutcome};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;

/// An action that has been authorized by policy.
///
/// Constructing an [`AuthorizedAction`] enforces that the decision is `Allow`,
/// so the executor can never receive a non-allowed action.
#[derive(Debug, Clone)]
pub struct AuthorizedAction {
    request: CapabilityRequest,
    decision: PolicyDecision,
}

impl AuthorizedAction {
    pub fn new(
        request: CapabilityRequest,
        decision: PolicyDecision,
    ) -> Result<Self, ExecutionError> {
        if decision.outcome != PolicyOutcome::Allow {
            return Err(ExecutionError::NotAuthorized(decision.outcome));
        }
        Ok(Self { request, decision })
    }

    pub fn request(&self) -> &CapabilityRequest {
        &self.request
    }

    pub fn decision(&self) -> &PolicyDecision {
        &self.decision
    }

    pub fn capability(&self) -> &CapabilityId {
        &self.request.capability
    }
}

/// The outcome of executing an authorized action.
#[derive(Debug, Clone, Serialize)]
pub struct ExecutionResult {
    pub capability: CapabilityId,
    pub evidence: Value,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
}

/// Errors from the executor boundary.
#[derive(Debug, thiserror::Error)]
pub enum ExecutionError {
    #[error("action is not authorized (outcome: {0:?})")]
    NotAuthorized(PolicyOutcome),

    #[error("capability '{0}' is not supported by this executor")]
    Unsupported(CapabilityId),

    #[error("execution failed: {0}")]
    Failed(String),
}
