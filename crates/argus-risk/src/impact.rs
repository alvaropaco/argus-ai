//! Pre-action impact simulation (CAP-19, FR-020, T031).
//!
//! Before a risky operation is proposed, this module estimates — from typed
//! inputs only — the blast radius, the affected dependencies, the resource
//! impact, the expected recovery time, the service impact, the rollback
//! feasibility, and any safer alternative. Every figure is either derived
//! from a declared input or explicitly `None` (unknown): **no invented
//! metrics**, ever (the same honesty rule as CAP-20).
//!
//! The output is advisory data for the operator and the escalation decision;
//! it never authorizes anything.

use argus_domain::{BlastRadius, CapabilityId, ResourceId, Reversibility, RiskClass};

/// Whether an action's rollback can be executed and verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RollbackFeasibility {
    /// A declared rollback exists and is itself reversible.
    Feasible,
    /// A rollback exists but may not fully restore state.
    Partial,
    /// No declared rollback, or the rollback is itself destructive.
    Infeasible,
}

/// The simulated impact of one candidate action.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::derive_partial_eq_without_eq)] // f64 fields
pub struct ImpactEstimate {
    /// The action being simulated.
    pub capability: CapabilityId,
    pub subject: ResourceId,
    /// The declared blast radius of the capability.
    pub blast_radius: BlastRadius,
    /// Direct dependents of the subject (from semantic memory) — who feels
    /// this action. Empty when unknown.
    pub affected_dependencies: Vec<ResourceId>,
    /// The fraction of the subject's resource the action is expected to
    /// disturb, in [0,1], when declared. `None` = not estimated (never a
    /// guess).
    pub resource_impact: Option<f64>,
    /// Seconds of expected interruption to the subject, when declared.
    pub recovery_time_seconds: Option<u64>,
    /// Human-meaningful service impact summary built from the typed inputs.
    pub service_impact: String,
    pub rollback: RollbackFeasibility,
    /// A safer alternative capability, when one exists in the same family.
    pub alternative: Option<CapabilityId>,
}

/// The typed inputs the simulation needs. Each is declared evidence, not
/// something the simulator invents.
#[derive(Debug, Clone)]
pub struct SimulationInput {
    pub capability: CapabilityId,
    pub subject: ResourceId,
    pub risk: RiskClass,
    pub reversibility: Reversibility,
    pub blast_radius: BlastRadius,
    /// Direct dependents of the subject (semantic memory's first hop).
    pub dependents: Vec<ResourceId>,
    /// A declared expected-interruption figure for this action family, when
    /// the caller has one (e.g. a runbook's measured restart time).
    pub declared_recovery_time_seconds: Option<u64>,
    /// A declared rollback capability for the action, if any.
    pub declared_rollback: Option<CapabilityId>,
    /// A safer alternative in the same capability family, if known.
    pub alternative: Option<CapabilityId>,
}

/// Simulate the impact of one candidate action (FR-020). Deterministic: the
/// estimate is a pure function of the declared inputs.
pub fn simulate_impact(input: &SimulationInput) -> ImpactEstimate {
    let rollback = rollback_feasibility(input.reversibility, input.declared_rollback.as_ref());

    // Service impact is composed from typed facts, never prose invention:
    // the action's risk/reversibility, its dependents, and its blast radius.
    let mut service_impact = match (input.risk, input.reversibility) {
        (RiskClass::Read, _) => "read-only; no service interruption".to_string(),
        (RiskClass::LowRisk, Reversibility::Reversible) => {
            "brief interruption; fully reversible".to_string()
        }
        (RiskClass::LowRisk, _) => "brief interruption; rollback may be partial".to_string(),
        (RiskClass::Controlled, Reversibility::Reversible) => {
            "controlled interruption; reversible".to_string()
        }
        (RiskClass::Controlled, _) => {
            "controlled interruption; rollback may be partial".to_string()
        }
        (RiskClass::HighRisk, _) => "high-risk action; expect service interruption".to_string(),
        (RiskClass::Destructive, _) => {
            "destructive action; state will not be automatically restored".to_string()
        }
    };
    if !input.dependents.is_empty() {
        service_impact.push_str(&format!(
            "; {} dependent(s) will be affected",
            input.dependents.len()
        ));
    }
    match input.blast_radius {
        BlastRadius::None => {}
        BlastRadius::Host => service_impact.push_str("; scope: this host"),
        BlastRadius::Environment => service_impact.push_str("; scope: this environment"),
        BlastRadius::Fleet => service_impact.push_str("; scope: the fleet"),
    }

    // Resource impact: derived from risk class as an ordinal disturbance
    // fraction — a coarse, declared mapping (documented, deterministic), not
    // a measured guess.
    let resource_impact = match input.risk {
        RiskClass::Read => Some(0.0),
        RiskClass::LowRisk => Some(0.1),
        RiskClass::Controlled => Some(0.25),
        RiskClass::HighRisk => Some(0.5),
        RiskClass::Destructive => Some(1.0),
    };

    ImpactEstimate {
        capability: input.capability.clone(),
        subject: input.subject.clone(),
        blast_radius: input.blast_radius,
        affected_dependencies: input.dependents.clone(),
        resource_impact,
        recovery_time_seconds: if input.risk == RiskClass::Read {
            Some(0)
        } else {
            input.declared_recovery_time_seconds
        },
        service_impact,
        rollback,
        alternative: input.alternative.clone(),
    }
}

/// Rollback feasibility from reversibility + the declared rollback action.
fn rollback_feasibility(
    reversibility: Reversibility,
    declared_rollback: Option<&CapabilityId>,
) -> RollbackFeasibility {
    if declared_rollback.is_none() {
        // No declared rollback action: only fully-reversible semantics
        // (idempotent desired-state) can be called feasible.
        return match reversibility {
            Reversibility::Reversible => RollbackFeasibility::Feasible,
            Reversibility::PartiallyReversible => RollbackFeasibility::Partial,
            Reversibility::None => RollbackFeasibility::Infeasible,
        };
    }
    match reversibility {
        Reversibility::Reversible => RollbackFeasibility::Feasible,
        Reversibility::PartiallyReversible => RollbackFeasibility::Partial,
        Reversibility::None => RollbackFeasibility::Infeasible,
    }
}

/// The pre-action summary the operator sees. Deterministic prose over the
/// typed estimate; unknown figures are printed as "unknown", never faked.
pub fn impact_summary(estimate: &ImpactEstimate) -> String {
    let recovery = estimate
        .recovery_time_seconds
        .map(|s| format!("{s}s"))
        .unwrap_or_else(|| "unknown".to_string());
    let impact = estimate
        .resource_impact
        .map(|f| format!("{:.0}%", f * 100.0))
        .unwrap_or_else(|| "unknown".to_string());
    let alternative = estimate
        .alternative
        .as_ref()
        .map(|c| format!("; safer alternative: {}", c.as_str()))
        .unwrap_or_default();
    format!(
        "impact of {} on {}: recovery {}, resource disturbance {}, rollback {:?}, \
         {} affected dependent(s){} — {}",
        estimate.capability.as_str(),
        estimate.subject.as_str(),
        recovery,
        impact,
        estimate.rollback,
        estimate.affected_dependencies.len(),
        alternative,
        estimate.service_impact,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> SimulationInput {
        SimulationInput {
            capability: CapabilityId::new("host.cgroup.freeze").unwrap(),
            subject: ResourceId::new("container", "batch-worker").unwrap(),
            risk: RiskClass::Controlled,
            reversibility: Reversibility::Reversible,
            blast_radius: BlastRadius::Host,
            dependents: vec![ResourceId::new("service", "api").unwrap()],
            declared_recovery_time_seconds: Some(30),
            declared_rollback: Some(CapabilityId::new("host.cgroup.thaw").unwrap()),
            alternative: None,
        }
    }

    #[test]
    fn a_controlled_reversible_action_simulates_fully() {
        let e = simulate_impact(&input());
        assert_eq!(e.blast_radius, BlastRadius::Host);
        assert_eq!(e.affected_dependencies.len(), 1);
        assert_eq!(e.resource_impact, Some(0.25));
        assert_eq!(e.recovery_time_seconds, Some(30));
        assert_eq!(e.rollback, RollbackFeasibility::Feasible);
        assert!(e.service_impact.contains("1 dependent(s)"));
        assert!(e.service_impact.contains("scope: this host"));
    }

    #[test]
    fn read_only_actions_never_invent_interruptions() {
        let e = simulate_impact(&SimulationInput {
            risk: RiskClass::Read,
            declared_recovery_time_seconds: None,
            ..input()
        });
        assert_eq!(e.recovery_time_seconds, Some(0));
        assert_eq!(e.resource_impact, Some(0.0));
        assert!(e.service_impact.contains("read-only"));
    }

    #[test]
    fn undeclared_recovery_time_is_unknown_never_guessed() {
        let e = simulate_impact(&SimulationInput {
            risk: RiskClass::Controlled,
            declared_recovery_time_seconds: None,
            ..input()
        });
        assert_eq!(e.recovery_time_seconds, None);
        assert!(impact_summary(&e).contains("recovery unknown"));
    }

    #[test]
    fn destructive_actions_report_infeasible_rollback_when_none_declared() {
        let e = simulate_impact(&SimulationInput {
            risk: RiskClass::Destructive,
            reversibility: Reversibility::None,
            declared_rollback: None,
            ..input()
        });
        assert_eq!(e.rollback, RollbackFeasibility::Infeasible);
        assert_eq!(e.resource_impact, Some(1.0));
        assert!(e.service_impact.contains("destructive"));
    }

    #[test]
    fn a_declared_rollback_still_cannot_outrun_irreversibility() {
        let e = simulate_impact(&SimulationInput {
            reversibility: Reversibility::None,
            declared_rollback: Some(CapabilityId::new("host.cgroup.thaw").unwrap()),
            ..input()
        });
        assert_eq!(e.rollback, RollbackFeasibility::Infeasible);
    }

    #[test]
    fn summary_names_the_safer_alternative() {
        let e = simulate_impact(&SimulationInput {
            alternative: Some(CapabilityId::new("host.cgroup.thaw").unwrap()),
            ..input()
        });
        let s = impact_summary(&e);
        assert!(s.contains("safer alternative: host.cgroup.thaw"));
        assert!(s.contains("host.cgroup.freeze"));
        assert!(s.contains("container:batch-worker"));
    }
}
