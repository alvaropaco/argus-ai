//! Protocol-level codes are modelled in [`crate::protocol::errors`]; this type
//! is the single error surface callers see.

use crate::transport::TransportError;

#[derive(Debug, thiserror::Error)]
pub enum CloudError {
    #[error(transparent)]
    Transport(#[from] TransportError),

    #[error(transparent)]
    Mapping(#[from] MappingError),

    #[error("protocol rejected the message: {0}")]
    Protocol(String),
}

/// A local value could not be projected onto the wire.
///
/// The only current cause is exceeding a contract bound, which the installation
/// must refuse itself rather than let the cloud reject (FR-019).
#[derive(Debug, thiserror::Error)]
pub enum MappingError {
    #[error("{kind} exceeds the contract bound: {actual} > {limit}")]
    BoundExceeded {
        kind: &'static str,
        actual: usize,
        limit: usize,
    },

    #[error("cannot encode {kind}: {detail}")]
    Encoding { kind: &'static str, detail: String },
}

impl MappingError {
    pub fn bound(kind: &'static str, actual: usize, limit: usize) -> Self {
        Self::BoundExceeded {
            kind,
            actual,
            limit,
        }
    }

    pub fn encoding(kind: &'static str, detail: impl Into<String>) -> Self {
        Self::Encoding {
            kind,
            detail: detail.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bound_error_names_the_limit() {
        let err = MappingError::bound("capabilities", 1200, 1000);
        let text = err.to_string();
        assert!(text.contains("capabilities"), "{text}");
        assert!(text.contains("1200"), "{text}");
        assert!(text.contains("1000"), "{text}");
    }

    #[test]
    fn transport_errors_convert_into_cloud_errors() {
        let err: CloudError = TransportError::Closed.into();
        assert!(matches!(err, CloudError::Transport(_)));
    }
}
