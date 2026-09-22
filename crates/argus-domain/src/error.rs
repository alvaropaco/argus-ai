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

    /// A plugin manifest was malformed or failed validation.
    #[error("invalid plugin manifest: {0}")]
    InvalidPluginManifest(String),

    /// A capability was registered more than once.
    #[error("duplicate capability registration: {0}")]
    DuplicateCapability(String),

    /// A privileged capability declared no OS privileges.
    ///
    /// A capability that needs privilege but declares none cannot be run
    /// least-privilege, so it is rejected at registration rather than at the
    /// moment of invocation (ADR-0021 §1).
    #[error("capability '{0}' is privileged but declares no OS privileges")]
    UndeclaredPrivileges(String),

    /// A capability requires privileges the running sandbox does not grant.
    ///
    /// Registration fails closed: advertising a capability the installation
    /// cannot actually perform would mislead the operator (ADR-0021 §2).
    #[error("capability '{0}' requires privileges the sandbox does not grant: {1}")]
    UngrantedPrivileges(String, String),
}
