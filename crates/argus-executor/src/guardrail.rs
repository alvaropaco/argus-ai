//! Execution-time guardrail predicates and their registry (ADR-0028 §1-§3).
//!
//! Schema validation is structural only; guardrails are the safety layer that
//! runs against a target re-resolved at execution time, immediately before the
//! effect. Predicates are deterministic, pure functions of the resolved target
//! and carry no host I/O, so behavior is testable without a live model or a real
//! host (Principle 9).

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use argus_domain::CapabilityId;

/// Why a guardrail refused an action.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("guardrail refused '{capability}': {reason}")]
pub struct GuardrailViolation {
    pub capability: CapabilityId,
    pub reason: String,
}

/// A deterministic predicate over a target re-resolved at execution time.
///
/// `Ok(())` means the target is safe; `Err(reason)` means it must be refused.
/// Predicates are consulted inside the privileged executor immediately before
/// the effect, on the value freshly re-read from the action — never on a
/// plan-time value (ADR-0028 §2).
pub trait Guardrail: Send + Sync {
    fn check(&self, target: &str) -> Result<(), String>;
}

/// A registry of guardrail predicates keyed by [`CapabilityId`].
///
/// Keying by capability keeps [`crate::AuthorizedAction`] and `CapabilityRequest`
/// unchanged: the daemon wires predicates when it builds the executor, and the
/// executor consults the registry at the single execution boundary.
#[derive(Default)]
pub struct GuardrailRegistry {
    by_capability: HashMap<CapabilityId, Vec<Arc<dyn Guardrail>>>,
}

impl GuardrailRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a predicate for a capability. Multiple predicates per
    /// capability are consulted in registration order.
    pub fn register(&mut self, capability: CapabilityId, guardrail: Arc<dyn Guardrail>) {
        self.by_capability
            .entry(capability)
            .or_default()
            .push(guardrail);
    }

    /// Runs every predicate registered for `capability` against the resolved
    /// target. The first refusal wins; a capability with no predicates is safe.
    pub fn check(&self, capability: &CapabilityId, target: &str) -> Result<(), GuardrailViolation> {
        let Some(predicates) = self.by_capability.get(capability) else {
            return Ok(());
        };
        for predicate in predicates {
            predicate
                .check(target)
                .map_err(|reason| GuardrailViolation {
                    capability: capability.clone(),
                    reason,
                })?;
        }
        Ok(())
    }
}

/// Refuses a target naming the daemon's own unit.
///
/// The unit name is configurable so the daemon can pass its own name; it
/// defaults to `argusd`. Both the bare name and its `.service` form are refused.
#[derive(Debug, Clone)]
pub struct NeverTargetArgusd {
    unit: String,
}

impl NeverTargetArgusd {
    pub fn new(unit: impl Into<String>) -> Self {
        Self { unit: unit.into() }
    }
}

impl Default for NeverTargetArgusd {
    fn default() -> Self {
        Self::new("argusd")
    }
}

impl Guardrail for NeverTargetArgusd {
    fn check(&self, target: &str) -> Result<(), String> {
        let unit = target.trim();
        if unit == self.unit || unit == format!("{}.service", self.unit) {
            return Err(format!("refusing to target the daemon's own unit '{unit}'"));
        }
        Ok(())
    }
}

/// Refuses an empty or malformed service unit name (no control or whitespace).
#[derive(Debug, Clone, Default)]
pub struct ValidServiceUnit;

impl Guardrail for ValidServiceUnit {
    fn check(&self, target: &str) -> Result<(), String> {
        if !is_valid_unit_name(target) {
            return Err(format!("'{target}' is not a valid service unit name"));
        }
        Ok(())
    }
}

/// A systemd unit name is non-empty, free of control characters and whitespace,
/// and not a path segment (`/`, `\`, `..`) or an option (a leading `-`).
fn is_valid_unit_name(unit: &str) -> bool {
    !unit.is_empty()
        && unit != ".."
        && !unit.starts_with('-')
        && !unit.contains('/')
        && !unit.contains('\\')
        && !unit.chars().any(char::is_control)
        && !unit.chars().any(char::is_whitespace)
}

/// Refuses a target outside a daemon-supplied allowed set.
///
/// Fail-closed: an empty set refuses everything. The daemon registers this
/// predicate only when it configures a non-empty allowed set (ADR-0028 §3).
#[derive(Debug, Clone)]
pub struct AllowedTargets {
    allowed: BTreeSet<String>,
}

impl AllowedTargets {
    pub fn new(allowed: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            allowed: allowed.into_iter().map(Into::into).collect(),
        }
    }
}

impl Guardrail for AllowedTargets {
    fn check(&self, target: &str) -> Result<(), String> {
        if !self.allowed.contains(target) {
            return Err(format!("'{target}' is not in the allowed target set"));
        }
        Ok(())
    }
}

/// Refuses a PID of 1 or lower.
///
/// A reusable predicate for a future `host.process.signal` capability. Milestone
/// 1 exposes no process capability, so this is tested in isolation rather than
/// registered (ADR-0028 §3).
pub fn pid_above_one(pid: i64) -> Result<(), String> {
    if pid <= 1 {
        return Err(format!(
            "refusing to signal PID {pid}: PID must be greater than 1"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_target_argusd_refuses_the_daemon_unit_and_its_service_form() {
        let guardrail = NeverTargetArgusd::default();
        assert!(guardrail.check("argusd").is_err());
        assert!(guardrail.check("argusd.service").is_err());
        assert!(guardrail.check("nginx.service").is_ok());
        assert!(guardrail.check("argusd-worker").is_ok());
    }

    #[test]
    fn valid_service_unit_refuses_empty_and_malformed_names() {
        let guardrail = ValidServiceUnit;
        assert!(guardrail.check("").is_err());
        assert!(guardrail.check("  ").is_err());
        assert!(guardrail.check("nginx service").is_err());
        assert!(guardrail.check("nginx\n.service").is_err());
        assert!(guardrail.check("nginx.service").is_ok());
    }

    #[test]
    fn valid_service_unit_refuses_paths_and_options() {
        let guardrail = ValidServiceUnit;
        for bad in ["a/b", "a\\b", "..", "-nginx", "/etc/passwd"] {
            assert!(guardrail.check(bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn allowed_targets_refuses_non_members_and_fails_closed_when_empty() {
        let allowed = AllowedTargets::new(["nginx.service", "postgres.service"]);
        assert!(allowed.check("nginx.service").is_ok());
        assert!(allowed.check("redis.service").is_err());

        let empty = AllowedTargets::new(std::iter::empty::<&str>());
        assert!(empty.check("nginx.service").is_err());
    }

    #[test]
    fn pid_above_one_refuses_pids_of_one_or_lower() {
        assert!(pid_above_one(1).is_err());
        assert!(pid_above_one(0).is_err());
        assert!(pid_above_one(-5).is_err());
        assert!(pid_above_one(2).is_ok());
        assert!(pid_above_one(1234).is_ok());
    }

    #[test]
    fn registry_runs_every_predicate_and_stops_at_the_first_refusal() {
        let capability = CapabilityId::new("host.service.restart").unwrap();
        let mut registry = GuardrailRegistry::new();
        registry.register(capability.clone(), Arc::new(ValidServiceUnit));
        registry.register(capability.clone(), Arc::new(NeverTargetArgusd::default()));

        // Both pass: no violation.
        assert!(registry.check(&capability, "nginx.service").is_ok());

        // First predicate fails: the violation names the capability.
        let violation = registry.check(&capability, "nginx service").unwrap_err();
        assert_eq!(violation.capability, capability);
        assert!(violation.reason.contains("valid service unit"));

        // Second predicate fails on a well-formed name.
        assert!(registry.check(&capability, "argusd").is_err());
    }

    #[test]
    fn registry_is_open_for_unregistered_capabilities() {
        let registry = GuardrailRegistry::new();
        let capability = CapabilityId::new("host.process.signal").unwrap();
        assert!(registry.check(&capability, "123").is_ok());
    }
}
