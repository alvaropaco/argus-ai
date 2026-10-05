//! The escalation decision (spec 003 M6, CAP-22, FR-023, ADR-0035 §3).
//!
//! For one detected situation, the runtime must choose between OBSERVE /
//! EXPLAIN / RECOMMEND / ASK-HUMAN / AUTO-FIX. The decision is a deterministic
//! function over typed inputs — evidence quality, confidence, reversibility,
//! blast radius, criticality, environment, historical success, autonomy level,
//! and the policy outcome — and **policy is final**: a deny or
//! require-approval always wins over any escalation.

use argus_domain::{AutonomyMode, BlastRadius, PolicyOutcome, RiskClass};

use crate::autopilot::Criticality;

/// How much evidence backs the situation the escalation is deciding about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceQuality {
    /// One weak signal, nothing corroborating — not a basis for asking a
    /// human to act.
    Sparse,
    /// Corroborated by more than one signal or validation.
    Adequate,
    /// Corroborated and validated against live state.
    Strong,
}

/// The environment the runtime operates in. Production demands the highest
/// confidence before any automatic fix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Environment {
    Production,
    Staging,
    Development,
}

/// The escalation decision for one situation (CAP-22).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Escalation {
    /// Watch it; do nothing else.
    Observe,
    /// Explain what is happening, in the TUI/report; no proposed action.
    Explain,
    /// Propose a plan for the operator to look at (never auto-executed).
    Recommend,
    /// Pause for a human decision.
    AskHuman,
    /// Execute the candidate plan automatically — still through policy, the
    /// typed executor, and validation.
    AutoFix,
}

/// Everything the escalation decision weighs, typed.
#[derive(Debug, Clone, Copy)]
pub struct EscalationInput {
    pub evidence_quality: EvidenceQuality,
    /// Investigation/hypothesis confidence in `[0,1]`.
    pub confidence: f32,
    /// Whether the candidate action is reversible.
    pub reversible: bool,
    /// The candidate action's blast radius.
    pub blast_radius: BlastRadius,
    /// The candidate action's risk class.
    pub risk: RiskClass,
    /// The subject's criticality (policy's view; Protected/Critical are never
    /// autopilot targets).
    pub criticality: Criticality,
    pub environment: Environment,
    /// Historical success rate of the matching runbook/procedure, when known.
    pub historical_success: Option<f32>,
    pub autonomy: AutonomyMode,
    /// The policy outcome for the candidate action. Final: Deny and
    /// RequireApproval always win.
    pub policy: PolicyOutcome,
}

/// Minimum confidence for an AUTO-FIX, by environment.
fn confidence_threshold(environment: Environment) -> f32 {
    match environment {
        Environment::Production => 0.75,
        Environment::Staging => 0.6,
        Environment::Development => 0.5,
    }
}

/// Decide the escalation for one situation.
///
/// Order of precedence (first match wins):
/// 1. Policy `Deny` → `Observe` (the situation may be watched and explained,
///    but nothing may act — policy is final).
/// 2. Policy `RequireApproval` → `AskHuman`.
/// 3. Autonomy cap: L0 → `Observe`, L1 → `Explain`, L2 → `Recommend`.
/// 4. L3/L4/L5: `AutoFix` when the candidate is safe to fix automatically
///    (reversible, blast radius at most Host, not a Protected/Critical
///    subject, not high-risk or destructive, confidence above the
///    environment's threshold, and historical success — when known — at
///    least 0.7). With sparse evidence there is no basis for asking a human
///    to act, so the decision degrades to `Recommend`; otherwise `AskHuman`.
pub fn decide_escalation(input: &EscalationInput) -> Escalation {
    match input.policy {
        PolicyOutcome::Deny => return Escalation::Observe,
        PolicyOutcome::RequireApproval => return Escalation::AskHuman,
        PolicyOutcome::Allow => {}
    }

    match input.autonomy {
        AutonomyMode::L0Observe => return Escalation::Observe,
        AutonomyMode::L1Explain => return Escalation::Explain,
        AutonomyMode::L2Recommend => return Escalation::Recommend,
        AutonomyMode::L3Assisted | AutonomyMode::L4Autonomous | AutonomyMode::L5Adaptive => {}
    }

    if safe_to_fix(input) {
        return Escalation::AutoFix;
    }
    if input.evidence_quality == EvidenceQuality::Sparse {
        return Escalation::Recommend;
    }
    Escalation::AskHuman
}

/// Whether the candidate action may run automatically at L3+.
fn safe_to_fix(input: &EscalationInput) -> bool {
    let bounded = matches!(input.blast_radius, BlastRadius::None | BlastRadius::Host);
    let non_critical = !matches!(
        input.criticality,
        Criticality::Protected | Criticality::Critical
    );
    let bounded_risk = matches!(
        input.risk,
        RiskClass::Read | RiskClass::LowRisk | RiskClass::Controlled
    );
    let confident = input.confidence >= confidence_threshold(input.environment);
    let historically_sound = input.historical_success.is_none_or(|rate| rate >= 0.7);

    input.reversible
        && bounded
        && non_critical
        && bounded_risk
        && confident
        && historically_sound
        && input.evidence_quality != EvidenceQuality::Sparse
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> EscalationInput {
        EscalationInput {
            evidence_quality: EvidenceQuality::Strong,
            confidence: 0.9,
            reversible: true,
            blast_radius: BlastRadius::Host,
            risk: RiskClass::LowRisk,
            criticality: Criticality::Standard,
            environment: Environment::Production,
            historical_success: Some(0.9),
            autonomy: AutonomyMode::L4Autonomous,
            policy: PolicyOutcome::Allow,
        }
    }

    #[test]
    fn a_strong_production_case_auto_fixes_at_l4() {
        assert_eq!(decide_escalation(&input()), Escalation::AutoFix);
    }

    #[test]
    fn policy_is_final_over_any_escalation() {
        let denied = EscalationInput {
            policy: PolicyOutcome::Deny,
            ..input()
        };
        assert_eq!(decide_escalation(&denied), Escalation::Observe);

        let approval = EscalationInput {
            policy: PolicyOutcome::RequireApproval,
            ..input()
        };
        assert_eq!(decide_escalation(&approval), Escalation::AskHuman);

        // Even at L5 with perfect evidence, a deny is never bypassed.
        let l5_denied = EscalationInput {
            autonomy: AutonomyMode::L5Adaptive,
            confidence: 1.0,
            policy: PolicyOutcome::Deny,
            ..input()
        };
        assert_eq!(decide_escalation(&l5_denied), Escalation::Observe);
    }

    #[test]
    fn the_autonomy_level_caps_the_escalation() {
        for (mode, expected) in [
            (AutonomyMode::L0Observe, Escalation::Observe),
            (AutonomyMode::L1Explain, Escalation::Explain),
            (AutonomyMode::L2Recommend, Escalation::Recommend),
        ] {
            assert_eq!(
                decide_escalation(&EscalationInput {
                    autonomy: mode,
                    ..input()
                }),
                expected
            );
        }
    }

    #[test]
    fn conservative_default_l0_always_observes() {
        let l0 = EscalationInput {
            autonomy: AutonomyMode::L0Observe,
            ..input()
        };
        assert_eq!(decide_escalation(&l0), Escalation::Observe);
    }

    #[test]
    fn irreversibility_and_blast_radius_block_auto_fix() {
        let irreversible = EscalationInput {
            reversible: false,
            ..input()
        };
        assert_eq!(decide_escalation(&irreversible), Escalation::AskHuman);

        let wide = EscalationInput {
            blast_radius: BlastRadius::Environment,
            ..input()
        };
        assert_eq!(decide_escalation(&wide), Escalation::AskHuman);

        let fleet = EscalationInput {
            blast_radius: BlastRadius::Fleet,
            ..input()
        };
        assert_eq!(decide_escalation(&fleet), Escalation::AskHuman);
    }

    #[test]
    fn protected_and_critical_subjects_are_never_auto_fixed() {
        for criticality in [Criticality::Protected, Criticality::Critical] {
            let subject = EscalationInput {
                criticality,
                ..input()
            };
            assert_eq!(decide_escalation(&subject), Escalation::AskHuman);
        }
    }

    #[test]
    fn high_risk_and_destructive_are_never_auto_fixed() {
        for risk in [RiskClass::HighRisk, RiskClass::Destructive] {
            let action = EscalationInput { risk, ..input() };
            assert_eq!(decide_escalation(&action), Escalation::AskHuman);
        }
    }

    #[test]
    fn confidence_thresholds_differ_by_environment() {
        let staging_ok = EscalationInput {
            environment: Environment::Staging,
            confidence: 0.6,
            ..input()
        };
        assert_eq!(decide_escalation(&staging_ok), Escalation::AutoFix);

        let production_insufficient = EscalationInput {
            environment: Environment::Production,
            confidence: 0.6,
            ..input()
        };
        assert_eq!(
            decide_escalation(&production_insufficient),
            Escalation::AskHuman
        );
    }

    #[test]
    fn sparse_evidence_degrades_to_recommend_not_ask_human() {
        let sparse = EscalationInput {
            evidence_quality: EvidenceQuality::Sparse,
            ..input()
        };
        assert_eq!(decide_escalation(&sparse), Escalation::Recommend);
    }

    #[test]
    fn unknown_poor_historical_success_blocks_auto_fix() {
        let poor = EscalationInput {
            historical_success: Some(0.4),
            ..input()
        };
        assert_eq!(decide_escalation(&poor), Escalation::AskHuman);

        // Unknown history does not block (no invented failure rate).
        let unknown = EscalationInput {
            historical_success: None,
            ..input()
        };
        assert_eq!(decide_escalation(&unknown), Escalation::AutoFix);
    }
}
