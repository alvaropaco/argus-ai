//! Errors from the decision layer.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum DecisionError {
    #[error("decision engine unavailable: {0}")]
    Unavailable(String),

    #[error("invalid decision response: {0}")]
    Invalid(String),

    #[error("decision validation failed: {0}")]
    Validation(String),
}
