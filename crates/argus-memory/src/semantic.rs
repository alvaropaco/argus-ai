//! Semantic memory: typed facts and dependency relationships — structured
//! knowledge retrieved by keyed queries, **never** embeddings (ADR-0038 §2).

use std::collections::{HashMap, HashSet};

use argus_domain::ResourceId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A typed fact about a resource (e.g. subject `host:local`, attribute
/// `kernel.release`, value `6.9.1`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fact {
    pub subject: ResourceId,
    pub attribute: String,
    pub value: String,
    /// Where the fact came from (observation source, operator, runbook).
    pub source: String,
    pub learned_at: DateTime<Utc>,
}

/// A typed relationship between two resources. The vocabulary is a closed
/// subset of the environment-graph relationship kinds (ADR-0033) relevant to
/// memory: what depends on what, what runs where.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationshipKind {
    DependsOn,
    RunsOn,
    Manages,
}

impl RelationshipKind {
    /// For impact analysis: the edges that, reversed, tell us who is affected
    /// when the target breaks.
    pub fn is_impact_relevant(self) -> bool {
        matches!(self, Self::DependsOn)
    }
}

/// One recorded relationship `from → kind → to`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relationship {
    pub from: ResourceId,
    pub kind: RelationshipKind,
    pub to: ResourceId,
    pub learned_at: DateTime<Utc>,
}

/// The semantic layer: facts and relationships with keyed retrieval.
#[derive(Debug, Default)]
pub struct SemanticMemory {
    facts: HashMap<(ResourceId, String), Fact>,
    relationships: HashSet<RelationshipTriple>,
}

/// The stored identity of a relationship edge. Equality and hash are on the
/// endpoints and kind only — the first-seen timestamp is metadata, so
/// re-recording an edge never duplicates it.
#[derive(Debug, Clone)]
struct RelationshipTriple {
    from: ResourceId,
    kind: RelationshipKind,
    to: ResourceId,
    learned_at: DateTime<Utc>,
}

impl PartialEq for RelationshipTriple {
    fn eq(&self, other: &Self) -> bool {
        self.from == other.from && self.kind == other.kind && self.to == other.to
    }
}

impl Eq for RelationshipTriple {}

impl std::hash::Hash for RelationshipTriple {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.from.as_str().hash(state);
        self.kind.hash(state);
        self.to.as_str().hash(state);
    }
}

impl SemanticMemory {
    /// Record a fact (re-recording the same subject+attribute supersedes).
    pub fn add_fact(&mut self, fact: Fact) {
        self.facts
            .insert((fact.subject.clone(), fact.attribute.clone()), fact);
    }

    /// The fact for one subject+attribute, if known.
    pub fn fact(&self, subject: &ResourceId, attribute: &str) -> Option<&Fact> {
        self.facts.get(&(subject.clone(), attribute.to_string()))
    }

    /// All facts about one subject, ordered by attribute.
    pub fn facts_about(&self, subject: &ResourceId) -> Vec<&Fact> {
        let mut out: Vec<&Fact> = self
            .facts
            .values()
            .filter(|f| f.subject == *subject)
            .collect();
        out.sort_by(|a, b| a.attribute.cmp(&b.attribute));
        out
    }

    /// Record `from → kind → to` (idempotent; re-recording keeps the first
    /// seen timestamp).
    pub fn add_relationship(
        &mut self,
        from: ResourceId,
        kind: RelationshipKind,
        to: ResourceId,
        learned_at: DateTime<Utc>,
    ) {
        self.relationships.insert(RelationshipTriple {
            from,
            kind,
            to,
            learned_at,
        });
    }

    /// What `subject` depends on (direct), ordered by resource id.
    pub fn dependencies_of(&self, subject: &ResourceId) -> Vec<&ResourceId> {
        let mut out: Vec<&ResourceId> = self
            .relationships
            .iter()
            .filter(|t| t.kind == RelationshipKind::DependsOn && &t.from == subject)
            .map(|t| &t.to)
            .collect();
        out.sort_by_key(|r| r.as_str().to_string());
        out
    }

    /// What depends on `subject` (direct), ordered by resource id — the
    /// first hop of impact analysis.
    pub fn dependents_of(&self, subject: &ResourceId) -> Vec<&ResourceId> {
        let mut out: Vec<&ResourceId> = self
            .relationships
            .iter()
            .filter(|t| t.kind == RelationshipKind::DependsOn && &t.to == subject)
            .map(|t| &t.from)
            .collect();
        out.sort_by_key(|r| r.as_str().to_string());
        out
    }

    /// All relationships as ordered records (deterministic rendering order).
    pub fn relationships(&self) -> Vec<Relationship> {
        let mut out: Vec<Relationship> = self
            .relationships
            .iter()
            .map(|t| Relationship {
                from: t.from.clone(),
                kind: t.kind,
                to: t.to.clone(),
                learned_at: t.learned_at,
            })
            .collect();
        out.sort_by(|a, b| {
            a.from
                .as_str()
                .cmp(b.from.as_str())
                .then_with(|| a.to.as_str().cmp(b.to.as_str()))
        });
        out
    }

    pub fn len(&self) -> usize {
        self.facts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.facts.is_empty() && self.relationships.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn ts() -> DateTime<Utc> {
        chrono::Utc.with_ymd_and_hms(2026, 10, 5, 12, 0, 0).unwrap()
    }

    fn rid(kind: &str, id: &str) -> ResourceId {
        ResourceId::new(kind, id).unwrap()
    }

    #[test]
    fn facts_supersede_and_list_deterministically() {
        let mut m = SemanticMemory::default();
        let host = rid("host", "local");
        m.add_fact(Fact {
            subject: host.clone(),
            attribute: "kernel.release".into(),
            value: "6.8.0".into(),
            source: "uname".into(),
            learned_at: ts(),
        });
        m.add_fact(Fact {
            subject: host.clone(),
            attribute: "kernel.release".into(),
            value: "6.9.1".into(),
            source: "uname".into(),
            learned_at: ts(),
        });
        m.add_fact(Fact {
            subject: host.clone(),
            attribute: "cpu.cores".into(),
            value: "8".into(),
            source: "procfs".into(),
            learned_at: ts(),
        });
        assert_eq!(m.len(), 2);
        assert_eq!(m.fact(&host, "kernel.release").unwrap().value, "6.9.1");
        let attrs: Vec<&str> = m
            .facts_about(&host)
            .iter()
            .map(|f| f.attribute.as_str())
            .collect();
        assert_eq!(attrs, ["cpu.cores", "kernel.release"]);
    }

    #[test]
    fn dependency_edges_answer_both_directions_idempotently() {
        let mut m = SemanticMemory::default();
        let (app, db, cache) = (
            rid("service", "app"),
            rid("service", "db"),
            rid("service", "cache"),
        );
        m.add_relationship(app.clone(), RelationshipKind::DependsOn, db.clone(), ts());
        m.add_relationship(
            app.clone(),
            RelationshipKind::DependsOn,
            cache.clone(),
            ts(),
        );
        m.add_relationship(app.clone(), RelationshipKind::DependsOn, db.clone(), ts());

        let deps: Vec<String> = m
            .dependencies_of(&app)
            .iter()
            .map(|r| r.as_str().to_string())
            .collect();
        assert_eq!(deps, ["service:cache", "service:db"]);
        let dependents: Vec<String> = m
            .dependents_of(&db)
            .iter()
            .map(|r| r.as_str().to_string())
            .collect();
        assert_eq!(dependents, ["service:app"]);
        // Both db and cache are dependencies of app; the reverse lookup for
        // cache is app too. A resource with no incoming edges has none.
        let cache_dependents: Vec<String> = m
            .dependents_of(&cache)
            .iter()
            .map(|r| r.as_str().to_string())
            .collect();
        assert_eq!(cache_dependents, ["service:app"]);
        assert!(m.dependents_of(&rid("service", "orphan")).is_empty());
    }
}
