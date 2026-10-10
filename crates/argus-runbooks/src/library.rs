//! The runbook library: keyed storage and deterministic retrieval by
//! trigger, status, and name.

use std::collections::HashMap;

use uuid::Uuid;

use crate::runbook::{Runbook, RunbookStatus, RunbookTrigger};

/// A keyed runbook collection. Retrieval is deterministic everywhere:
/// matches are ordered by name, never by recency or "relevance".
#[derive(Debug, Default)]
pub struct RunbookLibrary {
    runbooks: HashMap<Uuid, Runbook>,
}

impl RunbookLibrary {
    /// Register a runbook (idempotent on id; names are checked for collisions
    /// across *different* ids, so retrieval by name stays unambiguous).
    pub fn register(&mut self, runbook: Runbook) -> Result<(), String> {
        if let Some(existing) = self
            .runbooks
            .values()
            .find(|r| r.name == runbook.name && r.id != runbook.id)
        {
            return Err(format!(
                "runbook name '{}' already used by {}",
                runbook.name, existing.id
            ));
        }
        self.runbooks.insert(runbook.id, runbook);
        Ok(())
    }

    pub fn get(&self, id: Uuid) -> Option<&Runbook> {
        self.runbooks.get(&id)
    }

    /// The runbook with this exact name, if any (names are unique across ids).
    pub fn by_name(&self, name: &str) -> Option<&Runbook> {
        self.runbooks.values().find(|r| r.name == name)
    }

    /// Removes the runbook named `name` (supersede support, spec 009 FR-001:
    /// a re-delivery replaces the previous candidate). Returns whether one
    /// was removed.
    pub fn remove(&mut self, name: &str) -> bool {
        let id = self
            .runbooks
            .values()
            .find(|r| r.name == name)
            .map(|r| r.id);
        match id {
            Some(id) => self.runbooks.remove(&id).is_some(),
            None => false,
        }
    }

    /// All runbooks, ordered by name (deterministic).
    pub fn list(&self) -> Vec<&Runbook> {
        let mut out: Vec<&Runbook> = self.runbooks.values().collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    pub fn get_mut(&mut self, id: Uuid) -> Option<&mut Runbook> {
        self.runbooks.get_mut(&id)
    }

    /// The mutable runbook with this exact name, if any (the promotion
    /// ladder's entry point for named candidates).
    pub fn by_name_mut(&mut self, name: &str) -> Option<&mut Runbook> {
        self.runbooks.values_mut().find(|r| r.name == name)
    }

    /// Runbooks whose trigger matches, ordered by name.
    pub fn matching(&self, trigger: &RunbookTrigger) -> Vec<&Runbook> {
        let mut out: Vec<&Runbook> = self
            .runbooks
            .values()
            .filter(|r| r.matches_trigger(trigger))
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Runbooks in one status, ordered by name.
    pub fn by_status(&self, status: RunbookStatus) -> Vec<&Runbook> {
        let mut out: Vec<&Runbook> = self
            .runbooks
            .values()
            .filter(|r| r.status() == status)
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Promoted runbooks matching a trigger — the only ones eligible to
    /// drive procedures (ADR-0036 §3), ordered by name.
    pub fn promotable(&self, trigger: &RunbookTrigger) -> Vec<&Runbook> {
        self.matching(trigger)
            .into_iter()
            .filter(|r| r.status() == RunbookStatus::Promoted)
            .collect()
    }

    pub fn len(&self) -> usize {
        self.runbooks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.runbooks.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runbook::EvidenceKind;
    use argus_domain::CapabilityId;

    fn runbook(name: &str, symptom: &str) -> Runbook {
        Runbook::candidate(
            Uuid::new_v4(),
            name,
            RunbookTrigger::Symptom(symptom.into()),
            vec![EvidenceKind::Logs],
            vec![],
            vec![],
            vec![CapabilityId::new("host.service.restart").unwrap()],
            vec![],
            vec![],
        )
    }

    #[test]
    fn duplicate_names_across_ids_are_rejected() {
        let mut lib = RunbookLibrary::default();
        lib.register(runbook("restart-procedure", "restart-loop"))
            .unwrap();
        let same_name_other_id = runbook("restart-procedure", "oom-killed");
        assert!(lib.register(same_name_other_id).is_err());
        assert_eq!(lib.len(), 1);
    }

    #[test]
    fn matching_is_ordered_by_name_and_filters_by_trigger() {
        let mut lib = RunbookLibrary::default();
        lib.register(runbook("zebra", "restart-loop")).unwrap();
        lib.register(runbook("alpha", "restart-loop")).unwrap();
        lib.register(runbook("mid", "oom-killed")).unwrap();

        let trigger = RunbookTrigger::Symptom("restart-loop".into());
        let names: Vec<&str> = lib
            .matching(&trigger)
            .iter()
            .map(|r| r.name.as_str())
            .collect();
        assert_eq!(names, ["alpha", "zebra"]);
        assert!(lib.by_status(RunbookStatus::Candidate).len() == 3);
        // Nothing promoted yet: no promotable runbooks.
        assert!(lib.promotable(&trigger).is_empty());
    }
}
