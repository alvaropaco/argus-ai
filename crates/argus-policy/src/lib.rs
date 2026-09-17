//! Policy boundary: deterministic authorization evaluation.
//!
//! The LLM never defines what it is allowed to do; this crate evaluates
//! requests independently. The [`PolicyEvaluator`] trait is the integration
//! point for a future Cedar-backed evaluator without coupling domain contracts
//! to Cedar types (ADR-010).

mod bootstrap;

pub use bootstrap::BootstrapPolicyEvaluator;

use argus_domain::{AuthorizationRequest, PolicyDecision};

/// Evaluates an authorization request and returns a deterministic decision.
///
/// Implementations must be pure and testable independently of any LLM.
pub trait PolicyEvaluator: Send + Sync {
    fn evaluate(&self, request: &AuthorizationRequest) -> PolicyDecision;
}
