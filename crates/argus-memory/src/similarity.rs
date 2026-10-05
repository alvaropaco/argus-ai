//! Incident similarity over **typed fields** (CAP-16/FR-018, ADR-0038 §2):
//! subject kind, symptom signature, resource class, and change proximity.
//! No embeddings, no vector search — every match is a typed comparison that
//! can be cited field by field.

use crate::episodic::Episode;

/// The typed signature of the incident being investigated, matched against
/// remembered episodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncidentSignature {
    /// The subject's resource kind (e.g. "service").
    pub subject_kind: String,
    /// The normalized symptom signature (e.g. "restart-loop").
    pub symptom: String,
    /// The resource class (e.g. "service", "container").
    pub resource_class: String,
    /// How long before the incident a change landed, when known.
    pub change_proximity: Option<chrono::Duration>,
}

/// One remembered episode with its similarity score.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::derive_partial_eq_without_eq)] // f32 score
pub struct ScoredEpisode {
    pub episode: Episode,
    pub score: f32,
    /// Which typed fields matched, for cited evidence.
    pub matched_fields: Vec<&'static str>,
}

// Weights of the four typed fields; they sum to 1.0.
const SUBJECT_KIND_WEIGHT: f32 = 0.25;
const SYMPTOM_WEIGHT: f32 = 0.40;
const RESOURCE_CLASS_WEIGHT: f32 = 0.20;
const CHANGE_PROXIMITY_WEIGHT: f32 = 0.15;

/// Two change proximities are "close" when within one hour of each other,
/// and "related" when within one day.
const PROXIMITY_CLOSE: chrono::Duration = chrono::Duration::hours(1);
const PROXIMITY_RELATED: chrono::Duration = chrono::Duration::hours(24);

/// Score one episode against a signature in [0,1]. Deterministic and
/// explainable: the matched fields are returned with the score.
pub fn similarity_score(
    signature: &IncidentSignature,
    episode: &Episode,
) -> (f32, Vec<&'static str>) {
    let mut score = 0.0;
    let mut matched = Vec::new();

    if episode.resource_class == signature.subject_kind
        || episode.subject.as_str().split(':').next() == Some(signature.subject_kind.as_str())
    {
        score += SUBJECT_KIND_WEIGHT;
        matched.push("subject_kind");
    }
    if episode.symptom == signature.symptom {
        score += SYMPTOM_WEIGHT;
        matched.push("symptom");
    }
    if episode.resource_class == signature.resource_class {
        score += RESOURCE_CLASS_WEIGHT;
        matched.push("resource_class");
    }
    if let (Some(a), Some(b)) = (signature.change_proximity, episode.change_proximity) {
        let delta = (a.num_seconds() - b.num_seconds()).abs();
        if delta <= PROXIMITY_CLOSE.num_seconds() {
            score += CHANGE_PROXIMITY_WEIGHT;
            matched.push("change_proximity");
        } else if delta <= PROXIMITY_RELATED.num_seconds() {
            score += CHANGE_PROXIMITY_WEIGHT / 2.0;
            matched.push("change_proximity~");
        }
    }

    (score.min(1.0), matched)
}

impl crate::EpisodicMemory {
    /// "This looks like N prior incidents": the remembered episodes whose
    /// typed-field similarity to `signature` reaches `min_score`, ordered by
    /// score then time (deterministic).
    pub fn similar_incidents(
        &self,
        signature: &IncidentSignature,
        min_score: f32,
    ) -> Vec<ScoredEpisode> {
        let mut out: Vec<ScoredEpisode> = self
            .all()
            .into_iter()
            .map(|episode| {
                let (score, matched_fields) = similarity_score(signature, episode);
                ScoredEpisode {
                    episode: episode.clone(),
                    score,
                    matched_fields,
                }
            })
            .filter(|s| s.score >= min_score)
            .collect();
        out.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.episode.started_at.cmp(&b.episode.started_at))
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::episodic::{EpisodeOutcome, EpisodicMemory};
    use argus_domain::ResourceId;
    use uuid::Uuid;

    fn ts(minute: u32) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc
            .with_ymd_and_hms(2026, 10, 5, 12, minute, 0)
            .unwrap()
    }

    fn episode(
        subject: &str,
        symptom: &str,
        class: &str,
        proximity: Option<chrono::Duration>,
        minute: u32,
    ) -> Episode {
        Episode {
            id: Uuid::new_v4(),
            subject: ResourceId::new("service", subject).unwrap(),
            symptom: symptom.to_string(),
            resource_class: class.to_string(),
            root_cause: Some("leak".into()),
            remediation: Some("restart".into()),
            outcome: EpisodeOutcome::Resolved,
            change_proximity: proximity,
            started_at: ts(minute),
            resolved_at: Some(ts(minute + 1)),
        }
    }

    fn signature() -> IncidentSignature {
        IncidentSignature {
            subject_kind: "service".into(),
            symptom: "restart-loop".into(),
            resource_class: "service".into(),
            change_proximity: Some(chrono::Duration::minutes(30)),
        }
    }

    #[test]
    fn identical_signature_scores_full_match() {
        let mut m = EpisodicMemory::default();
        m.record(episode(
            "api",
            "restart-loop",
            "service",
            Some(chrono::Duration::minutes(30)),
            0,
        ));
        let scored = m.similar_incidents(&signature(), 0.99);
        assert_eq!(scored.len(), 1);
        assert!((scored[0].score - 1.0).abs() < 1e-6);
        assert_eq!(
            scored[0].matched_fields,
            [
                "subject_kind",
                "symptom",
                "resource_class",
                "change_proximity"
            ]
        );
    }

    #[test]
    fn different_symptom_never_reaches_the_symptom_weight() {
        let mut m = EpisodicMemory::default();
        m.record(episode(
            "api",
            "oom-killed",
            "service",
            Some(chrono::Duration::minutes(30)),
            0,
        ));
        let scored = m.similar_incidents(&signature(), 0.0);
        assert_eq!(scored.len(), 1);
        // Everything but the symptom matched: the score sits exactly at
        // 1.0 − SYMPTOM_WEIGHT (0.60), never above it.
        assert!((scored[0].score - 0.60).abs() < 1e-6);
        assert!(!scored[0].matched_fields.contains(&"symptom"));
    }

    #[test]
    fn ordering_is_score_then_time_and_threshold_filters() {
        let mut m = EpisodicMemory::default();
        // Perfect match, older.
        m.record(episode(
            "api",
            "restart-loop",
            "service",
            Some(chrono::Duration::minutes(30)),
            0,
        ));
        // Perfect match, newer.
        m.record(episode(
            "api",
            "restart-loop",
            "service",
            Some(chrono::Duration::minutes(30)),
            10,
        ));
        // Weak match (no proximity known).
        m.record(episode("billing", "restart-loop", "service", None, 5));

        let scored = m.similar_incidents(&signature(), 0.99);
        assert_eq!(scored.len(), 2);
        // Equal scores: older first.
        assert!(scored[0].episode.started_at <= scored[1].episode.started_at);

        // The weak match is excluded above its score.
        let strict = m.similar_incidents(&signature(), 1.0 - 0.15);
        assert_eq!(strict.len(), 2);
    }

    #[test]
    fn proximity_is_graded_not_binary() {
        // 3h apart: beyond the 1h "close" band, inside the 24h "related" band.
        let related = episode(
            "api",
            "restart-loop",
            "service",
            Some(chrono::Duration::minutes(210)),
            0,
        );
        let (score, matched) = similarity_score(&signature(), &related);
        assert!(matched.contains(&"change_proximity~"));
        assert!((score - 0.925).abs() < 1e-6);

        let far = episode(
            "api",
            "restart-loop",
            "service",
            Some(chrono::Duration::hours(72)),
            0,
        );
        let (score_far, matched_far) = similarity_score(&signature(), &far);
        assert!(!matched_far.contains(&"change_proximity~"));
        assert!(!matched_far.contains(&"change_proximity"));
        assert!((score_far - 0.85).abs() < 1e-6);
    }
}
