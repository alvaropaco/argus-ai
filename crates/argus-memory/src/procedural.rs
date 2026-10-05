//! Procedural memory: learned-procedure records with success history
//! (ADR-0038 §1). The runbooks themselves live in `argus-runbooks`; this
//! layer remembers what has been tried and how it worked out, so runbook
//! promotion and escalation can cite real history.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The promotion status of a learned procedure (mirrors the runbook ladder,
/// ADR-0036 §3: a learned procedure is a candidate until it passes the gate
/// sequence).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcedureStatus {
    Candidate,
    Approved,
    Promoted,
}

/// One remembered procedure: its trigger, its recorded outcomes, and its
/// current promotion status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcedureRecord {
    pub id: Uuid,
    /// The trigger signature this procedure answers (e.g. "restart-loop").
    pub trigger: String,
    /// A stable human name (e.g. "restart-and-validate").
    pub name: String,
    pub status: ProcedureStatus,
    attempts: u32,
    successes: u32,
    first_seen: DateTime<Utc>,
    last_seen: DateTime<Utc>,
}

impl ProcedureRecord {
    /// The recorded success rate, or `None` before the first attempt — an
    /// unknown rate is never invented as 0 or 1.
    pub fn success_rate(&self) -> Option<f32> {
        if self.attempts == 0 {
            None
        } else {
            Some(self.successes as f32 / self.attempts as f32)
        }
    }

    pub fn attempts(&self) -> u32 {
        self.attempts
    }

    pub fn successes(&self) -> u32 {
        self.successes
    }

    pub fn first_seen(&self) -> DateTime<Utc> {
        self.first_seen
    }

    pub fn last_seen(&self) -> DateTime<Utc> {
        self.last_seen
    }
}

/// The procedural layer: keyed procedure records with outcome history.
#[derive(Debug, Default)]
pub struct ProceduralMemory {
    procedures: HashMap<Uuid, ProcedureRecord>,
}

impl ProceduralMemory {
    /// Register a procedure (idempotent on id).
    pub fn register(&mut self, record: ProcedureRecord) {
        self.procedures.insert(record.id, record);
    }

    pub fn get(&self, id: Uuid) -> Option<&ProcedureRecord> {
        self.procedures.get(&id)
    }

    /// Promote a procedure's status through the ladder. An unknown id or a
    /// backwards transition is rejected (Candidates never skip to Promoted —
    /// the runbook crate owns the gate sequence; this layer only records).
    pub fn set_status(&mut self, id: Uuid, status: ProcedureStatus) -> Option<()> {
        let record = self.procedures.get_mut(&id)?;
        if (status as u8) < (record.status as u8) {
            return None;
        }
        record.status = status;
        Some(())
    }

    /// Record one attempt of a procedure.
    pub fn record_outcome(&mut self, id: Uuid, success: bool, at: DateTime<Utc>) -> Option<()> {
        let record = self.procedures.get_mut(&id)?;
        record.attempts += 1;
        if success {
            record.successes += 1;
        }
        record.last_seen = at;
        Some(())
    }

    /// Procedures registered for a trigger, ordered by name. Candidates come
    /// after promoted ones only implicitly via name order — retrieval is
    /// keyed and deterministic; ranking is the caller's policy.
    pub fn procedures_for(&self, trigger: &str) -> Vec<&ProcedureRecord> {
        let mut out: Vec<&ProcedureRecord> = self
            .procedures
            .values()
            .filter(|p| p.trigger == trigger)
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    pub fn len(&self) -> usize {
        self.procedures.len()
    }

    pub fn is_empty(&self) -> bool {
        self.procedures.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn ts(minute: u32) -> DateTime<Utc> {
        chrono::Utc
            .with_ymd_and_hms(2026, 10, 5, 12, minute, 0)
            .unwrap()
    }

    fn record(id: Uuid, name: &str) -> ProcedureRecord {
        ProcedureRecord {
            id,
            trigger: "restart-loop".to_string(),
            name: name.to_string(),
            status: ProcedureStatus::Candidate,
            attempts: 0,
            successes: 0,
            first_seen: ts(0),
            last_seen: ts(0),
        }
    }

    #[test]
    fn unknown_success_rate_is_never_invented() {
        let mut m = ProceduralMemory::default();
        let id = Uuid::new_v4();
        m.register(record(id, "restart-and-validate"));
        assert_eq!(m.get(id).unwrap().success_rate(), None);
        m.record_outcome(id, true, ts(1)).unwrap();
        m.record_outcome(id, false, ts(2)).unwrap();
        let rate = m.get(id).unwrap().success_rate().unwrap();
        assert!((rate - 0.5).abs() < 1e-6);
    }

    #[test]
    fn status_never_moves_backwards() {
        let mut m = ProceduralMemory::default();
        let id = Uuid::new_v4();
        m.register(record(id, "p"));
        assert!(m.set_status(id, ProcedureStatus::Approved).is_some());
        assert!(m.set_status(id, ProcedureStatus::Candidate).is_none());
        assert_eq!(m.get(id).unwrap().status, ProcedureStatus::Approved);
    }

    #[test]
    fn retrieval_by_trigger_is_ordered_by_name() {
        let mut m = ProceduralMemory::default();
        m.register(record(Uuid::new_v4(), "zebra"));
        m.register(record(Uuid::new_v4(), "alpha"));
        let names: Vec<&str> = m
            .procedures_for("restart-loop")
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(names, ["alpha", "zebra"]);
        assert!(m.procedures_for("disk-pressure").is_empty());
    }
}
