//! Cloud-delivered runbooks and their provenance (spec 009 FR-001/FR-002).
//!
//! Delivery is data (ADR-0041): a delivered runbook authorizes nothing and
//! its `allowed_actions` stay candidates-not-grants. What a delivery *does*
//! carry is its provenance — the managed configuration that delivered it —
//! because every delivered candidate must be able to answer "where did I
//! come from" on the operator's runbook card. Ladder state lives inside the
//! [`Runbook`] itself and is earned locally, never delivered.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::runbook::Runbook;

/// A delivered candidate plus the configuration version that delivered it.
/// Persisted keyed by the runbook's name (spec 009 FR-002): a re-delivery of
/// the same name supersedes the previous row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeliveredRunbook {
    /// The parsed candidate — status and earned gates included, so gate
    /// progress survives a restart (FR-002). Serialized with the same serde
    /// shape the runbook crate tests round-trip.
    pub runbook: Runbook,
    /// The managed configuration that delivered this runbook (provenance).
    pub configuration_id: Uuid,
    /// The exact version id of that configuration.
    pub version_id: Uuid,
    /// The version number, for human-readable provenance.
    pub version_number: i64,
    /// When the daemon accepted the delivery.
    pub delivered_at: DateTime<Utc>,
}

impl DeliveredRunbook {
    pub fn name(&self) -> &str {
        &self.runbook.name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runbook::{EvidenceKind, RunbookTrigger};

    fn runbook(name: &str) -> Runbook {
        Runbook::candidate(
            Uuid::new_v4(),
            name,
            RunbookTrigger::Symptom("restart-loop".into()),
            vec![EvidenceKind::Logs],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
        )
    }

    #[test]
    fn a_delivery_round_trips_with_its_provenance_and_gates() {
        let mut rb = runbook("learned-procedure");
        rb.record_gate(crate::promotion::Gate::Evaluation).unwrap();
        let delivered = DeliveredRunbook {
            runbook: rb,
            configuration_id: Uuid::new_v4(),
            version_id: Uuid::new_v4(),
            version_number: 7,
            delivered_at: Utc::now(),
        };
        let json = serde_json::to_string(&delivered).unwrap();
        let back: DeliveredRunbook = serde_json::from_str(&json).unwrap();
        assert_eq!(back, delivered);
        assert_eq!(back.name(), "learned-procedure");
        assert_eq!(back.runbook.gates(), [crate::promotion::Gate::Evaluation]);
    }
}
