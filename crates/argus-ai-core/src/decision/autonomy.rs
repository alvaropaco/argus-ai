//! Autonomy-mode gating (FR-007).

use argus_domain::{AutonomyMode, RiskClass};

/// Whether an action of the given risk may execute without explicit operator
/// approval under the configured autonomy mode.
///
/// - `ObserveOnly`: never execute.
/// - `Propose`: never execute without approval (default).
/// - `Assisted`: execute only `Read` or `LowRisk` actions.
pub fn may_execute_without_approval(mode: AutonomyMode, risk: RiskClass) -> bool {
    match mode {
        AutonomyMode::ObserveOnly | AutonomyMode::Propose => false,
        AutonomyMode::Assisted => matches!(risk, RiskClass::Read | RiskClass::LowRisk),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observe_only_never_executes() {
        for risk in [
            RiskClass::Read,
            RiskClass::LowRisk,
            RiskClass::Controlled,
            RiskClass::HighRisk,
            RiskClass::Destructive,
        ] {
            assert!(!may_execute_without_approval(
                AutonomyMode::ObserveOnly,
                risk
            ));
        }
    }

    #[test]
    fn propose_never_executes() {
        assert!(!may_execute_without_approval(
            AutonomyMode::Propose,
            RiskClass::LowRisk
        ));
    }

    #[test]
    fn assisted_executes_low_risk_only() {
        assert!(may_execute_without_approval(
            AutonomyMode::Assisted,
            RiskClass::LowRisk
        ));
        assert!(!may_execute_without_approval(
            AutonomyMode::Assisted,
            RiskClass::Controlled
        ));
        assert!(!may_execute_without_approval(
            AutonomyMode::Assisted,
            RiskClass::HighRisk
        ));
    }
}
