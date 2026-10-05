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
use serde_json::Value;

/// The published validation event types (`contracts/events.md`).
///
/// These are the transport-independent contract strings; they live here so the
/// validator, the learning pass, and both daemon hooks share one source of
/// truth. `argus-events::types` re-declares the same literals for the async bus,
/// which this crate deliberately does not depend on.
pub const VALIDATION_PASSED: &str = "validation.passed";
pub const VALIDATION_FAILED: &str = "validation.failed";
/// A live-state re-observation read failed; no validation pass is fabricated.
pub const VALIDATION_READ_FAILED: &str = "brain.validation.read.failed";

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
/// when one exists: start/restart move toward active, stop toward inactive, a
/// container restart toward running, a freeze toward frozen, a thaw toward
/// thawed. Other capabilities have no desired-state check (ADR-0031 §2).
///
/// The boolean reads as "the target is in its desired operational state"; the
/// family-specific live read (unit active, container running, cgroup frozen)
/// is the daemon's, dispatched per family.
pub fn desired_state(capability: &CapabilityId) -> Option<bool> {
    match capability.as_str() {
        CapabilityId::HOST_SERVICE_START | CapabilityId::HOST_SERVICE_RESTART => Some(true),
        CapabilityId::HOST_SERVICE_STOP => Some(false),
        CapabilityId::CONTAINER_RESTART => Some(true),
        CapabilityId::HOST_CGROUP_FREEZE => Some(true),
        CapabilityId::HOST_CGROUP_THAW => Some(false),
        // Kubernetes (spec 003 M4): a restarted/deleted pod is desired gone
        // from its current (unhealthy) incarnation — restart re-runs it, so
        // the machine-checkable state is "running again"; delete is "gone".
        CapabilityId::K8S_POD_RESTART => Some(true),
        CapabilityId::K8S_POD_DELETE => Some(false),
        CapabilityId::K8S_DEPLOYMENT_RESTART | CapabilityId::K8S_DEPLOYMENT_ROLLBACK => Some(true),
        CapabilityId::K8S_NODE_CORDON => Some(false),
        CapabilityId::K8S_NODE_UNCORDON => Some(true),
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

/// The `(event_type, payload)` for a re-observed step's validation outcome, or
/// `None` when the capability has no machine-checkable desired state (nothing to
/// publish).
///
/// This is the single payload construction shared by both daemon hooks — the
/// host-health loop and the plan step loop — so the two cannot drift
/// (ADR-0031 §6).
pub fn validation_event(
    capability: &CapabilityId,
    unit: &str,
    expected: bool,
    observed: bool,
    context_hash: &str,
) -> Option<(&'static str, Value)> {
    match validate(Some(expected), observed) {
        ValidationOutcome::Passed => Some((
            VALIDATION_PASSED,
            serde_json::json!({
                "plan_id": context_hash,
                "capability": capability.as_str(),
                "evidence": { "unit": unit, "observed": observed, "desired": expected },
            }),
        )),
        ValidationOutcome::Failed => Some((
            VALIDATION_FAILED,
            serde_json::json!({
                "plan_id": context_hash,
                "capability": capability.as_str(),
                "reason": format!("observed {observed}, desired {expected}"),
            }),
        )),
        ValidationOutcome::Inconclusive => None,
    }
}

/// The payload for a failed live-state read. No pass is fabricated; the failure
/// is recorded instead (ADR-0031 §3).
pub fn read_failure_event(
    capability: &CapabilityId,
    unit: &str,
    context_hash: &str,
    reason: &str,
) -> Value {
    serde_json::json!({
        "plan_id": context_hash,
        "capability": capability.as_str(),
        "unit": unit,
        "reason": reason,
    })
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

    #[test]
    fn desired_state_maps_remediation_capabilities() {
        assert_eq!(
            desired_state(&cap(CapabilityId::CONTAINER_RESTART)),
            Some(true),
            "a restarted container is desired running"
        );
        assert_eq!(
            desired_state(&cap(CapabilityId::HOST_CGROUP_FREEZE)),
            Some(true),
            "a frozen subtree is the freeze's desired state"
        );
        assert_eq!(
            desired_state(&cap(CapabilityId::HOST_CGROUP_THAW)),
            Some(false),
            "a thawed subtree is the thaw's desired state"
        );
        assert_eq!(
            desired_state(&cap(CapabilityId::HOST_PROCESS_SIGNAL)),
            None,
            "a signal has no machine-checkable desired state"
        );
    }

    #[test]
    fn validation_event_builds_pass_and_fail_payloads() {
        let cap = cap(CapabilityId::HOST_SERVICE_RESTART);
        let (kind, payload) = validation_event(&cap, "nginx.service", true, true, "hash").unwrap();
        assert_eq!(kind, VALIDATION_PASSED);
        assert_eq!(payload["plan_id"], "hash");
        assert_eq!(payload["evidence"]["observed"], true);

        let (kind, payload) = validation_event(&cap, "nginx.service", true, false, "hash").unwrap();
        assert_eq!(kind, VALIDATION_FAILED);
        assert_eq!(payload["plan_id"], "hash");
    }

    #[test]
    fn read_failure_event_binds_the_failure_to_the_plan() {
        let cap = cap(CapabilityId::HOST_SERVICE_RESTART);
        let payload = read_failure_event(&cap, "nginx.service", "hash", "systemd down");
        assert_eq!(payload["plan_id"], "hash");
        assert_eq!(payload["unit"], "nginx.service");
        assert_eq!(payload["reason"], "systemd down");
    }
}
