//! Assimilation and graduated autonomy state (spec 008, ADR-0040).
//!
//! Authority is *earned* per environment, *bounded* per period, and
//! *revocable*. The daemon keeps a persisted [`AutonomyState`] — the
//! assimilation phase, the earned rung on the L2 → L3 → L4 ladder, the
//! clean-cycle gate counters, and the blast-radius budget windows — while the
//! managed `brain.autonomy` stays the operator's **ceiling**. Every consumer
//! reads the *effective* autonomy, `min(ceiling, earned)`, never either
//! number alone (ADR-0040 §1).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{AutonomyMode, EnvironmentId, PlanStatus};

/// The assimilation phase (spec 008 FR-001): where the daemon is on the
/// acquisition path MAP → SHADOW → EARNED.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssimilationPhase {
    /// Acquiring the environment: the machine cannot leave `mapping` until
    /// live state has been observed and provider readiness is known.
    #[default]
    Mapping,
    /// Bounded cycles building baselines and proving clean behavior.
    Shadow,
    /// Authority earned rung-by-rung toward the operator's ceiling.
    Earned,
}

impl AssimilationPhase {
    /// The canonical snake_case name (`mapping` | `shadow` | `earned`).
    pub fn canonical_name(&self) -> &'static str {
        match self {
            Self::Mapping => "mapping",
            Self::Shadow => "shadow",
            Self::Earned => "earned",
        }
    }
}

/// The blast-radius budget windows (spec 008 FR-004): anchored UTC hour/day
/// buckets, not sliding — deterministic, restart-safe, cheap. Each window is
/// `(anchor index, consumed)`; a window whose anchor no longer matches the
/// current hour/day has rolled over and counts as empty.
///
/// Anchors are `hours/days since the UTC epoch`, computed by the pure
/// evaluator in `argus-policy::budget` — this crate carries the state, never
/// the clock math.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BudgetWindows {
    /// Low-risk auto-executions within the current UTC hour.
    pub low_risk_hour: Option<(i64, u32)>,
    /// Controlled-risk auto-executions within the current UTC day.
    pub controlled_day: Option<(i64, u32)>,
    /// Host-scope auto-executions within the current UTC day, any risk class.
    pub host_scope_day: Option<(i64, u32)>,
}

/// The persisted per-environment autonomy state (spec 008 FR-001): phase,
/// earned rung, gate counters, and budget windows. It survives restart;
/// corrupt state, a foreign environment id, or an unreadable store re-assimilates
/// from `mapping` at L0 — the fail-closed readings (ADR-0040 §4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AutonomyState {
    /// The environment this state belongs to; a mismatch with the daemon's
    /// environment id reads as a fresh environment (state is per environment).
    pub environment_id: EnvironmentId,
    pub phase: AssimilationPhase,
    /// The earned rung: L0, L2, L3, or L4 — L1 is a labeling mode the ladder
    /// skips, and L5 (learning) is never earned, only operator-granted.
    pub earned: AutonomyMode,
    /// Clean cycles accumulated in the shadow window.
    pub shadow_clean_cycles: u32,
    /// Clean cycles accumulated at the current rung.
    pub rung_clean_cycles: u32,
    /// Validated (Completed) executions since the last rung change — the
    /// behavior evidence a rung promotion additionally requires (ADR-0040:
    /// clean cycles are "work actually done and validated"; a quiet paired
    /// host proves reliability, never competence). Reset on every promotion
    /// and demotion. `#[serde(default)]` so state written before this field
    /// still parses — a missing count simply means "no validated work yet".
    #[serde(default)]
    pub validated_actions_since_rung: u32,
    pub budget: BudgetWindows,
    pub updated_at: DateTime<Utc>,
}

impl AutonomyState {
    /// A fresh state: `mapping`, earned L0, empty counters — the fail-closed
    /// starting point for a new (or corrupted) environment.
    pub fn fresh(environment_id: EnvironmentId, now: DateTime<Utc>) -> Self {
        Self {
            environment_id,
            phase: AssimilationPhase::Mapping,
            earned: AutonomyMode::L0Observe,
            shadow_clean_cycles: 0,
            rung_clean_cycles: 0,
            validated_actions_since_rung: 0,
            budget: BudgetWindows::default(),
            updated_at: now,
        }
    }
}

/// Whether a plan's terminal status is a validation failure — the revocation
/// signature (spec 008 FR-003): the daemon acted and its own re-observation
/// contradicted it (`RolledBack`), or the failure could not be fully undone
/// (`NeedsManual`). Shared by the runtime's plan-terminal hook and the
/// brain's per-cycle signal, so the failure surface and the revocation
/// surface cannot drift.
pub fn is_validation_failure(status: PlanStatus) -> bool {
    matches!(status, PlanStatus::RolledBack | PlanStatus::NeedsManual)
}

impl AutonomyMode {
    /// The trust rank of the level (L0 = 0 … L5 = 5), for comparisons.
    pub fn rank(self) -> u8 {
        match self {
            Self::L0Observe => 0,
            Self::L1Explain => 1,
            Self::L2Recommend => 2,
            Self::L3Assisted => 3,
            Self::L4Autonomous => 4,
            Self::L5Adaptive => 5,
        }
    }

    /// The lower of two levels: the effective autonomy is always
    /// `min(ceiling, earned)` (spec 008 FR-003) — no path may exceed the
    /// managed ceiling.
    pub fn lower_of(self, other: Self) -> Self {
        if self.rank() <= other.rank() {
            self
        } else {
            other
        }
    }

    /// The next rung up the promotion ladder, or `None` at the top. The
    /// ladder runs L2 → L3 → L4: L1 is a labeling mode, not a trust rung,
    /// and L5 (learning) is reachable only by explicit operator grant
    /// (spec 008 FR-002, ADR-0040 §3).
    pub fn next_rung(self) -> Option<Self> {
        match self {
            Self::L0Observe | Self::L1Explain => Some(Self::L2Recommend),
            Self::L2Recommend => Some(Self::L3Assisted),
            Self::L3Assisted => Some(Self::L4Autonomous),
            Self::L4Autonomous | Self::L5Adaptive => None,
        }
    }

    /// One rung down the ladder — the demotion step: L4 → L3 → L2 → L0.
    /// The ladder skips L1 in both directions; L5 demotes like L4 (its action
    /// boundary is identical), and anything at L0 stays L0.
    pub fn demote_rung(self) -> Self {
        match self {
            Self::L3Assisted => Self::L2Recommend,
            Self::L4Autonomous | Self::L5Adaptive => Self::L3Assisted,
            _ => Self::L0Observe,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_failures_are_the_revocation_statuses() {
        assert!(is_validation_failure(PlanStatus::RolledBack));
        assert!(is_validation_failure(PlanStatus::NeedsManual));
        for fine in [
            PlanStatus::Proposed,
            PlanStatus::Approved,
            PlanStatus::Executing,
            PlanStatus::Completed,
            PlanStatus::Failed,
            PlanStatus::Denied,
            PlanStatus::AwaitingApproval,
            PlanStatus::RollingBack,
        ] {
            assert!(
                !is_validation_failure(fine),
                "{fine:?} is not terminal evidence"
            );
        }
    }

    #[test]
    fn ranks_order_the_levels() {
        assert!(AutonomyMode::L0Observe.rank() < AutonomyMode::L2Recommend.rank());
        assert!(AutonomyMode::L2Recommend.rank() < AutonomyMode::L3Assisted.rank());
        assert!(AutonomyMode::L3Assisted.rank() < AutonomyMode::L4Autonomous.rank());
        assert!(AutonomyMode::L4Autonomous.rank() < AutonomyMode::L5Adaptive.rank());
    }

    #[test]
    fn effective_is_always_the_lower_level() {
        // The ceiling binds: earned above the ceiling reads at the ceiling.
        assert_eq!(
            AutonomyMode::L4Autonomous.lower_of(AutonomyMode::L2Recommend),
            AutonomyMode::L2Recommend
        );
        // The earned rung binds when it is lower.
        assert_eq!(
            AutonomyMode::L2Recommend.lower_of(AutonomyMode::L4Autonomous),
            AutonomyMode::L2Recommend
        );
        assert_eq!(
            AutonomyMode::L3Assisted.lower_of(AutonomyMode::L3Assisted),
            AutonomyMode::L3Assisted
        );
    }

    #[test]
    fn the_ladder_skips_l1_upward_and_stops_at_l4() {
        assert_eq!(
            AutonomyMode::L0Observe.next_rung(),
            Some(AutonomyMode::L2Recommend),
            "L1 is a labeling mode, not a trust rung"
        );
        assert_eq!(
            AutonomyMode::L2Recommend.next_rung(),
            Some(AutonomyMode::L3Assisted)
        );
        assert_eq!(
            AutonomyMode::L3Assisted.next_rung(),
            Some(AutonomyMode::L4Autonomous)
        );
        assert_eq!(
            AutonomyMode::L4Autonomous.next_rung(),
            None,
            "L5 is never earned — learning stays operator-granted"
        );
        assert_eq!(AutonomyMode::L5Adaptive.next_rung(), None);
    }

    #[test]
    fn demotion_walks_down_one_rung_and_l1_is_skipped() {
        assert_eq!(
            AutonomyMode::L4Autonomous.demote_rung(),
            AutonomyMode::L3Assisted
        );
        assert_eq!(
            AutonomyMode::L3Assisted.demote_rung(),
            AutonomyMode::L2Recommend
        );
        assert_eq!(
            AutonomyMode::L2Recommend.demote_rung(),
            AutonomyMode::L0Observe,
            "the demotion ladder skips L1"
        );
        assert_eq!(
            AutonomyMode::L0Observe.demote_rung(),
            AutonomyMode::L0Observe,
            "demotion at L0 is a no-op"
        );
        assert_eq!(
            AutonomyMode::L5Adaptive.demote_rung(),
            AutonomyMode::L3Assisted
        );
    }

    #[test]
    fn phase_names_are_stable() {
        assert_eq!(AssimilationPhase::default(), AssimilationPhase::Mapping);
        for (phase, name) in [
            (AssimilationPhase::Mapping, "mapping"),
            (AssimilationPhase::Shadow, "shadow"),
            (AssimilationPhase::Earned, "earned"),
        ] {
            assert_eq!(phase.canonical_name(), name);
            let json = serde_json::to_string(&phase).unwrap();
            assert_eq!(json, format!("\"{name}\""));
            let back: AssimilationPhase = serde_json::from_str(&json).unwrap();
            assert_eq!(back, phase);
        }
    }

    #[test]
    fn autonomy_state_round_trips_with_its_environment_key() {
        let state = AutonomyState::fresh(EnvironmentId::new(), Utc::now());
        let json = serde_json::to_string(&state).unwrap();
        let back: AutonomyState = serde_json::from_str(&json).unwrap();
        assert_eq!(back, state);
        assert_eq!(back.phase, AssimilationPhase::Mapping);
        assert_eq!(back.earned, AutonomyMode::L0Observe);
        assert_eq!(back.budget, BudgetWindows::default());
    }
}
