//! Episodic memory: incident episodes — what was seen, what the root cause
//! was, what remediated it, and how it ended (ADR-0038 §1).

use std::collections::HashMap;

use argus_domain::ResourceId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// How an episode ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EpisodeOutcome {
    /// The remediation held; the condition did not return within the window.
    Resolved,
    /// The condition returned after remediation.
    Recurred,
    /// Never conclusively fixed.
    Unresolved,
}

/// One remembered incident episode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Episode {
    pub id: Uuid,
    pub subject: ResourceId,
    /// The symptom signature, normalized lowercase (e.g.
    /// "restart-loop", "disk-pressure", "oom-killed").
    pub symptom: String,
    /// The subject's resource kind (e.g. "service", "host", "container").
    pub resource_class: String,
    pub root_cause: Option<String>,
    pub remediation: Option<String>,
    pub outcome: EpisodeOutcome,
    /// How long before the episode a change landed (typed change proximity,
    /// feeding similarity — ADR-0038 §2).
    pub change_proximity: Option<chrono::Duration>,
    pub started_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}

/// The episodic layer: append-only episode records with keyed retrieval.
#[derive(Debug, Default)]
pub struct EpisodicMemory {
    episodes: HashMap<Uuid, Episode>,
}

impl EpisodicMemory {
    /// Record one episode; re-recording the same id replaces it (idempotent
    /// re-derivation, never duplication).
    pub fn record(&mut self, episode: Episode) {
        self.episodes.insert(episode.id, episode);
    }

    pub fn get(&self, id: Uuid) -> Option<&Episode> {
        self.episodes.get(&id)
    }

    /// All episodes for one subject, oldest first.
    pub fn episodes_for(&self, subject: &ResourceId) -> Vec<&Episode> {
        let mut out: Vec<&Episode> = self
            .episodes
            .values()
            .filter(|e| e.subject == *subject)
            .collect();
        out.sort_by_key(|e| e.started_at);
        out
    }

    /// All episodes, oldest first (deterministic order).
    pub fn all(&self) -> Vec<&Episode> {
        let mut out: Vec<&Episode> = self.episodes.values().collect();
        out.sort_by_key(|e| e.started_at);
        out
    }

    /// Update an episode's outcome (e.g. a `Resolved` episode that later
    /// recurred). Unknown ids are rejected.
    pub fn set_outcome(
        &mut self,
        id: Uuid,
        outcome: EpisodeOutcome,
        resolved_at: Option<DateTime<Utc>>,
    ) -> Option<()> {
        let episode = self.episodes.get_mut(&id)?;
        episode.outcome = outcome;
        episode.resolved_at = resolved_at;
        Some(())
    }

    pub fn len(&self) -> usize {
        self.episodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.episodes.is_empty()
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

    fn episode(subject: &str, symptom: &str, minute: u32) -> Episode {
        Episode {
            id: Uuid::new_v4(),
            subject: ResourceId::new("service", subject).unwrap(),
            symptom: symptom.to_string(),
            resource_class: "service".to_string(),
            root_cause: Some("memory leak".to_string()),
            remediation: Some("restart".to_string()),
            outcome: EpisodeOutcome::Resolved,
            change_proximity: None,
            started_at: ts(minute),
            resolved_at: Some(ts(minute + 5)),
        }
    }

    #[test]
    fn episodes_listed_oldest_first_per_subject() {
        let mut m = EpisodicMemory::default();
        let a = episode("api", "restart-loop", 10);
        let b = episode("api", "oom-killed", 2);
        let c = episode("billing", "restart-loop", 5);
        m.record(a.clone());
        m.record(b.clone());
        m.record(c);

        let for_api = m.episodes_for(&ResourceId::new("service", "api").unwrap());
        assert_eq!(for_api.len(), 2);
        assert_eq!(for_api[0].id, b.id);
        assert_eq!(for_api[1].id, a.id);
    }

    #[test]
    fn outcome_updates_are_explicit() {
        let mut m = EpisodicMemory::default();
        let e = episode("api", "disk-pressure", 0);
        let id = e.id;
        m.record(e);
        m.set_outcome(id, EpisodeOutcome::Recurred, None).unwrap();
        assert_eq!(m.get(id).unwrap().outcome, EpisodeOutcome::Recurred);
        assert!(
            m.set_outcome(Uuid::new_v4(), EpisodeOutcome::Resolved, None)
                .is_none()
        );
    }
}
