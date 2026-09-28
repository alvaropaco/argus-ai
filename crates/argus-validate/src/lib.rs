//! Post-execution validator and the separate, read-only learning pass.
//!
//! The validator is pure: it compares a machine-checkable desired state against
//! the re-observed live state and returns an outcome, with no IO. The daemon
//! owns the live read (`ServiceController::is_active`), the observation
//! recording, and event publication (ADR-0031 §1). The learning pass reads
//! persisted evidence and is read-only by construction — it receives data,
//! never a plan handle (ADR-0031 §5).

pub mod learning;

use argus_domain::CapabilityId;

/// The result of comparing a machine-checkable desired state against the
/// re-observed live state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationOutcome {
    /// The observed state matches the desired state.
    Passed,
    /// The observed state differs from the desired state.
    Failed,
    /// No machine-checkable desired state exists for the capability.
    Inconclusive,
}

/// The machine-checkable desired state a capability moves its target toward,
/// when one exists: start/restart move toward active, stop toward inactive.
/// Other capabilities have no desired-state check (ADR-0031 §2).
pub fn desired_state(capability: &CapabilityId) -> Option<bool> {
    match capability.as_str() {
        CapabilityId::HOST_SERVICE_START | CapabilityId::HOST_SERVICE_RESTART => Some(true),
        CapabilityId::HOST_SERVICE_STOP => Some(false),
        _ => None,
    }
}

/// Compares a desired state against the observed state, purely.
///
/// A missing desired state is [`ValidationOutcome::Inconclusive`] (nothing to
/// compare); a desired state that matches the observation is `Passed`; anything
/// else is `Failed`.
pub fn validate(desired: Option<bool>, observed: bool) -> ValidationOutcome {
    match desired {
        Some(desired) if desired == observed => ValidationOutcome::Passed,
        Some(_) => ValidationOutcome::Failed,
        None => ValidationOutcome::Inconclusive,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(path: &str) -> CapabilityId {
        CapabilityId::new(path).unwrap()
    }

    #[test]
    fn validate_passes_when_observed_matches_desired() {
        assert_eq!(validate(Some(true), true), ValidationOutcome::Passed);
        assert_eq!(validate(Some(false), false), ValidationOutcome::Passed);
    }

    #[test]
    fn validate_fails_when_observed_differs_from_desired() {
        assert_eq!(validate(Some(true), false), ValidationOutcome::Failed);
        assert_eq!(validate(Some(false), true), ValidationOutcome::Failed);
    }

    #[test]
    fn validate_is_inconclusive_without_a_desired_state() {
        assert_eq!(validate(None, true), ValidationOutcome::Inconclusive);
        assert_eq!(validate(None, false), ValidationOutcome::Inconclusive);
    }

    #[test]
    fn desired_state_maps_service_capabilities() {
        assert_eq!(
            desired_state(&cap(CapabilityId::HOST_SERVICE_START)),
            Some(true)
        );
        assert_eq!(
            desired_state(&cap(CapabilityId::HOST_SERVICE_RESTART)),
            Some(true)
        );
        assert_eq!(
            desired_state(&cap(CapabilityId::HOST_SERVICE_STOP)),
            Some(false)
        );
    }

    #[test]
    fn desired_state_is_none_for_non_service_capabilities() {
        assert_eq!(desired_state(&cap(CapabilityId::HOST_STATUS_READ)), None);
    }
}
