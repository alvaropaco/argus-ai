//! Declarative runbook shape (data-model §6, CAP-17).

use argus_domain::CapabilityId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::criterion::Criterion;

/// The promotion status of a runbook (ADR-0036 §3). Candidates are recorded
/// but never executed end-to-end; only promoted runbooks drive procedures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunbookStatus {
    Candidate,
    Approved,
    Promoted,
}

/// What causes a runbook to apply. Typed — never a free-text glob.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunbookTrigger {
    /// A normalized symptom signature (e.g. "restart-loop", "oom-killed").
    Symptom(String),
    /// A named monitored signal (e.g. "disk.used_percent").
    Signal(String),
    /// Operator-invoked, always available.
    Manual,
}

/// The kinds of evidence an investigation step gathers or a runbook requires
/// before its decision criteria may be evaluated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    Metrics,
    Process,
    Service,
    Container,
    Cgroup,
    Kubernetes,
    Logs,
    ChangeRecord,
}

/// One investigation step: a description plus the kind of evidence it
/// gathers. Steps are declarative data; execution happens through the
/// ordinary observation/investigation paths, never through the runbook.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub description: String,
    pub evidence: EvidenceKind,
}

/// A declarative operational procedure (data-model §6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Runbook {
    pub id: Uuid,
    /// Stable name (e.g. "diagnose_disk_pressure").
    pub name: String,
    pub trigger: RunbookTrigger,
    /// Evidence that must exist before the criteria are evaluated at all.
    pub required_evidence: Vec<EvidenceKind>,
    pub investigation_steps: Vec<Step>,
    pub decision_criteria: Vec<Criterion>,
    /// Candidate typed capabilities, not grants (ADR-0036 §2).
    pub allowed_actions: Vec<CapabilityId>,
    /// The declared rollback capabilities, in the order they would run.
    pub rollback: Vec<CapabilityId>,
    pub validation: Vec<Criterion>,
    status: RunbookStatus,
    /// Crate-private: only the promotion ladder (ADR-0036 §3) appends.
    pub(crate) gates: Vec<Gate>,
    attempts: u32,
    successes: u32,
}

use crate::promotion::Gate;

impl Runbook {
    /// Create a Candidate runbook. Every runbook — hand-written or learned —
    /// starts here; promotion is earned, never declared.
    ///
    /// The parameter list mirrors the data-model §6 shape one-to-one, which
    /// is worth exceeding the argument lint for (a nested builder would hide
    /// which fields are required).
    #[allow(clippy::too_many_arguments)]
    pub fn candidate(
        id: Uuid,
        name: impl Into<String>,
        trigger: RunbookTrigger,
        required_evidence: Vec<EvidenceKind>,
        investigation_steps: Vec<Step>,
        decision_criteria: Vec<Criterion>,
        allowed_actions: Vec<CapabilityId>,
        rollback: Vec<CapabilityId>,
        validation: Vec<Criterion>,
    ) -> Self {
        Self {
            id,
            name: name.into(),
            trigger,
            required_evidence,
            investigation_steps,
            decision_criteria,
            allowed_actions,
            rollback,
            validation,
            status: RunbookStatus::Candidate,
            gates: Vec::new(),
            attempts: 0,
            successes: 0,
        }
    }

    pub fn status(&self) -> RunbookStatus {
        self.status
    }

    /// Set the status; crate-private so only the promotion ladder can move it.
    pub(crate) fn set_status(&mut self, status: RunbookStatus) {
        self.status = status;
    }

    /// The gates passed so far, in pass order.
    pub fn gates(&self) -> &[Gate] {
        &self.gates
    }

    /// Whether this runbook applies to `trigger`.
    pub fn matches_trigger(&self, trigger: &RunbookTrigger) -> bool {
        &self.trigger == trigger
    }

    /// Whether `capability` is one of this runbook's candidate actions. A
    /// runbook cannot widen the capability set: this is a filter against the
    /// registry, never a permission.
    pub fn allows_candidate(&self, capability: &CapabilityId) -> bool {
        self.allowed_actions.iter().any(|c| c == capability)
    }

    /// The recorded success rate, or `None` before the first attempt.
    pub fn historical_success_rate(&self) -> Option<f32> {
        if self.attempts == 0 {
            None
        } else {
            Some(self.successes as f32 / self.attempts as f32)
        }
    }

    /// Record one full run of the procedure (attempts and outcomes feed the
    /// historical success rate used by the escalation decision).
    pub fn record_outcome(&mut self, success: bool) {
        self.attempts += 1;
        if success {
            self.successes += 1;
        }
    }

    pub fn attempts(&self) -> u32 {
        self.attempts
    }

    pub fn successes(&self) -> u32 {
        self.successes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::criterion::Comparison;
    use crate::promotion::{GateError, PromotionError};

    fn restart_cap() -> CapabilityId {
        CapabilityId::new("host.service.restart").unwrap()
    }

    fn runbook() -> Runbook {
        Runbook::candidate(
            Uuid::new_v4(),
            "restart-failing-service",
            RunbookTrigger::Symptom("restart-loop".into()),
            vec![EvidenceKind::Service, EvidenceKind::Logs],
            vec![Step {
                description: "read the unit's recent logs".into(),
                evidence: EvidenceKind::Logs,
            }],
            vec![Criterion {
                description: "unit is failing".into(),
                attribute: "unit.active_state".into(),
                comparison: Comparison::Equal("failed".into()),
            }],
            vec![restart_cap()],
            vec![restart_cap()],
            vec![Criterion {
                description: "unit is active again".into(),
                attribute: "unit.active_state".into(),
                comparison: Comparison::Equal("active".into()),
            }],
        )
    }

    #[test]
    fn new_runbooks_start_as_candidates_with_no_gates() {
        let rb = runbook();
        assert_eq!(rb.status(), RunbookStatus::Candidate);
        assert!(rb.gates().is_empty());
        assert_eq!(rb.historical_success_rate(), None);
    }

    #[test]
    fn candidates_filter_capabilities_but_grant_nothing() {
        let rb = runbook();
        assert!(rb.allows_candidate(&restart_cap()));
        assert!(!rb.allows_candidate(&CapabilityId::new("host.cgroup.freeze").unwrap()));
    }

    #[test]
    fn trigger_matching_is_exact() {
        let rb = runbook();
        assert!(rb.matches_trigger(&RunbookTrigger::Symptom("restart-loop".into())));
        assert!(!rb.matches_trigger(&RunbookTrigger::Symptom("oom-killed".into())));
        assert!(!rb.matches_trigger(&RunbookTrigger::Manual));
    }

    #[test]
    fn outcomes_feed_the_success_rate() {
        let mut rb = runbook();
        rb.record_outcome(true);
        rb.record_outcome(true);
        rb.record_outcome(false);
        assert_eq!(rb.attempts(), 3);
        assert_eq!(rb.successes(), 2);
        let rate = rb.historical_success_rate().unwrap();
        assert!((rate - 2.0 / 3.0).abs() < 1e-6);
    }

    #[test]
    fn serde_round_trip_keeps_private_fields() {
        let rb = runbook();
        let json = serde_json::to_string(&rb).unwrap();
        let back: Runbook = serde_json::from_str(&json).unwrap();
        assert_eq!(back, rb);
        let _ = (GateError::OutOfOrder, PromotionError::NotApproved);
    }
}
