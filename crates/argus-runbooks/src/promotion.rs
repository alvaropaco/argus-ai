//! The promotion gate ladder (ADR-0036 §3): evaluation → simulation →
//! validation → policy → approval → promotion, one gate at a time, in that
//! order. A learned runbook is a candidate until it has earned every gate;
//! nothing here self-promotes.

use serde::{Deserialize, Serialize};

use crate::runbook::{Runbook, RunbookStatus};

/// The six gates, in the order they must be passed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Gate {
    /// The procedure's steps were evaluated against recorded episodes and
    /// produced the recorded outcome (ADR-0031's learning pass output).
    Evaluation,
    /// The candidate actions' impact was simulated (CAP-19) and accepted.
    Simulation,
    /// The procedure was executed in a safe context and validated.
    Validation,
    /// Policy review: the candidate capabilities are policy-compatible.
    Policy,
    /// An operator explicitly approved the procedure.
    Approval,
    /// The operator promoted it to driving procedures.
    Promotion,
}

/// The ordered ladder; gate *i* requires gates *0..i* to have been passed.
const LADDER: [Gate; 6] = [
    Gate::Evaluation,
    Gate::Simulation,
    Gate::Validation,
    Gate::Policy,
    Gate::Approval,
    Gate::Promotion,
];

/// Errors from the promotion ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GateError {
    /// A gate was recorded out of order or twice.
    #[error(
        "gate must be passed in order: evaluation, simulation, validation, policy, approval, promotion"
    )]
    OutOfOrder,
    /// Approval/promotion was requested before its prerequisites existed.
    #[error("prerequisite gates have not been passed yet")]
    PrerequisitesMissing,
}

/// Errors from the status transitions built on the ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PromotionError {
    #[error("approval requires the evaluation, simulation, validation, and policy gates")]
    NotReadyForApproval,
    #[error("promotion requires operator approval first")]
    NotApproved,
    #[error("the runbook is already {0:?}")]
    AlreadyInStatus(crate::runbook::RunbookStatus),
    #[error(transparent)]
    Gate(#[from] GateError),
}

impl Runbook {
    /// Record that `gate` was passed, enforcing ladder order.
    pub fn record_gate(&mut self, gate: Gate) -> Result<(), GateError> {
        let expected = LADDER
            .get(self.gates.len())
            .copied()
            .unwrap_or(Gate::Promotion);
        if gate != expected {
            return Err(GateError::OutOfOrder);
        }
        self.gates.push(gate);
        Ok(())
    }

    /// Whether every prerequisite gate before `gate` has been passed.
    pub fn has_reached(&self, gate: Gate) -> bool {
        let position = LADDER.iter().position(|g| *g == gate).unwrap_or(0);
        self.gates.len() >= position
    }

    /// Operator approval: Candidate → Approved. Requires the four technical
    /// gates (evaluation, simulation, validation, policy) to have been passed;
    /// the approval gate itself is recorded here.
    pub fn approve(&mut self) -> Result<(), PromotionError> {
        if self.status() != RunbookStatus::Candidate {
            return Err(PromotionError::AlreadyInStatus(self.status()));
        }
        if !self.has_reached(Gate::Policy) {
            return Err(PromotionError::NotReadyForApproval);
        }
        self.record_gate(Gate::Approval)?;
        self.set_status(RunbookStatus::Approved);
        Ok(())
    }

    /// Promotion: Approved → Promoted. Requires the operator approval gate.
    pub fn promote(&mut self) -> Result<(), PromotionError> {
        if self.status() == RunbookStatus::Promoted {
            return Err(PromotionError::AlreadyInStatus(self.status()));
        }
        if self.status() != RunbookStatus::Approved {
            return Err(PromotionError::NotApproved);
        }
        self.record_gate(Gate::Promotion)?;
        self.set_status(RunbookStatus::Promoted);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runbook::RunbookTrigger;
    use uuid::Uuid;

    fn runbook() -> Runbook {
        Runbook::candidate(
            Uuid::new_v4(),
            "learned-procedure",
            RunbookTrigger::Symptom("disk-pressure".into()),
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
        )
    }

    fn pass_technical_gates(rb: &mut Runbook) {
        for gate in [
            Gate::Evaluation,
            Gate::Simulation,
            Gate::Validation,
            Gate::Policy,
        ] {
            rb.record_gate(gate).unwrap();
        }
    }

    #[test]
    fn gates_must_pass_in_order() {
        let mut rb = runbook();
        assert_eq!(rb.record_gate(Gate::Simulation), Err(GateError::OutOfOrder));
        assert_eq!(rb.record_gate(Gate::Promotion), Err(GateError::OutOfOrder));
        rb.record_gate(Gate::Evaluation).unwrap();
        assert_eq!(rb.record_gate(Gate::Evaluation), Err(GateError::OutOfOrder));
        rb.record_gate(Gate::Simulation).unwrap();
        assert_eq!(rb.gates(), [Gate::Evaluation, Gate::Simulation]);
    }

    #[test]
    fn approval_requires_all_four_technical_gates() {
        let mut rb = runbook();
        assert_eq!(rb.approve(), Err(PromotionError::NotReadyForApproval));
        pass_technical_gates(&mut rb);
        rb.approve().unwrap();
        assert_eq!(rb.status(), RunbookStatus::Approved);
        assert_eq!(rb.gates().last(), Some(&Gate::Approval));
    }

    #[test]
    fn a_candidate_cannot_skip_to_promoted() {
        let mut rb = runbook();
        assert_eq!(rb.promote(), Err(PromotionError::NotApproved));
        pass_technical_gates(&mut rb);
        // Even with all technical gates, promotion without approval fails.
        assert_eq!(rb.promote(), Err(PromotionError::NotApproved));
        rb.approve().unwrap();
        rb.promote().unwrap();
        assert_eq!(rb.status(), RunbookStatus::Promoted);
        assert_eq!(
            rb.promote(),
            Err(PromotionError::AlreadyInStatus(RunbookStatus::Promoted))
        );
    }

    #[test]
    fn status_never_regresses_through_the_ladder() {
        let mut rb = runbook();
        pass_technical_gates(&mut rb);
        rb.approve().unwrap();
        assert_eq!(
            rb.approve(),
            Err(PromotionError::AlreadyInStatus(RunbookStatus::Approved))
        );
    }
}
