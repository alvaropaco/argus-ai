//! Resource-autopilot governance (spec 003 M3, CAP-15, FR-017).
//!
//! The governor is the policy layer's answer to "may the autopilot adjust this
//! resource, now?". It is deterministic and denies by default: a resource the
//! operator has not classified is never adjusted, protected resources are never
//! adjusted, and even permitted adjustments are bounded by a budget per window
//! and a per-subject cooldown. It decides nothing about *what* to do — a
//! refused adjustment is recorded and nothing executes; an allowed adjustment
//! still crosses the ordinary policy → guardrail → executor boundary.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

/// How critical a resource is to the environment, from the operator's view.
///
/// `Protected` and `Critical` resources are never autopilot targets: the
/// memory-pressure scenario protects the critical service and adjusts a
/// non-critical consumer instead (AC-013). `Standard` and `BestEffort`
/// resources may be adjusted within the governance bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Criticality {
    /// Never adjusted, by any autopilot action, under any pressure.
    Protected,
    /// Never adjusted; remediation targets a consumer instead.
    Critical,
    /// Adjustable within budget, cooldown, and policy.
    Standard,
    /// Adjustable; the preferred autopilot target.
    BestEffort,
}

/// The adjustment the autopilot proposes to apply to a subject.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResourceAdjustment {
    /// The resource to adjust: a cgroup path, container id, or unit name —
    /// the same subject the executor will act on.
    pub subject: String,
    /// The capability the adjustment would execute.
    pub capability: String,
    /// A short deterministic justification (evidence reference), recorded with
    /// the decision; prose, never authorization.
    pub reason: String,
}

/// Why the governor refused an adjustment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    /// The subject was never classified; deny-by-default applies.
    Unclassified,
    /// The subject is protected or critical and is never an autopilot target.
    Protected,
    /// The environment's action budget for the window is exhausted.
    BudgetExhausted,
    /// The subject was adjusted too recently (rate limiting).
    Cooldown,
}

/// The governor's decision for one adjustment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GovernanceDecision {
    /// The adjustment may proceed through the ordinary safety boundary.
    Allowed { reason: String },
    /// The adjustment is refused; nothing executes and the refusal is recorded.
    Refused { why: Refusal },
}

impl GovernanceDecision {
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed { .. })
    }
}

/// The governance bounds an operator configures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutopilotLimits {
    /// Governance actions permitted per rolling window, across all subjects.
    pub budget_per_window: u32,
    /// The rolling window the budget applies to.
    pub window: Duration,
    /// Minimum spacing between two adjustments of the same subject.
    pub cooldown_per_subject: Duration,
}

impl Default for AutopilotLimits {
    fn default() -> Self {
        Self {
            budget_per_window: 10,
            window: Duration::hours(1),
            cooldown_per_subject: Duration::minutes(10),
        }
    }
}

/// The integration seam the remediation loop consumes, so tests and callers
/// can substitute a permissive or recording governor.
pub trait ResourceGovernor: Send + Sync {
    fn evaluate(&self, adjustment: &ResourceAdjustment) -> GovernanceDecision;

    /// The resource families the governor gates (e.g. `host.cgroup.`): a
    /// capability whose id starts with one of these prefixes is governed.
    fn governed_prefixes(&self) -> &[&str];
}

/// The resource families the autopilot governs. The cgroup v2 freezer is the
/// resource-control adjustment of this milestone; later milestones extend this
/// list (process priority, container resources) as their capabilities land.
pub const GOVERNED_PREFIXES: &[&str] = &["host.cgroup."];

/// The deterministic autopilot governor.
///
/// State (the window's actions and per-subject history) lives behind a mutex;
/// the decision itself is a pure function of the classification, the limits,
/// and that state, so the same adjustment at the same instant always yields
/// the same answer.
pub struct AutopilotGovernor {
    criticality: Mutex<HashMap<String, Criticality>>,
    limits: AutopilotLimits,
    /// Adjustments granted, newest last — the budget window's history.
    granted: Mutex<VecDeque<(DateTime<Utc>, String)>>,
}

impl AutopilotGovernor {
    pub fn new(limits: AutopilotLimits) -> Self {
        Self {
            criticality: Mutex::new(HashMap::new()),
            limits,
            granted: Mutex::new(VecDeque::new()),
        }
    }

    /// Classifies a subject. The last classification wins, so an operator can
    /// raise or lower a resource's tier at runtime.
    pub fn classify(&self, subject: impl Into<String>, criticality: Criticality) {
        self.criticality
            .lock()
            .expect("autopilot state is not poisoned")
            .insert(subject.into(), criticality);
    }

    /// The pure decision for `adjustment` at instant `now`, without recording
    /// it. Tests drive this directly with fixed clocks.
    pub fn decide(
        &self,
        adjustment: &ResourceAdjustment,
        now: DateTime<Utc>,
    ) -> GovernanceDecision {
        let classified = self
            .criticality
            .lock()
            .expect("autopilot state is not poisoned")
            .get(&adjustment.subject)
            .copied();

        let Some(criticality) = classified else {
            return GovernanceDecision::Refused {
                why: Refusal::Unclassified,
            };
        };
        // Protected and critical resources are never autopilot targets: the
        // pressure scenario adjusts a non-critical consumer instead (FR-017).
        if matches!(criticality, Criticality::Protected | Criticality::Critical) {
            return GovernanceDecision::Refused {
                why: Refusal::Protected,
            };
        }

        let mut granted = self
            .granted
            .lock()
            .expect("autopilot state is not poisoned");

        // Rate limiting (1/2): the per-subject cooldown.
        if let Some((_, _)) = granted.iter().rev().find(|(at, subject)| {
            *subject == adjustment.subject && now - *at < self.limits.cooldown_per_subject
        }) {
            return GovernanceDecision::Refused {
                why: Refusal::Cooldown,
            };
        }

        // Rate limiting (2/2): the rolling budget window. An entry exactly one
        // window old has left the window.
        while granted
            .front()
            .is_some_and(|(at, _)| now - *at >= self.limits.window)
        {
            granted.pop_front();
        }
        if granted.len() >= self.limits.budget_per_window as usize {
            return GovernanceDecision::Refused {
                why: Refusal::BudgetExhausted,
            };
        }

        GovernanceDecision::Allowed {
            reason: format!(
                "subject '{}' is {criticality:?} and within budget and cooldown",
                adjustment.subject
            ),
        }
    }

    /// Records an allowed decision, so budgets and cooldowns bind it.
    pub fn record(&self, adjustment: &ResourceAdjustment, now: DateTime<Utc>) {
        self.granted
            .lock()
            .expect("autopilot state is not poisoned")
            .push_back((now, adjustment.subject.clone()));
    }

    /// A subject's classification, if any.
    pub fn classification(&self, subject: &str) -> Option<Criticality> {
        self.criticality
            .lock()
            .expect("autopilot state is not poisoned")
            .get(subject)
            .copied()
    }
}

impl Default for AutopilotGovernor {
    fn default() -> Self {
        Self::new(AutopilotLimits::default())
    }
}

impl ResourceGovernor for AutopilotGovernor {
    fn evaluate(&self, adjustment: &ResourceAdjustment) -> GovernanceDecision {
        let decision = self.decide(adjustment, Utc::now());
        if decision.is_allowed() {
            self.record(adjustment, Utc::now());
        }
        decision
    }

    fn governed_prefixes(&self) -> &[&str] {
        GOVERNED_PREFIXES
    }
}

/// A governor that refuses everything — the fail-closed stand-in for callers
/// that have not configured autopilot governance.
#[derive(Debug, Default)]
pub struct NoGovernor;

impl ResourceGovernor for NoGovernor {
    fn evaluate(&self, _adjustment: &ResourceAdjustment) -> GovernanceDecision {
        GovernanceDecision::Refused {
            why: Refusal::Unclassified,
        }
    }

    fn governed_prefixes(&self) -> &[&str] {
        GOVERNED_PREFIXES
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn freeze(subject: &str) -> ResourceAdjustment {
        ResourceAdjustment {
            subject: subject.to_string(),
            capability: "host.cgroup.freeze".to_string(),
            reason: "memory pressure evidence".to_string(),
        }
    }

    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    #[test]
    fn unclassified_subjects_are_denied_by_default() {
        let governor = AutopilotGovernor::default();
        let decision = governor.decide(&freeze("workload.batch/app"), now());
        assert_eq!(
            decision,
            GovernanceDecision::Refused {
                why: Refusal::Unclassified
            }
        );
    }

    #[test]
    fn protected_and_critical_resources_are_never_autopilot_targets() {
        let governor = AutopilotGovernor::default();
        governor.classify("system.slice/postgres", Criticality::Critical);
        governor.classify("system.slice/argusd", Criticality::Protected);

        for subject in ["system.slice/postgres", "system.slice/argusd"] {
            assert_eq!(
                governor.decide(&freeze(subject), now()),
                GovernanceDecision::Refused {
                    why: Refusal::Protected
                },
                "{subject} must be protected"
            );
        }
    }

    #[test]
    fn the_memory_pressure_scenario_protects_the_service_and_adjusts_the_consumer() {
        // AC-013: a critical service is protected, a non-critical consumer
        // receives the policy-approved adjustment.
        let governor = AutopilotGovernor::default();
        governor.classify("system.slice/postgres", Criticality::Critical);
        governor.classify("workload.batch/exporter", Criticality::BestEffort);

        assert_eq!(
            governor.decide(&freeze("system.slice/postgres"), now()),
            GovernanceDecision::Refused {
                why: Refusal::Protected
            }
        );
        let decision = governor.decide(&freeze("workload.batch/exporter"), now());
        assert!(decision.is_allowed(), "{decision:?}");
    }

    #[test]
    fn a_subject_cannot_be_adjusted_twice_within_its_cooldown() {
        let governor = AutopilotGovernor::default();
        governor.classify("workload.batch/app", Criticality::BestEffort);
        let t0 = now();

        assert!(
            governor
                .decide(&freeze("workload.batch/app"), t0)
                .is_allowed()
        );
        governor.record(&freeze("workload.batch/app"), t0);

        let t1 = t0 + governor.limits.cooldown_per_subject - Duration::seconds(1);
        assert_eq!(
            governor.decide(&freeze("workload.batch/app"), t1),
            GovernanceDecision::Refused {
                why: Refusal::Cooldown
            }
        );

        let t2 = t0 + governor.limits.cooldown_per_subject;
        assert!(
            governor
                .decide(&freeze("workload.batch/app"), t2)
                .is_allowed(),
            "after the cooldown the subject is adjustable again"
        );
    }

    #[test]
    fn the_budget_bounds_governance_actions_per_window() {
        let governor = AutopilotGovernor::new(AutopilotLimits {
            budget_per_window: 2,
            window: Duration::hours(1),
            cooldown_per_subject: Duration::seconds(0),
        });
        governor.classify("a", Criticality::BestEffort);
        governor.classify("b", Criticality::BestEffort);
        governor.classify("c", Criticality::BestEffort);
        let t0 = now();

        assert!(governor.decide(&freeze("a"), t0).is_allowed());
        governor.record(&freeze("a"), t0);
        assert!(governor.decide(&freeze("b"), t0).is_allowed());
        governor.record(&freeze("b"), t0);
        assert_eq!(
            governor.decide(&freeze("c"), t0),
            GovernanceDecision::Refused {
                why: Refusal::BudgetExhausted
            }
        );

        // Outside the window the budget resets.
        let later = t0 + Duration::hours(1);
        assert!(governor.decide(&freeze("c"), later).is_allowed());
    }

    #[test]
    fn the_trait_object_evaluates_and_records_atomically() {
        let governor = AutopilotGovernor::default();
        governor.classify("workload.batch/app", Criticality::Standard);
        let governor: &dyn ResourceGovernor = &governor;

        let adjustment = freeze("workload.batch/app");
        assert!(governor.evaluate(&adjustment).is_allowed());
        // The recorded first grant binds the cooldown for the second call.
        assert_eq!(
            governor.evaluate(&adjustment),
            GovernanceDecision::Refused {
                why: Refusal::Cooldown
            }
        );
        assert!(governor.governed_prefixes().contains(&"host.cgroup."));
    }

    #[test]
    fn no_governor_refuses_everything() {
        let governor = NoGovernor;
        assert!(!governor.evaluate(&freeze("anything")).is_allowed());
    }

    #[test]
    fn criticality_serializes_stably() {
        let json = serde_json::to_string(&Criticality::BestEffort).unwrap();
        assert_eq!(json, "\"best_effort\"");
        let back: Criticality = serde_json::from_str(&json).unwrap();
        assert_eq!(back, Criticality::BestEffort);
    }
}
