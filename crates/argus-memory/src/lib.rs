//! ARGUS operational memory (spec 003 M6, CAP-16, FR-018, ADR-0038).
//!
//! Five layers, all **structured, deterministic records** retrieved by keyed,
//! typed queries — never embeddings (ADR-0038 §2):
//!
//! - [`WorkingMemory`] — in-process, bounded per-investigation evidence.
//! - [`OperationalMemory`] — the latest observed value per signal, with a
//!   freshness gate so stale state is never served as current.
//! - [`EpisodicMemory`] — incident episodes (symptom, root cause,
//!   remediation, outcome) with typed-field similarity.
//! - [`SemanticMemory`] — typed facts and dependency relationships.
//! - [`ProceduralMemory`] — learned-procedure records with success history.
//!
//! Persistence stays behind the repository abstraction; this crate holds the
//! record shapes and the retrieval semantics and imports no store API.

mod episodic;
mod operational;
mod procedural;
mod semantic;
mod similarity;
mod working;

pub use episodic::{Episode, EpisodeOutcome, EpisodicMemory};
pub use operational::{Current, OperationalMemory};
pub use procedural::{ProceduralMemory, ProcedureRecord, ProcedureStatus};
pub use semantic::{Fact, Relationship, RelationshipKind, SemanticMemory};
pub use similarity::{IncidentSignature, ScoredEpisode, similarity_score};
pub use working::WorkingMemory;

use chrono::{DateTime, Utc};

/// The five memory layers in one handle. Every layer is independently
/// constructible; the aggregate exists so a consumer (the daemon, the
/// investigator) can carry one memory through the reasoning loop.
#[derive(Debug, Default)]
pub struct Memory {
    pub working: WorkingMemory,
    pub operational: OperationalMemory,
    pub episodic: EpisodicMemory,
    pub semantic: SemanticMemory,
    pub procedural: ProceduralMemory,
}

/// Whether a record's timestamp is fresh against `now`.
///
/// Shared freshness rule: a record older than `max_age` is stale, and stale
/// records are never served as current (CAP-24: degrade safely rather than
/// act on stale data).
pub fn is_fresh(at: DateTime<Utc>, now: DateTime<Utc>, max_age: chrono::Duration) -> bool {
    now.signed_duration_since(at) <= max_age
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freshness_bounds() {
        let now = chrono::Utc::now();
        assert!(is_fresh(now, now, chrono::Duration::minutes(5)));
        assert!(!is_fresh(
            now - chrono::Duration::minutes(6),
            now,
            chrono::Duration::minutes(5)
        ));
    }
}
