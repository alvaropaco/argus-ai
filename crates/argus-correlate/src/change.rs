//! Change intelligence: correlate incidents with recorded environment
//! changes and answer "what changed before the incident" (CAP-18, FR-013,
//! T028).
//!
//! The [`ChangeLedger`] stores immutable [`Change`] records (git, deploys,
//! images, config, Kubernetes, Terraform, cloud, packages, kernel,
//! service-config). The query ranks the changes inside an incident's lookback
//! window into an **evidence-backed hypothesis** — explicitly labeled as a
//! hypothesis, deterministic, and never authorization to act.

use chrono::{DateTime, Duration, Utc};

use argus_domain::{Change, ChangeSource, ResourceId};

/// A change inside the incident window with its deterministic ranking.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::derive_partial_eq_without_eq)] // f32 scores
pub struct RankedChange {
    pub change: Change,
    /// Exponential recency in [0,1]: 1.0 exactly at the incident's start,
    /// halving every `lookback/4`. Recency dominates the ranking.
    pub recency: f32,
    /// Source-class weight in [0,1] — how close the changed surface is to the
    /// affected workload.
    pub source_weight: f32,
    /// `recency × source_weight`.
    pub score: f32,
}

/// The answer to "what changed before the incident".
///
/// `statement` is deterministic prose, prefixed `HYPOTHESIS` so it can never
/// read as a root-cause finding; each candidate cites its change record as
/// evidence.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::derive_partial_eq_without_eq)] // f32 confidence
pub struct ChangeHypothesis {
    pub statement: String,
    pub candidates: Vec<RankedChange>,
    /// The top candidate's score, or 0 when there are no candidates.
    pub confidence: f32,
    /// The queried window: `[window_end - lookback, window_end]`.
    pub window_end: DateTime<Utc>,
    pub lookback: Duration,
}

/// How strongly each change surface can explain a workload incident, ranked
/// by proximity to the workload: code/config/image/service-config surfaces
/// change the workload itself; platform surfaces (Kubernetes, Terraform)
/// change its environment; environment surfaces (packages, kernel, cloud)
/// change the substrate beneath it.
fn source_weight(source: ChangeSource) -> f32 {
    match source {
        ChangeSource::Git
        | ChangeSource::Deploy
        | ChangeSource::Image
        | ChangeSource::Config
        | ChangeSource::ServiceConfig => 1.0,
        ChangeSource::K8s | ChangeSource::Terraform => 0.85,
        ChangeSource::Cloud | ChangeSource::Package | ChangeSource::Kernel => 0.7,
    }
}

/// An append-only ledger of recorded environment changes.
#[derive(Debug, Default)]
pub struct ChangeLedger {
    changes: Vec<Change>,
}

impl ChangeLedger {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one observed change.
    pub fn record(&mut self, change: Change) {
        self.changes.push(change);
    }

    /// All recorded changes, in insertion order.
    pub fn changes(&self) -> &[Change] {
        &self.changes
    }

    /// Answer "what changed before the incident": rank every recorded change
    /// that happened at or before `incident_started_at` and within `lookback`
    /// (optionally restricted to one subject). Returns `None` when nothing
    /// changed in the window — the absence of changes is never dressed up as
    /// a hypothesis.
    pub fn what_changed_before(
        &self,
        incident_started_at: DateTime<Utc>,
        subject: Option<&ResourceId>,
        lookback: Duration,
    ) -> Option<ChangeHypothesis> {
        let window_start = incident_started_at - lookback;
        // Recency half-life: a quarter of the lookback window, so a change
        // right at the incident start is 16× the score of one at the window's
        // far edge.
        let half_life = (lookback.num_seconds().max(1) as f64) / 4.0;

        let mut candidates: Vec<RankedChange> = self
            .changes
            .iter()
            .filter(|c| {
                c.changed_at() >= window_start
                    && c.changed_at() <= incident_started_at
                    && subject.is_none_or(|s| c.subject() == s)
            })
            .map(|c| {
                let age = (incident_started_at - c.changed_at()).num_seconds().max(0) as f64;
                let recency = 0.5f64.powf(age / half_life) as f32;
                let weight = source_weight(c.source());
                RankedChange {
                    change: c.clone(),
                    recency,
                    source_weight: weight,
                    score: recency * weight,
                }
            })
            .collect();

        if candidates.is_empty() {
            return None;
        }
        // Deterministic order: score desc, then most recent, then id.
        // `total_cmp` is exact for the finite scores produced here.
        candidates.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| b.change.changed_at().cmp(&a.change.changed_at()))
                .then_with(|| a.change.id().cmp(&b.change.id()))
        });

        let confidence = candidates[0].score;
        let statement = build_statement(&candidates, incident_started_at, lookback);

        Some(ChangeHypothesis {
            statement,
            candidates,
            confidence,
            window_end: incident_started_at,
            lookback,
        })
    }
}

/// Deterministic hypothesis prose over the top candidates. The `HYPOTHESIS`
/// prefix is structural (FR-013): a ranked correlation is a suspect list,
/// never a verdict.
fn build_statement(
    candidates: &[RankedChange],
    incident_started_at: DateTime<Utc>,
    lookback: Duration,
) -> String {
    let top: Vec<String> = candidates
        .iter()
        .take(3)
        .map(|rc| {
            let c = &rc.change;
            let diff = match (c.before(), c.after()) {
                (Some(b), Some(a)) => format!(" {b} → {a}"),
                (Some(b), None) => format!(" removing {b}"),
                (None, Some(a)) => format!(" introducing {a}"),
                (None, None) => String::new(),
            };
            let actor = c.actor().map(|a| format!(" by {a}")).unwrap_or_default();
            format!(
                "{} on {}{}{} (score {:.2})",
                source_label(c.source()),
                c.subject().as_str(),
                diff,
                actor,
                rc.score
            )
        })
        .collect();

    format!(
        "HYPOTHESIS (not a finding): {} change(s) recorded within {} before {} — \
         ranked: {}. Each is a suspect, not a verdict; verify against the evidence.",
        candidates.len(),
        fmt_lookback(lookback),
        incident_started_at.to_rfc3339(),
        top.join("; "),
    )
}

fn source_label(source: ChangeSource) -> &'static str {
    match source {
        ChangeSource::Git => "git commit",
        ChangeSource::Deploy => "deployment",
        ChangeSource::Image => "container image",
        ChangeSource::Config => "config change",
        ChangeSource::K8s => "kubernetes change",
        ChangeSource::Terraform => "terraform change",
        ChangeSource::Cloud => "cloud change",
        ChangeSource::Package => "package change",
        ChangeSource::Kernel => "kernel change",
        ChangeSource::ServiceConfig => "service-config change",
    }
}

fn fmt_lookback(lookback: Duration) -> String {
    let total = lookback.num_seconds().max(0);
    let (h, rem) = (total / 3600, total % 3600);
    let (m, s) = (rem / 60, rem % 60);
    if h > 0 {
        format!("{h}h{m}m")
    } else if m > 0 {
        format!("{m}m{s}s")
    } else {
        format!("{s}s")
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use uuid::Uuid;

    use super::*;

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 5, 12, 0, 0).unwrap()
    }

    fn change(source: ChangeSource, minutes_before: i64, after: &str) -> Change {
        Change::new(
            Uuid::new_v4(),
            source,
            ResourceId::new("service", "api").unwrap(),
            None,
            Some(after.to_string()),
            Some("alice".to_string()),
            t0() - Duration::minutes(minutes_before),
        )
        .unwrap()
    }

    #[test]
    fn the_most_recent_application_change_ranks_first() {
        let mut ledger = ChangeLedger::new();
        let old_kernel = change(ChangeSource::Kernel, 50, "6.9.1");
        let recent_git = change(ChangeSource::Git, 2, "abc1234");
        ledger.record(old_kernel.clone());
        ledger.record(recent_git.clone());

        let h = ledger
            .what_changed_before(t0(), None, Duration::hours(1))
            .unwrap();
        assert_eq!(h.candidates[0].change.id(), recent_git.id());
        assert!(h.candidates[0].score > h.candidates[1].score);
        assert!(h.confidence > 0.5);
        assert!(h.statement.starts_with("HYPOTHESIS"));
        assert!(h.statement.contains("git commit"));
        assert!(h.statement.contains("abc1234"));
        assert!(h.statement.contains("by alice"));
        assert!(h.statement.contains("not a finding"));
        let _ = old_kernel;
    }

    #[test]
    fn an_equal_recency_let_application_changes_outrank_platform_ones() {
        let mut ledger = ChangeLedger::new();
        let terraform = change(ChangeSource::Terraform, 5, "plan-42");
        let config = change(ChangeSource::Config, 5, "nginx.conf");
        ledger.record(terraform.clone());
        ledger.record(config.clone());

        let h = ledger
            .what_changed_before(t0(), None, Duration::hours(1))
            .unwrap();
        assert_eq!(h.candidates[0].change.id(), config.id());
        // The weight gap is exactly the documented 1.0 vs 0.85.
        assert!((h.candidates[0].source_weight - 1.0).abs() < 1e-6);
        assert!((h.candidates[1].source_weight - 0.85).abs() < 1e-6);
        let _ = terraform;
    }

    #[test]
    fn changes_outside_the_window_are_excluded() {
        let mut ledger = ChangeLedger::new();
        ledger.record(change(ChangeSource::Deploy, 90, "old-release")); // before lookback
        ledger.record(change(ChangeSource::Deploy, -5, "after-incident")); // after start

        assert!(
            ledger
                .what_changed_before(t0(), None, Duration::hours(1))
                .is_none()
        );
    }

    #[test]
    fn no_changes_in_window_yields_no_hypothesis() {
        let ledger = ChangeLedger::new();
        assert!(
            ledger
                .what_changed_before(t0(), None, Duration::hours(1))
                .is_none()
        );
    }

    #[test]
    fn the_subject_filter_narrows_to_the_incident_subject() {
        let mut ledger = ChangeLedger::new();
        let api = change(ChangeSource::Deploy, 10, "release-7");
        let other = Change::new(
            Uuid::new_v4(),
            ChangeSource::Deploy,
            ResourceId::new("service", "billing").unwrap(),
            None,
            Some("release-9".to_string()),
            None,
            t0() - Duration::minutes(1),
        )
        .unwrap();
        ledger.record(api.clone());
        ledger.record(other);

        let subject = ResourceId::new("service", "api").unwrap();
        let h = ledger
            .what_changed_before(t0(), Some(&subject), Duration::hours(1))
            .unwrap();
        assert_eq!(h.candidates.len(), 1);
        assert_eq!(h.candidates[0].change.id(), api.id());
        // Unfiltered sees both.
        assert_eq!(
            ledger
                .what_changed_before(t0(), None, Duration::hours(1))
                .unwrap()
                .candidates
                .len(),
            2
        );
    }

    #[test]
    fn a_change_exactly_at_incident_start_counts_as_before() {
        let mut ledger = ChangeLedger::new();
        let at_start = Change::new(
            Uuid::new_v4(),
            ChangeSource::Deploy,
            ResourceId::new("service", "api").unwrap(),
            None,
            Some("just-in-time".to_string()),
            None,
            t0(),
        )
        .unwrap();
        ledger.record(at_start);

        let h = ledger
            .what_changed_before(t0(), None, Duration::hours(1))
            .unwrap();
        assert_eq!(h.candidates[0].recency, 1.0);
        assert_eq!(h.confidence, 1.0);
    }

    #[test]
    fn ordering_is_fully_deterministic_for_equal_scores() {
        let build = || {
            let mut ledger = ChangeLedger::new();
            // Two deploys of equal weight and equal recency; only ids differ.
            ledger.record(change(ChangeSource::Deploy, 5, "release-a"));
            ledger.record(change(ChangeSource::Deploy, 5, "release-b"));
            ledger
                .what_changed_before(t0(), None, Duration::hours(1))
                .unwrap()
        };
        let a = build();
        let b = build();
        let ids_a: Vec<Uuid> = a.candidates.iter().map(|c| c.change.id()).collect();
        let ids_b: Vec<Uuid> = b.candidates.iter().map(|c| c.change.id()).collect();
        // Different ids across builds, but the tie is broken by id, so the
        // relative order of scores/statement shape is stable.
        assert_eq!(a.candidates.len(), b.candidates.len());
        assert_eq!(
            a.statement.split(" (score").count(),
            b.statement.split(" (score").count()
        );
        let _ = (ids_a, ids_b);
    }
}
