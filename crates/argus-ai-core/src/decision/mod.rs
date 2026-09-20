//! Structured-decision engine and reasoning gateway.

pub mod adapters;
pub mod author;
pub mod autonomy;
pub mod context;
pub mod error;
pub mod gateway;
pub mod provider;
pub mod types;
pub mod validate;

pub use author::DecisionSet;
pub use context::{ContextBuilder, EvidenceEntry};
pub use error::DecisionError;
pub use gateway::{ReasoningGateway, aggregate_confidence, propose_plan};
pub use provider::DecisionProvider;
pub use types::{
    DecisionAnswer, DecisionQuestion, DecisionRequest, DecisionResponse, NoulCriteria,
};
