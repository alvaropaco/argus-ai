//! Domain validation errors.

/// Errors produced when constructing or validating domain values.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[allow(clippy::derive_partial_eq_without_eq)] // ConfidenceOutOfRange holds f32
pub enum DomainError {
    /// A [`ResourceId`](crate::ResourceId) did not match the `kind:identifier` grammar.
    #[error(
        "invalid resource id '{0}': expected a lowercase-kebab kind and a non-empty, control-free identifier separated by ':'"
    )]
    InvalidResourceId(String),

    /// A [`CapabilityId`](crate::CapabilityId) did not match the dotted-path grammar.
    #[error(
        "invalid capability id '{0}': expected a dotted lowercase path with at least two segments"
    )]
    InvalidCapabilityId(String),

    /// An [`EventType`](crate::EventType) did not match the dotted-path grammar.
    #[error(
        "invalid event type '{0}': expected a dotted lowercase path with at least two segments"
    )]
    InvalidEventType(String),

    /// A confidence value fell outside `[0.0, 1.0]`.
    #[error("confidence {0} out of range: expected 0.0..=1.0")]
    ConfidenceOutOfRange(f32),
}
