//! ARGUS incident management (CAP-10).
//!
//! An incident aggregates a detected condition through a lifecycle
//! (`open → investigating → mitigated → resolved → closed`) with deduplication:
//! repeated observations of the same underlying condition attach to the same
//! open incident rather than opening a new one.

use std::collections::HashMap;

use argus_domain::{ResourceId, Severity};
use chrono::{DateTime, Utc};
use uuid::Uuid;

/// The incident lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncidentStatus {
    Open,
    Investigating,
    Mitigated,
    Resolved,
    Closed,
}

/// A managed incident.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Incident {
    pub id: Uuid,
    /// The dedup key (e.g. `service:checkout-api` + symptom) used to collapse
    /// repeated observations of the same condition.
    pub dedup_key: String,
    pub severity: Severity,
    pub status: IncidentStatus,
    pub started_at: DateTime<Utc>,
    pub affected: Vec<ResourceId>,
    pub detection_source: String,
    pub root_cause: Option<String>,
    pub resolution: Option<String>,
    pub impact: Option<String>,
}

/// Incident-manager errors.
#[derive(Debug, thiserror::Error)]
pub enum IncidentError {
    #[error("unknown incident {0}")]
    UnknownId(Uuid),
    #[error("invalid transition {from:?} -> {to:?}")]
    InvalidTransition {
        from: IncidentStatus,
        to: IncidentStatus,
    },
}

/// Tracks open incidents and deduplicates repeated observations.
#[derive(Debug, Default)]
pub struct IncidentManager {
    incidents: HashMap<Uuid, Incident>,
}

impl IncidentManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Open an incident for a trigger, deduplicating against any open incident
    /// with the same key. Returns the incident id and whether a new incident was
    /// created.
    pub fn open(
        &mut self,
        dedup_key: &str,
        severity: Severity,
        affected: Vec<ResourceId>,
        source: &str,
        now: DateTime<Utc>,
    ) -> (Uuid, bool) {
        if let Some(existing) = self
            .incidents
            .values()
            .find(|i| i.dedup_key == dedup_key && i.status != IncidentStatus::Closed)
        {
            return (existing.id, false);
        }

        let id = Uuid::new_v4();
        self.incidents.insert(
            id,
            Incident {
                id,
                dedup_key: dedup_key.to_string(),
                severity,
                status: IncidentStatus::Open,
                started_at: now,
                affected,
                detection_source: source.to_string(),
                root_cause: None,
                resolution: None,
                impact: None,
            },
        );
        (id, true)
    }

    pub fn get(&self, id: Uuid) -> Option<&Incident> {
        self.incidents.get(&id)
    }

    pub fn get_mut(&mut self, id: Uuid) -> Option<&mut Incident> {
        self.incidents.get_mut(&id)
    }

    /// Advance an incident's lifecycle, rejecting transitions that skip or
    /// reverse the canonical order.
    pub fn transition(&mut self, id: Uuid, status: IncidentStatus) -> Result<(), IncidentError> {
        let from = self
            .incidents
            .get(&id)
            .ok_or(IncidentError::UnknownId(id))?
            .status;
        if !valid_transition(from, status) {
            return Err(IncidentError::InvalidTransition { from, to: status });
        }
        self.incidents.get_mut(&id).expect("checked above").status = status;
        Ok(())
    }

    pub fn open_incidents(&self) -> impl Iterator<Item = &Incident> {
        self.incidents
            .values()
            .filter(|i| i.status != IncidentStatus::Closed)
    }
}

fn valid_transition(from: IncidentStatus, to: IncidentStatus) -> bool {
    use IncidentStatus::*;
    matches!(
        (from, to),
        (Open, Investigating)
            | (Open, Closed)
            | (Investigating, Mitigated)
            | (Investigating, Resolved)
            | (Mitigated, Resolved)
            | (Resolved, Closed)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 30, 12, 0, 0).unwrap()
    }

    fn rid(kind: &str, id: &str) -> ResourceId {
        ResourceId::new(kind, id).unwrap()
    }

    #[test]
    fn repeated_observations_deduplicate() {
        let mut m = IncidentManager::new();
        let (a, created_a) = m.open("service:checkout-api", Severity::Warning, vec![], "sensor", now());
        let (b, created_b) = m.open("service:checkout-api", Severity::Warning, vec![], "sensor", now());
        assert!(created_a);
        assert!(!created_b);
        assert_eq!(a, b);
        assert_eq!(m.open_incidents().count(), 1);
    }

    #[test]
    fn different_keys_open_different_incidents() {
        let mut m = IncidentManager::new();
        let (a, _) = m.open("service:a", Severity::Warning, vec![], "s", now());
        let (b, _) = m.open("service:b", Severity::Warning, vec![], "s", now());
        assert_ne!(a, b);
        assert_eq!(m.open_incidents().count(), 2);
    }

    #[test]
    fn closed_incident_frees_the_dedup_key() {
        let mut m = IncidentManager::new();
        let (id, _) = m.open("service:a", Severity::Warning, vec![], "s", now());
        m.transition(id, IncidentStatus::Closed).unwrap();
        let (new_id, created) = m.open("service:a", Severity::Warning, vec![], "s", now());
        assert!(created);
        assert_ne!(id, new_id);
    }

    #[test]
    fn valid_lifecycle_advances() {
        let mut m = IncidentManager::new();
        let (id, _) = m.open("service:a", Severity::Error, vec![rid("service", "a")], "s", now());
        m.transition(id, IncidentStatus::Investigating).unwrap();
        m.transition(id, IncidentStatus::Mitigated).unwrap();
        m.transition(id, IncidentStatus::Resolved).unwrap();
        m.transition(id, IncidentStatus::Closed).unwrap();
        assert_eq!(m.get(id).unwrap().status, IncidentStatus::Closed);
    }

    #[test]
    fn invalid_transition_is_rejected() {
        let mut m = IncidentManager::new();
        let (id, _) = m.open("service:a", Severity::Error, vec![], "s", now());
        // Cannot jump from Open to Resolved.
        assert!(m.transition(id, IncidentStatus::Resolved).is_err());
        // Cannot go backwards.
        m.transition(id, IncidentStatus::Investigating).unwrap();
        assert!(m.transition(id, IncidentStatus::Open).is_err());
    }
}
