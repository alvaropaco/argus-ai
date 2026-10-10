//! Assimilation and graduated autonomy (spec 008, ADR-0040): the persisted
//! state machine that earns, bounds, and revokes the daemon's authority.
//!
//! The managed `brain.autonomy` is the operator's **ceiling** — the most the
//! operator is willing to grant, never a command to act at that level. This
//! module owns the **earned rung**: promoted rung-by-rung (L2 → L3 → L4)
//! through clean cycles, demoted automatically on failure signatures
//! (SafeMode → L0; failed-validation-with-rollback or provider degradation →
//! −1 rung), and bounded per period by a blast-radius budget. Every consumer
//! reads [`AutonomyManager::effective`] — `min(ceiling, earned)` — never
//! either number alone.
//!
//! One computation point: this module owns the state and the evaluator and
//! exposes `effective()`, `on_cycle(...)`, and the [`BudgetGate`] port;
//! `brain.rs` ticks it and `control.rs` consumes the gate — never recompute.
//!
//! Fail-closed by construction (ADR-0040 §4): a corrupted or unreadable store
//! re-assimilates from `mapping` at L0; the evaluator is pure, so a budget
//! *check* has no error path — store failures affect persistence only, and
//! authority never rises by accident. The production default is unchanged:
//! absent an operator-set ceiling, `min(L0, …)` keeps the daemon at L0.

use std::sync::Arc;

use argus_domain::{
    AssimilationPhase, AutonomyMode, AutonomyState, BlastRadius, BrainTraceRecord, DomainEvent,
    EnvironmentId, EventType, RiskClass, Severity,
};
use argus_events::{EventBus, LedgerSink, types};
use argus_policy::budget;
use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::config::AutonomyConfig;
use crate::ledger::DaemonLedger;

/// The per-tick gate signals (spec 008 FR-002). Every one is a number the
/// daemon already computes — provider health, cloud connectivity, the
/// self-observability safe mode, the sentinel's incident count, and the
/// cycle's run outcome — no new sensing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CycleSignals {
    /// The operator's ceiling this tick (`brain.autonomy` as the loop read it).
    pub ceiling: AutonomyMode,
    /// The reasoning provider is configured and its last step succeeded.
    pub provider_ready: bool,
    /// The cloud pairing is alive (connected, even if degraded).
    pub cloud_paired: bool,
    /// SafeMode is active — stale inputs or suspended execution.
    pub safe_mode_active: bool,
    /// Unresolved critical incidents open.
    pub open_critical_incidents: u32,
    /// The tick consulted live environment state (the cycle's live read ran).
    pub live_environment: bool,
    /// This tick's run terminated in a failed validation with rollback.
    pub failed_validation: bool,
}

impl CycleSignals {
    /// The gate evidence one clean cycle must show (FR-002): provider
    /// healthy, cloud paired, no SafeMode, no unresolved critical incident,
    /// no failed validation.
    pub fn clean(&self) -> bool {
        self.provider_ready
            && self.cloud_paired
            && !self.safe_mode_active
            && self.open_critical_incidents == 0
            && !self.failed_validation
    }
}

/// The blast-radius budget port (spec 008 FR-004): what the plan loop
/// consults in the Allow branch, before the autonomy matrix. Exhaustion is
/// a pause — the caller parks the plan on the ordinary approval machinery
/// (`budget.exhausted`) — never a denial and never an execution.
#[async_trait::async_trait]
pub trait BudgetGate: Send + Sync {
    /// Atomically reserves one unit for a step of this risk and scope:
    /// rollover, room-check, and consumption under one lock, so concurrent
    /// plans can never overshoot the cap. `false` = exhausted; nothing was
    /// consumed.
    async fn reserve(&self, risk: RiskClass, scope: BlastRadius, now: DateTime<Utc>) -> bool;

    /// Surrenders a reservation when the step turned out not to execute:
    /// an already-desired no-op (zero blast radius), or a pause for
    /// approval/escalation after the reserve. A failed-then-rolled-back
    /// step is not refunded — the blast radius was touched.
    async fn refund(&self, risk: RiskClass, scope: BlastRadius, now: DateTime<Utc>);
}

/// The stand-in gate for callers that run no autonomy machine (tests, the
/// noop ports): every step passes, nothing is consumed.
#[derive(Debug, Default)]
pub struct NoopBudget;

#[async_trait::async_trait]
impl BudgetGate for NoopBudget {
    async fn reserve(&self, _risk: RiskClass, _scope: BlastRadius, _now: DateTime<Utc>) -> bool {
        true
    }

    async fn refund(&self, _risk: RiskClass, _scope: BlastRadius, _now: DateTime<Utc>) {}
}

/// The assimilation state machine: one instance per daemon, persisted through
/// the local repository so phase, rung, and counters survive a restart
/// (spec 008 FR-001, AC-007).
pub struct AutonomyManager {
    environment_id: EnvironmentId,
    config: AutonomyConfig,
    repository: Arc<dyn argus_state::DomainRepository>,
    ledger: Arc<DaemonLedger>,
    state: tokio::sync::Mutex<AutonomyState>,
}

impl AutonomyManager {
    /// Loads the persisted state, or starts fresh at `mapping`/L0 when none
    /// exists, it belongs to another environment, or the store is unreadable —
    /// the fail-closed readings (spec 008 FR-001, ADR-0040 §4). A fresh start
    /// is persisted immediately: the state exists from the first moment.
    pub async fn load(
        environment_id: EnvironmentId,
        config: AutonomyConfig,
        repository: Arc<dyn argus_state::DomainRepository>,
        ledger: Arc<DaemonLedger>,
    ) -> Arc<Self> {
        let (state, fresh) = match repository.get_autonomy_state().await {
            Ok(Some(persisted)) if persisted.environment_id == environment_id => (persisted, false),
            Ok(Some(foreign)) => {
                tracing::info!(
                    environment_id = %environment_id.as_uuid(),
                    found = %foreign.environment_id.as_uuid(),
                    "persisted autonomy state belongs to another environment; \
                     re-assimilating from mapping"
                );
                (AutonomyState::fresh(environment_id, Utc::now()), true)
            }
            Ok(None) => (AutonomyState::fresh(environment_id, Utc::now()), true),
            Err(error) => {
                // Fail-closed: an unreadable store means unproven behavior.
                tracing::warn!(
                    %error,
                    "autonomy state unreadable; re-assimilating from mapping at L0"
                );
                (AutonomyState::fresh(environment_id, Utc::now()), true)
            }
        };
        let machine = Self {
            environment_id,
            config,
            repository,
            ledger,
            state: tokio::sync::Mutex::new(state.clone()),
        };
        if fresh {
            machine.persist(&state).await;
        }
        Arc::new(machine)
    }

    /// The effective autonomy for a run at `ceiling`: `min(ceiling, earned)`.
    /// This is the one computation every consumer reads — the ceiling binds,
    /// and a lowered ceiling drops the effective level immediately while the
    /// earned rung is preserved (spec 008 FR-003, AC-005).
    pub async fn effective(&self, ceiling: AutonomyMode) -> AutonomyMode {
        let state = self.state.lock().await;
        ceiling.lower_of(state.earned)
    }

    /// A snapshot of the raw state (phase, rung, counters, budgets).
    pub async fn snapshot(&self) -> AutonomyState {
        self.state.lock().await.clone()
    }

    /// The environment the machine assimilates into.
    pub fn environment_id(&self) -> EnvironmentId {
        self.environment_id
    }

    /// One brain tick of the state machine (FR-001/FR-002). Revocation runs
    /// before earning: a failure signature lowers the rung first, and an
    /// unclean cycle is not gate evidence — the counters restart while the
    /// phase holds (missing evidence never errors; it waits).
    pub async fn on_cycle(&self, signals: CycleSignals) {
        let mut state = self.state.lock().await;
        let mut changed = false;

        // Revocation first (FR-003): SafeMode drops the rung to L0 outright;
        // a degraded provider walks it down one rung per degraded tick. Both
        // re-earn through the ordinary gates.
        if signals.safe_mode_active && state.earned.rank() > AutonomyMode::L0Observe.rank() {
            self.transition(&mut state, AutonomyMode::L0Observe, "safe_mode")
                .await;
            state.rung_clean_cycles = 0;
            changed = true;
        } else if !signals.provider_ready && state.earned.rank() > AutonomyMode::L0Observe.rank() {
            let demoted = state.earned.demote_rung();
            self.transition(&mut state, demoted, "provider_degraded")
                .await;
            state.rung_clean_cycles = 0;
            changed = true;
        }

        if signals.clean() {
            match state.phase {
                // MAP → SHADOW: the live environment graph has been observed
                // and provider readiness is known (the caller supplies both
                // as computed signals).
                AssimilationPhase::Mapping if signals.live_environment => {
                    state.phase = AssimilationPhase::Shadow;
                    state.shadow_clean_cycles = 0;
                    state.updated_at = Utc::now();
                    changed = true;
                    tracing::info!(
                        environment_id = %self.environment_id.as_uuid(),
                        "assimilation: mapping complete, the shadow window opens"
                    );
                }
                AssimilationPhase::Shadow => {
                    state.shadow_clean_cycles = state.shadow_clean_cycles.saturating_add(1);
                    if state.shadow_clean_cycles >= self.config.shadow_min_cycles {
                        // SHADOW → EARNED at the first trust rung (L2). The
                        // effective level stays capped by the ceiling.
                        state.phase = AssimilationPhase::Earned;
                        state.shadow_clean_cycles = 0;
                        state.rung_clean_cycles = 0;
                        self.transition(&mut state, AutonomyMode::L2Recommend, "shadow_window")
                            .await;
                    }
                    changed = true;
                }
                AssimilationPhase::Earned => {
                    state.rung_clean_cycles = state.rung_clean_cycles.saturating_add(1);
                    if let Some(next) = state.earned.next_rung() {
                        // A promotion needs both kinds of evidence: the
                        // rung's clean-cycle streak AND at least one
                        // validated (Completed) execution since the last
                        // rung change — a quiet paired host proves
                        // reliability, never competence (ADR-0040: time
                        // alone promotes nothing). Below the ceiling the
                        // gate keeps accumulating; the effective level is
                        // capped by the ceiling anyway.
                        if state.validated_actions_since_rung >= 1
                            && state.rung_clean_cycles >= self.config.rung_clean_cycles
                            && next.rank() <= signals.ceiling.rank()
                        {
                            state.rung_clean_cycles = 0;
                            self.transition(&mut state, next, "clean_cycles").await;
                        }
                    }
                    changed = true;
                }
                // Unreachable while `clean` (mapping requires live evidence),
                // but the arm keeps the match exhaustive.
                AssimilationPhase::Mapping => {}
            }
        } else {
            // An unclean cycle is not evidence: the counters restart. The
            // phase holds — missing evidence keeps counting from zero.
            if state.shadow_clean_cycles != 0 {
                state.shadow_clean_cycles = 0;
                changed = true;
            }
            if state.rung_clean_cycles != 0 {
                state.rung_clean_cycles = 0;
                changed = true;
            }
        }

        if changed {
            self.persist(&state).await;
        }
    }

    /// The plan-terminal demotion hook (FR-003, AC-003): a failed validation
    /// with rollback costs one rung. At L0 it is a no-op — the demotion never
    /// walks below the floor, and nothing is emitted for it.
    pub async fn on_validation_failed(&self) {
        let mut state = self.state.lock().await;
        if state.earned.rank() > AutonomyMode::L0Observe.rank() {
            let demoted = state.earned.demote_rung();
            self.transition(&mut state, demoted, "failed_validation")
                .await;
            state.rung_clean_cycles = 0;
            self.persist(&state).await;
        }
    }

    /// The plan-terminal evidence feed (spec 008 FR-002): `count`
    /// executions completed and validated in one finished run. Rung
    /// promotions require at least one such execution since the last rung
    /// change, so idle observation alone never climbs the ladder.
    pub async fn record_validated_executions(&self, count: u32) {
        if count == 0 {
            return;
        }
        let mut state = self.state.lock().await;
        state.validated_actions_since_rung =
            state.validated_actions_since_rung.saturating_add(count);
        state.updated_at = Utc::now();
        self.persist(&state).await;
    }

    /// The sentinel view's `autonomy` map (FR-005): the assimilation phase
    /// with gate progress, the earned rung, the ceiling, the effective level,
    /// and the remaining budgets. Additive fields only — the cloud validates
    /// the view loosely.
    pub async fn view(&self, ceiling: AutonomyMode) -> Map<String, Value> {
        let state = self.state.lock().await;
        let effective = ceiling.lower_of(state.earned);
        let (cycles, required) = match state.phase {
            AssimilationPhase::Mapping => (0, 0),
            AssimilationPhase::Shadow => (state.shadow_clean_cycles, self.config.shadow_min_cycles),
            AssimilationPhase::Earned => (state.rung_clean_cycles, self.config.rung_clean_cycles),
        };
        let budgets: Map<String, Value> =
            budget::remaining(&state.budget, &self.config.budgets, Utc::now())
                .into_iter()
                .map(|(name, remaining, limit)| {
                    (
                        name.to_string(),
                        json!({ "remaining": remaining, "limit": limit }),
                    )
                })
                .collect();
        json!({
            "phase": state.phase.canonical_name(),
            "phase_progress": { "cycles": cycles, "required": required },
            "earned": state.earned.canonical_name(),
            "ceiling": ceiling.canonical_name(),
            "effective": effective.canonical_name(),
            "budgets": budgets,
        })
        .as_object()
        .cloned()
        .unwrap_or_default()
    }

    /// Moves the earned rung and records the promotion/demotion as both a
    /// ledger trace and a domain event (FR-003). A same-rung call is a no-op.
    async fn transition(&self, state: &mut AutonomyState, to: AutonomyMode, reason: &str) {
        let from = state.earned;
        if from == to {
            return;
        }
        let promoted = to.rank() > from.rank();
        let outcome = if promoted { "promoted" } else { "demoted" };
        state.earned = to;
        // Every rung change restarts the evidence: the new rung must prove
        // itself with its own validated work.
        state.validated_actions_since_rung = 0;
        state.updated_at = Utc::now();

        // Ledger trace: an auditable row the operator can find (FR-003).
        self.ledger
            .record_trace(BrainTraceRecord {
                trace_id: Uuid::new_v4(),
                cycle_id: None,
                plan_id: None,
                evidence: vec![format!(
                    "autonomy rung: {} -> {} ({})",
                    from.canonical_name(),
                    to.canonical_name(),
                    reason
                )],
                decision: None,
                objective: Some(format!("autonomy rung {outcome}")),
                steps: Vec::new(),
                outcome: Some(outcome.to_string()),
                occurred_at: Utc::now(),
            })
            .await;

        // Domain event: additive vocabulary on the ledger's local bus — no
        // new wire kind, no protocol change.
        let event_type = if promoted {
            types::AUTONOMY_PROMOTED
        } else {
            types::AUTONOMY_DEMOTED
        };
        let _ = self
            .ledger
            .events()
            .publish(&DomainEvent::new(
                Uuid::new_v4(),
                EventType::new(event_type).expect("autonomy event types are valid"),
                Utc::now(),
                "argusd",
                "argusd",
                if promoted {
                    Severity::Info
                } else {
                    Severity::Warning
                },
                None,
                None,
                json!({
                    "from": from.canonical_name(),
                    "to": to.canonical_name(),
                    "reason": reason,
                    "phase": state.phase.canonical_name(),
                }),
            ))
            .await;
        tracing::info!(
            from = from.canonical_name(),
            to = to.canonical_name(),
            reason = reason,
            "autonomy rung {outcome}"
        );
    }

    /// Persists the state; a failure is logged, never fatal — the in-memory
    /// machine keeps degrading downward, and a restart re-assimilates.
    async fn persist(&self, state: &AutonomyState) {
        if let Err(error) = self.repository.put_autonomy_state(state).await {
            tracing::warn!(%error, "autonomy state not persisted");
        }
    }
}

#[async_trait::async_trait]
impl BudgetGate for AutonomyManager {
    async fn reserve(&self, risk: RiskClass, scope: BlastRadius, now: DateTime<Utc>) -> bool {
        // One lock around rollover + room-check + consumption: the reserve
        // is atomic, so concurrent plans cannot overshoot the cap.
        let mut state = self.state.lock().await;
        let reserved = budget::reserve(&mut state.budget, &self.config.budgets, risk, scope, now);
        if reserved {
            state.updated_at = now;
            self.persist(&state).await;
        }
        reserved
    }

    async fn refund(&self, risk: RiskClass, scope: BlastRadius, now: DateTime<Utc>) {
        let mut state = self.state.lock().await;
        budget::refund(&mut state.budget, risk, scope, now);
        state.updated_at = now;
        self.persist(&state).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use argus_domain::AssimilationPhase as Phase;
    use argus_state::{DomainRepository, InMemoryRepository, RepositoryError};

    fn config(shadow: u32, rung: u32) -> AutonomyConfig {
        AutonomyConfig {
            shadow_min_cycles: shadow,
            rung_clean_cycles: rung,
            budgets: argus_policy::BudgetLimits {
                low_risk_per_hour: 2,
                controlled_per_day: 5,
                host_scope_per_day: 3,
            },
        }
    }

    /// Clean-cycle signals at `ceiling`: provider healthy, cloud paired, no
    /// SafeMode, no incidents, a live environment, no failed validation.
    fn clean(ceiling: AutonomyMode) -> CycleSignals {
        CycleSignals {
            ceiling,
            provider_ready: true,
            cloud_paired: true,
            safe_mode_active: false,
            open_critical_incidents: 0,
            live_environment: true,
            failed_validation: false,
        }
    }

    async fn manager(
        repo: Arc<dyn DomainRepository>,
    ) -> (
        Arc<AutonomyManager>,
        tokio::sync::broadcast::Receiver<DomainEvent>,
    ) {
        let ledger = DaemonLedger::new(Arc::clone(&repo) as Arc<dyn DomainRepository>, false, 30);
        // Subscribing before any tick means the broadcast buffer already
        // holds every promotion/demotion event when the test drains it —
        // no scheduling race.
        let events_rx = ledger.events().subscribe();
        let machine = AutonomyManager::load(EnvironmentId::new(), config(2, 2), repo, ledger).await;
        (machine, events_rx)
    }

    fn event_names(rx: &mut tokio::sync::broadcast::Receiver<DomainEvent>) -> Vec<String> {
        use tokio::sync::broadcast::error::TryRecvError;
        let mut names = Vec::new();
        loop {
            match rx.try_recv() {
                Ok(event) => {
                    let name = event.event_type().as_str().to_string();
                    // The bus also carries the traces' own `brain.trace`
                    // events; the autonomy vocabulary is what these tests
                    // observe.
                    if name.starts_with("autonomy.") {
                        names.push(name);
                    }
                }
                Err(TryRecvError::Empty) | Err(TryRecvError::Closed) => break,
                Err(TryRecvError::Lagged(_)) => continue,
            }
        }
        names
    }

    #[tokio::test]
    async fn a_fresh_environment_starts_mapping_at_l0_and_persists() {
        let repo: Arc<dyn DomainRepository> = Arc::new(InMemoryRepository::new());
        let (machine, _rx) = manager(Arc::clone(&repo)).await;

        let state = machine.snapshot().await;
        assert_eq!(state.phase, Phase::Mapping);
        assert_eq!(state.earned, AutonomyMode::L0Observe);
        assert_eq!(
            machine.effective(AutonomyMode::L0Observe).await,
            AutonomyMode::L0Observe,
            "absent a ceiling the daemon stays L0 (production default unchanged)"
        );
        assert_eq!(
            machine.effective(AutonomyMode::L4Autonomous).await,
            AutonomyMode::L0Observe,
            "nothing is earned yet: min(ceiling, L0) is L0"
        );

        let persisted = repo.get_autonomy_state().await.unwrap().unwrap();
        assert_eq!(persisted.environment_id, machine.environment_id());
        assert_eq!(persisted.phase, Phase::Mapping);
    }

    #[tokio::test]
    async fn the_shadow_gate_promotes_to_earned_l2_with_trace_and_event() {
        let repo: Arc<dyn DomainRepository> = Arc::new(InMemoryRepository::new());
        let (machine, mut rx) = manager(Arc::clone(&repo)).await;
        let ceiling = AutonomyMode::L2Recommend;

        // The first clean cycle completes the MAP gate: the shadow window
        // opens with its counter at zero — the mapping cycle is not itself
        // shadow evidence.
        machine.on_cycle(clean(ceiling)).await;
        let state = machine.snapshot().await;
        assert_eq!(state.phase, Phase::Shadow);
        assert_eq!(state.shadow_clean_cycles, 0);
        assert!(
            event_names(&mut rx).is_empty(),
            "no rung event before the gate passes"
        );

        // One clean shadow cycle: inside the window (needs 2).
        machine.on_cycle(clean(ceiling)).await;
        let state = machine.snapshot().await;
        assert_eq!(state.shadow_clean_cycles, 1);
        assert!(event_names(&mut rx).is_empty());

        // The gate passes: the phase turns earned at L2.
        machine.on_cycle(clean(ceiling)).await;
        let state = machine.snapshot().await;
        assert_eq!(state.phase, Phase::Earned);
        assert_eq!(state.earned, AutonomyMode::L2Recommend);
        assert_eq!(machine.effective(ceiling).await, AutonomyMode::L2Recommend);

        // Promotion emitted `autonomy.promoted` AND a promoted ledger trace.
        let events = event_names(&mut rx);
        assert_eq!(events, vec!["autonomy.promoted".to_string()]);
        let traces = repo.list_brain_traces(None, 10).await.unwrap();
        assert!(
            traces
                .iter()
                .any(|t| t.outcome.as_deref() == Some("promoted"))
        );
    }

    #[tokio::test]
    async fn rung_promotion_needs_the_rung_gate_and_a_ceiling_that_admits_it() {
        let repo: Arc<dyn DomainRepository> = Arc::new(InMemoryRepository::new());
        let (machine, mut rx) = manager(Arc::clone(&repo)).await;
        let ceiling = AutonomyMode::L3Assisted;

        // Tick 1 opens the shadow window; ticks 2–3 fill it (needs 2) and
        // the phase turns earned at L2. The shadow gate is behavior-based:
        // no executed work is required to be trusted with L2, where nothing
        // auto-executes anyway.
        machine.on_cycle(clean(ceiling)).await;
        machine.on_cycle(clean(ceiling)).await;
        machine.on_cycle(clean(ceiling)).await;
        assert_eq!(machine.snapshot().await.earned, AutonomyMode::L2Recommend);

        // Two clean cycles at L2 satisfy the rung gate — but with no
        // validated (Completed) execution since the rung change, idle
        // observation alone never climbs.
        machine.on_cycle(clean(ceiling)).await;
        machine.on_cycle(clean(ceiling)).await;
        let state = machine.snapshot().await;
        assert_eq!(state.earned, AutonomyMode::L2Recommend);
        assert!(state.rung_clean_cycles >= 2, "the cycle gate is satisfied");
        assert_eq!(state.validated_actions_since_rung, 0);

        // Validated work lands, and the next clean cycle promotes: L2 → L3
        // with a promoted trace. Two events total — the shadow promotion
        // and this one, one each.
        machine.record_validated_executions(1).await;
        machine.on_cycle(clean(ceiling)).await;
        assert_eq!(machine.snapshot().await.earned, AutonomyMode::L3Assisted);
        assert_eq!(
            event_names(&mut rx),
            vec![
                "autonomy.promoted".to_string(),
                "autonomy.promoted".to_string()
            ],
        );

        // At L3 with the ceiling at L3: the L4 gate may satisfy, but the
        // ceiling below the next rung holds the rung while it accumulates.
        machine.record_validated_executions(1).await;
        machine.on_cycle(clean(ceiling)).await;
        machine.on_cycle(clean(ceiling)).await;
        let state = machine.snapshot().await;
        assert_eq!(state.earned, AutonomyMode::L3Assisted);
        assert!(
            state.rung_clean_cycles >= 2,
            "the gate accumulates beneath the ceiling"
        );
        assert_eq!(
            machine.effective(ceiling).await,
            AutonomyMode::L3Assisted,
            "effective is capped by the ceiling"
        );
    }

    #[tokio::test]
    async fn idle_cycles_never_promote_without_validated_work() {
        let repo: Arc<dyn DomainRepository> = Arc::new(InMemoryRepository::new());
        let (machine, _rx) = manager(Arc::clone(&repo)).await;
        let ceiling = AutonomyMode::L4Autonomous;

        // Earned at L2 via the shadow window…
        for _ in 0..3 {
            machine.on_cycle(clean(ceiling)).await;
        }
        assert_eq!(machine.snapshot().await.earned, AutonomyMode::L2Recommend);

        // …then far more clean idle cycles than any rung gate requires:
        // without validated work the rung holds, no matter how quiet and
        // clean the host stays (ADR-0040: a quiet host proves nothing).
        for _ in 0..10 {
            machine.on_cycle(clean(ceiling)).await;
        }
        let state = machine.snapshot().await;
        assert_eq!(state.earned, AutonomyMode::L2Recommend);
        assert_eq!(state.validated_actions_since_rung, 0);

        // One validated execution unblocks the climb at the next gate.
        machine.record_validated_executions(1).await;
        machine.on_cycle(clean(ceiling)).await;
        assert_eq!(machine.snapshot().await.earned, AutonomyMode::L3Assisted);
    }

    #[tokio::test]
    async fn the_ladder_never_reaches_l5_or_l1() {
        let repo: Arc<dyn DomainRepository> = Arc::new(InMemoryRepository::new());
        let (machine, _rx) = manager(Arc::clone(&repo)).await;
        let ceiling = AutonomyMode::L5Adaptive;

        // Tick 1 opens the window; ticks 2–3 earn L2; validated work then
        // lets 4–5 earn L3 and 6–7 earn L4 — the top of the ladder under
        // even an L5 ceiling. Every rung proves itself with its own work.
        for _ in 0..3 {
            machine.on_cycle(clean(ceiling)).await;
        }
        machine.record_validated_executions(1).await;
        for _ in 0..2 {
            machine.on_cycle(clean(ceiling)).await;
        }
        machine.record_validated_executions(1).await;
        for _ in 0..2 {
            machine.on_cycle(clean(ceiling)).await;
        }

        let state = machine.snapshot().await;
        assert_eq!(state.earned, AutonomyMode::L4Autonomous);
        assert_eq!(
            machine.effective(ceiling).await,
            AutonomyMode::L4Autonomous,
            "L5 stays operator-granted even under an L5 ceiling"
        );

        // Many more clean cycles: the rung counter accumulates but the rung
        // holds at L4; L1 is never visited on any path.
        for _ in 0..5 {
            machine.on_cycle(clean(ceiling)).await;
        }
        assert_eq!(machine.snapshot().await.earned, AutonomyMode::L4Autonomous);
    }

    #[tokio::test]
    async fn a_failed_validation_demotes_one_rung_with_trace_and_event() {
        let repo: Arc<dyn DomainRepository> = Arc::new(InMemoryRepository::new());
        let (machine, mut rx) = manager(Arc::clone(&repo)).await;
        let ceiling = AutonomyMode::L4Autonomous;

        for _ in 0..3 {
            machine.on_cycle(clean(ceiling)).await; // earned L2
        }
        machine.record_validated_executions(1).await;
        for _ in 0..2 {
            machine.on_cycle(clean(ceiling)).await; // L3
        }
        // Drain the two promotions so the demotion is the only event left.
        assert_eq!(
            event_names(&mut rx),
            vec![
                "autonomy.promoted".to_string(),
                "autonomy.promoted".to_string()
            ],
        );

        // A failed validation with rollback at L3: −1 rung, demoted event.
        machine.on_validation_failed().await;
        let state = machine.snapshot().await;
        assert_eq!(state.earned, AutonomyMode::L2Recommend);
        assert_eq!(state.rung_clean_cycles, 0, "the gate restarts");
        assert_eq!(
            state.validated_actions_since_rung, 0,
            "the new rung starts with no validated evidence of its own"
        );
        assert_eq!(event_names(&mut rx), vec!["autonomy.demoted".to_string()]);
        let traces = repo.list_brain_traces(None, 10).await.unwrap();
        assert!(
            traces
                .iter()
                .any(|t| t.outcome.as_deref() == Some("demoted"))
        );

        // Re-earning re-runs the rung gates from L2 (the phase stays
        // earned): validated work, then two clean cycles climb back to L3.
        machine.record_validated_executions(1).await;
        machine.on_cycle(clean(ceiling)).await;
        machine.on_cycle(clean(ceiling)).await;
        assert_eq!(machine.snapshot().await.earned, AutonomyMode::L3Assisted);
    }

    #[tokio::test]
    async fn a_failed_validation_at_l0_is_a_no_op() {
        let repo: Arc<dyn DomainRepository> = Arc::new(InMemoryRepository::new());
        let (machine, mut rx) = manager(Arc::clone(&repo)).await;

        machine.on_validation_failed().await;
        assert_eq!(machine.snapshot().await.earned, AutonomyMode::L0Observe);
        assert!(
            event_names(&mut rx).is_empty(),
            "nothing is emitted for a demotion at the floor"
        );
        assert!(
            repo.list_brain_traces(None, 10).await.unwrap().is_empty(),
            "and no trace either"
        );
    }

    #[tokio::test]
    async fn safe_mode_drops_the_rung_to_l0_immediately() {
        let repo: Arc<dyn DomainRepository> = Arc::new(InMemoryRepository::new());
        let (machine, mut rx) = manager(Arc::clone(&repo)).await;
        let ceiling = AutonomyMode::L4Autonomous;

        for _ in 0..3 {
            machine.on_cycle(clean(ceiling)).await; // earned L2
        }
        machine.record_validated_executions(1).await;
        for _ in 0..2 {
            machine.on_cycle(clean(ceiling)).await; // L3
        }
        // Drain the promotions so the demotion is the only event left.
        assert_eq!(event_names(&mut rx).len(), 2);

        // SafeMode entry: the rung goes to L0 outright, not one rung down.
        let mut signals = clean(ceiling);
        signals.safe_mode_active = true;
        machine.on_cycle(signals).await;

        let state = machine.snapshot().await;
        assert_eq!(state.earned, AutonomyMode::L0Observe);
        assert_eq!(machine.effective(ceiling).await, AutonomyMode::L0Observe);
        assert_eq!(event_names(&mut rx), vec!["autonomy.demoted".to_string()]);

        // Re-earning re-runs the gates from the floor: validated work, then
        // two clean cycles climb back to L2.
        machine.record_validated_executions(1).await;
        machine.on_cycle(clean(ceiling)).await;
        machine.on_cycle(clean(ceiling)).await;
        assert_eq!(machine.snapshot().await.earned, AutonomyMode::L2Recommend);
    }

    #[tokio::test]
    async fn provider_degradation_demotes_one_rung() {
        let repo: Arc<dyn DomainRepository> = Arc::new(InMemoryRepository::new());
        let (machine, _rx) = manager(Arc::clone(&repo)).await;
        let ceiling = AutonomyMode::L4Autonomous;

        for _ in 0..3 {
            machine.on_cycle(clean(ceiling)).await; // earned L2
        }
        machine.record_validated_executions(1).await;
        for _ in 0..2 {
            machine.on_cycle(clean(ceiling)).await; // L3
        }

        let mut signals = clean(ceiling);
        signals.provider_ready = false;
        machine.on_cycle(signals).await;
        assert_eq!(machine.snapshot().await.earned, AutonomyMode::L2Recommend);

        // A sustained degradation walks to the floor; recovery re-earns.
        machine.on_cycle(signals).await;
        machine.on_cycle(signals).await;
        assert_eq!(machine.snapshot().await.earned, AutonomyMode::L0Observe);
    }

    #[tokio::test]
    async fn unclean_cycles_reset_the_gate_counters_and_hold_the_phase() {
        let repo: Arc<dyn DomainRepository> = Arc::new(InMemoryRepository::new());
        let (machine, _rx) = manager(Arc::clone(&repo)).await;
        let ceiling = AutonomyMode::L2Recommend;

        machine.on_cycle(clean(ceiling)).await; // the window opens at 0
        machine.on_cycle(clean(ceiling)).await; // 1/2
        let mut dirty = clean(ceiling);
        dirty.open_critical_incidents = 1;
        machine.on_cycle(dirty).await;
        machine.on_cycle(clean(ceiling)).await;

        let state = machine.snapshot().await;
        assert_eq!(state.phase, Phase::Shadow);
        assert_eq!(
            state.shadow_clean_cycles, 1,
            "an unclean cycle is not gate evidence: the streak restarts"
        );
    }

    #[tokio::test]
    async fn a_lowered_ceiling_drops_effective_without_touching_earned() {
        let repo: Arc<dyn DomainRepository> = Arc::new(InMemoryRepository::new());
        let (machine, _rx) = manager(Arc::clone(&repo)).await;

        for _ in 0..3 {
            machine.on_cycle(clean(AutonomyMode::L3Assisted)).await; // earned L2
        }

        // The ceiling lands lower (AC-005): the effective level drops
        // immediately; the earned rung is preserved and re-caps on a raise.
        assert_eq!(
            machine.effective(AutonomyMode::L2Recommend).await,
            AutonomyMode::L2Recommend
        );
        assert_eq!(
            machine.effective(AutonomyMode::L0Observe).await,
            AutonomyMode::L0Observe,
            "the lowered ceiling binds"
        );
        assert_eq!(
            machine.snapshot().await.earned,
            AutonomyMode::L2Recommend,
            "earned is unchanged by the ceiling"
        );
    }

    #[tokio::test]
    async fn a_restart_resumes_from_the_persisted_state() {
        let repo: Arc<dyn DomainRepository> = Arc::new(InMemoryRepository::new());
        let environment = EnvironmentId::new();
        let ledger = DaemonLedger::new(Arc::clone(&repo), false, 30);
        let first =
            AutonomyManager::load(environment, config(10, 20), Arc::clone(&repo), ledger).await;

        // Mid-assimilation: the window opened plus two clean shadow cycles.
        for _ in 0..3 {
            first.on_cycle(clean(AutonomyMode::L2Recommend)).await;
        }

        // The daemon restarts: a fresh manager resumes the same persisted
        // counters (AC-007).
        let ledger = DaemonLedger::new(Arc::clone(&repo), false, 30);
        let second =
            AutonomyManager::load(environment, config(10, 20), Arc::clone(&repo), ledger).await;
        let state = second.snapshot().await;
        assert_eq!(state.phase, Phase::Shadow);
        assert_eq!(state.shadow_clean_cycles, 2, "the counters resume");

        // The next clean cycle counts as the third, not the first.
        second.on_cycle(clean(AutonomyMode::L2Recommend)).await;
        assert_eq!(second.snapshot().await.shadow_clean_cycles, 3);
    }

    #[tokio::test]
    async fn a_foreign_environment_starts_fresh() {
        let repo: Arc<dyn DomainRepository> = Arc::new(InMemoryRepository::new());
        let environment = EnvironmentId::new();
        let ledger = DaemonLedger::new(Arc::clone(&repo), false, 30);
        let first =
            AutonomyManager::load(environment, config(2, 2), Arc::clone(&repo), ledger).await;
        first.on_cycle(clean(AutonomyMode::L2Recommend)).await;
        assert_eq!(first.snapshot().await.phase, Phase::Shadow);

        // A different environment id reads as a fresh environment: state is
        // per environment, so nothing carries over.
        let ledger = DaemonLedger::new(Arc::clone(&repo), false, 30);
        let second = AutonomyManager::load(
            EnvironmentId::new(),
            config(2, 2),
            Arc::clone(&repo),
            ledger,
        )
        .await;
        let state = second.snapshot().await;
        assert_eq!(state.phase, Phase::Mapping);
        assert_eq!(state.earned, AutonomyMode::L0Observe);
    }

    /// A store whose autonomy reads and writes always fail — the fail-closed
    /// corruption case (spec 008 FR-001, intent matrix "Restart").
    struct FailingStore {
        autonomy_fails: bool,
    }

    #[async_trait::async_trait]
    impl DomainRepository for FailingStore {
        async fn save_environment(&self, _id: &EnvironmentId) -> Result<(), RepositoryError> {
            Ok(())
        }
        async fn get_environment(&self) -> Result<Option<EnvironmentId>, RepositoryError> {
            Ok(None)
        }
        async fn put_observation(
            &self,
            _observation: &argus_domain::Observation,
        ) -> Result<(), RepositoryError> {
            Ok(())
        }
        async fn list_observations(
            &self,
        ) -> Result<Vec<argus_domain::Observation>, RepositoryError> {
            Ok(Vec::new())
        }
        async fn put_audit_event(
            &self,
            _event: &argus_domain::DomainEvent,
        ) -> Result<(), RepositoryError> {
            Ok(())
        }
        async fn save_health(
            &self,
            _health: &argus_domain::HealthStatus,
        ) -> Result<(), RepositoryError> {
            Ok(())
        }
        async fn get_health(&self) -> Result<Option<argus_domain::HealthStatus>, RepositoryError> {
            Ok(None)
        }
        async fn put_autonomy_state(&self, _state: &AutonomyState) -> Result<(), RepositoryError> {
            Err(RepositoryError::Unavailable("store unavailable".into()))
        }
        async fn get_autonomy_state(&self) -> Result<Option<AutonomyState>, RepositoryError> {
            if self.autonomy_fails {
                Err(RepositoryError::Corrupt(
                    "unparseable autonomy state".into(),
                ))
            } else {
                Ok(None)
            }
        }
    }

    #[tokio::test]
    async fn an_unreadable_store_re_assimilates_from_mapping_at_l0() {
        let store: Arc<dyn DomainRepository> = Arc::new(FailingStore {
            autonomy_fails: true,
        });
        let ledger = DaemonLedger::new(Arc::clone(&store), false, 30);
        let machine = AutonomyManager::load(
            EnvironmentId::new(),
            config(2, 2),
            Arc::clone(&store),
            ledger,
        )
        .await;

        let state = machine.snapshot().await;
        assert_eq!(state.phase, Phase::Mapping, "corrupt state reads as fresh");
        assert_eq!(state.earned, AutonomyMode::L0Observe);

        // The machine still runs; persistence failures are logged, never fatal.
        machine.on_cycle(clean(AutonomyMode::L2Recommend)).await;
        assert_eq!(machine.snapshot().await.phase, Phase::Shadow);
    }

    #[tokio::test]
    async fn budget_exhaustion_pauses_and_rolls_with_time() {
        let repo: Arc<dyn DomainRepository> = Arc::new(InMemoryRepository::new());
        let (machine, _rx) = manager(Arc::clone(&repo)).await;
        let now = Utc::now();
        let gate: &dyn BudgetGate = machine.as_ref();

        // Two low-risk executions at host scope fill the test's hourly limit
        // (low_risk_per_hour: 2) and two of the three host-scope slots.
        assert!(
            gate.reserve(RiskClass::LowRisk, BlastRadius::Host, now)
                .await
        );
        assert!(
            gate.reserve(RiskClass::LowRisk, BlastRadius::Host, now)
                .await
        );

        assert!(
            !gate
                .reserve(RiskClass::LowRisk, BlastRadius::Host, now)
                .await,
            "the exhausted low-risk window pauses the next step"
        );

        // A refund returns the unit: the step the reservation was taken for
        // turned out to be an already-desired no-op, and the hour refills
        // without waiting for the window to roll.
        gate.refund(RiskClass::LowRisk, BlastRadius::Host, now)
            .await;
        assert!(
            gate.reserve(RiskClass::LowRisk, BlastRadius::Host, now)
                .await,
            "the refunded unit is usable again"
        );

        // A controlled step at host scope still has its own room (used 0/5)…
        assert!(
            gate.reserve(RiskClass::Controlled, BlastRadius::Host, now)
                .await
        );
        // …but the third host-scope slot is now gone, so the next host-scope
        // step pauses whatever its risk.
        assert!(
            !gate
                .reserve(RiskClass::Controlled, BlastRadius::Host, now)
                .await,
            "the host-scope window bounds any risk at host scope"
        );
        // Off-host the same risk still has room.
        assert!(
            gate.reserve(RiskClass::Controlled, BlastRadius::Environment, now)
                .await
        );

        // The windows roll with time: the next hour frees the low-risk
        // window (anchored UTC hour bucket); the day window needs a day.
        let next_hour = now + chrono::Duration::hours(1);
        assert!(
            gate.reserve(RiskClass::LowRisk, BlastRadius::None, next_hour)
                .await
        );
        assert!(
            !gate
                .reserve(RiskClass::Controlled, BlastRadius::Host, next_hour)
                .await,
            "the daily host-scope window has not rolled yet"
        );
        let next_day = now + chrono::Duration::hours(24);
        assert!(
            gate.reserve(RiskClass::Controlled, BlastRadius::Host, next_day)
                .await
        );

        // The consumed counters persist for a restarted manager (AC-007).
        let ledger = DaemonLedger::new(Arc::clone(&repo), false, 30);
        let restarted = AutonomyManager::load(
            machine.environment_id(),
            config(2, 2),
            Arc::clone(&repo),
            ledger,
        )
        .await;
        let state = restarted.snapshot().await;
        assert!(
            state.budget.low_risk_hour.is_some(),
            "the budget rides the state"
        );
        assert!(state.budget.host_scope_day.is_some());
    }

    #[tokio::test]
    async fn the_view_reports_phase_progress_earned_ceiling_and_budgets() {
        let repo: Arc<dyn DomainRepository> = Arc::new(InMemoryRepository::new());
        let (machine, _rx) = manager(Arc::clone(&repo)).await;
        let ceiling = AutonomyMode::L2Recommend;

        machine.on_cycle(clean(ceiling)).await;
        let view = machine.view(ceiling).await;
        assert_eq!(view["phase"], json!("shadow"));
        assert_eq!(
            view["phase_progress"],
            json!({ "cycles": 0, "required": 2 }),
            "the window just opened: the mapping cycle is not shadow evidence"
        );
        assert_eq!(view["earned"], json!("l0_observe"));
        assert_eq!(view["ceiling"], json!("l2_recommend"));
        assert_eq!(view["effective"], json!("l0_observe"));
        let budgets = view["budgets"].as_object().unwrap();
        assert_eq!(
            budgets["low_risk_per_hour"],
            json!({ "remaining": 2, "limit": 2 })
        );
        assert_eq!(
            budgets["host_scope_per_day"],
            json!({ "remaining": 3, "limit": 3 })
        );

        // After the gate passes the view reads earned, with the rung gate's
        // progress in its turn.
        machine.on_cycle(clean(ceiling)).await;
        machine.on_cycle(clean(ceiling)).await;
        let view = machine.view(ceiling).await;
        assert_eq!(view["phase"], json!("earned"));
        assert_eq!(view["earned"], json!("l2_recommend"));
        assert_eq!(view["effective"], json!("l2_recommend"));
        assert_eq!(
            view["phase_progress"],
            json!({ "cycles": 0, "required": 2 })
        );
    }

    #[tokio::test]
    async fn the_noop_gate_passes_everything() {
        let gate = NoopBudget;
        let now = Utc::now();
        assert!(
            gate.reserve(RiskClass::Destructive, BlastRadius::Fleet, now)
                .await
        );
        gate.refund(RiskClass::Destructive, BlastRadius::Fleet, now)
            .await;
    }

    /// Compile-time shape checks for the signal struct's cleanliness rule.
    #[test]
    fn cleanliness_requires_every_gate_signal() {
        let mut signals = clean(AutonomyMode::L2Recommend);
        assert!(signals.clean());
        for (name, dirty) in [
            (
                "provider",
                CycleSignals {
                    provider_ready: false,
                    ..signals
                },
            ),
            (
                "cloud",
                CycleSignals {
                    cloud_paired: false,
                    ..signals
                },
            ),
            (
                "safe mode",
                CycleSignals {
                    safe_mode_active: true,
                    ..signals
                },
            ),
            (
                "incident",
                CycleSignals {
                    open_critical_incidents: 1,
                    ..signals
                },
            ),
            (
                "validation",
                CycleSignals {
                    failed_validation: true,
                    ..signals
                },
            ),
        ] {
            signals = dirty;
            assert!(!signals.clean(), "an unclean {name} is not gate evidence");
        }
    }

    /// The shared-state shape: `AutonomyState` round-trips through the
    /// repository trait's serialization (the persistence contract the
    /// SQLite and in-memory backends share).
    #[tokio::test]
    async fn the_state_survives_a_serialization_round_trip() {
        let mut state = AutonomyState::fresh(EnvironmentId::new(), Utc::now());
        state.phase = Phase::Earned;
        state.earned = AutonomyMode::L3Assisted;
        state.rung_clean_cycles = 7;
        state.budget.controlled_day = Some((day_anchor(), 2));
        let wire = serde_json::to_string(&state).unwrap();
        let back: AutonomyState = serde_json::from_str(&wire).unwrap();
        assert_eq!(back, state);
    }

    fn day_anchor() -> i64 {
        budget::day_index(Utc::now())
    }
}
