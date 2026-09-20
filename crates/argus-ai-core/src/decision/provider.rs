//! The decision-provider boundary (ADR-014 amendment).

use async_trait::async_trait;

use crate::decision::error::DecisionError;
use crate::decision::types::{DecisionRequest, DecisionResponse};

/// A structured-decision engine ("System One").
///
/// The core depends only on this trait; a concrete engine (a local `simple-jev`
/// sidecar or a hosted endpoint) is an adapter. Decision output is data, never
/// authority.
#[async_trait]
pub trait DecisionProvider: Send + Sync {
    async fn decide(&self, request: DecisionRequest) -> Result<DecisionResponse, DecisionError>;
}
