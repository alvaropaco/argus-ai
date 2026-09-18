//! Event errors.

/// Errors from the event bus.
#[derive(Debug, thiserror::Error)]
pub enum EventError {
    #[error("event publish failed: {0}")]
    PublishFailed(String),
}
