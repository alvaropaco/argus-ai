//! Autonomy-level gating (FR-007, ADR-0035).
//!
//! A higher level grants more autonomy, never more privilege: the same
//! policy/executor boundary applies at every level, and a policy deny or
//! require-approval always wins over the level.

use argus_domain::{AutonomyMode, RiskClass};

/// Whether an action of the given risk may execute without explicit operator
/// approval under the configured autonomy level (ADR-0035 §2).
///
/// - `L0Observe` / `L1Explain` / `L2Recommend`: never execute.
/// - `L3Assisted`: execute only `Read` or `LowRisk` actions.
/// - `L4Autonomous` / `L5Adaptive`: additionally `Controlled` — the actions
///   policy already allows; high-risk and destructive stay approval-gated.
pub fn may_execute_without_approval(mode: AutonomyMode, risk: RiskClass) -> bool {
    match mode {
        AutonomyMode::L0Observe | AutonomyMode::L1Explain | AutonomyMode::L2Recommend => false,
        AutonomyMode::L3Assisted => matches!(risk, RiskClass::Read | RiskClass::LowRisk),
        AutonomyMode::L4Autonomous | AutonomyMode::L5Adaptive => {
            matches!(
                risk,
                RiskClass::Read | RiskClass::LowRisk | RiskClass::Controlled
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observe_explain_and_recommend_never_execute() {
        for mode in [
            AutonomyMode::L0Observe,
            AutonomyMode::L1Explain,
            AutonomyMode::L2Recommend,
        ] {
            for risk in [
                RiskClass::Read,
                RiskClass::LowRisk,
                RiskClass::Controlled,
                RiskClass::HighRisk,
                RiskClass::Destructive,
            ] {
                assert!(!may_execute_without_approval(mode, risk));
            }
        }
    }

    #[test]
    fn l3_executes_low_risk_only() {
        assert!(may_execute_without_approval(
            AutonomyMode::L3Assisted,
            RiskClass::LowRisk
        ));
        assert!(!may_execute_without_approval(
            AutonomyMode::L3Assisted,
            RiskClass::Controlled
        ));
        assert!(!may_execute_without_approval(
            AutonomyMode::L3Assisted,
            RiskClass::HighRisk
        ));
    }

    #[test]
    fn l4_and_l5_extend_to_controlled_never_to_high_risk() {
        for mode in [AutonomyMode::L4Autonomous, AutonomyMode::L5Adaptive] {
            assert!(may_execute_without_approval(mode, RiskClass::Controlled));
            assert!(!may_execute_without_approval(mode, RiskClass::HighRisk));
            assert!(!may_execute_without_approval(mode, RiskClass::Destructive));
        }
    }
}
