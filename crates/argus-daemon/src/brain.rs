//! The brain: the running observe → reason → act loop (spec 005).
//!
//! The loop turns live state into typed evidence, poses the host-health
//! structured decisions to the configured provider through
//! [`Daemon::diagnose_once`], and executes any returned plan through the
//! ordinary safety boundary (`run_remediation`) at the operator's
//! configured autonomy — L0 by default, so a fresh install observes and
//! explains but executes nothing. Model output is data: it can answer
//! questions, never grant authority.

use std::sync::Arc;

use chrono::Utc;
use tokio::time::Duration;

use argus_ai_core::decision::adapters::{DeepSeekProvider, LayaHttpProvider};
use argus_ai_core::decision::provider::DecisionProvider;
use argus_domain::{BrainTraceRecord, ResourceId, TokenUsageRecord};
use argus_memory::{Episode, EpisodeOutcome, ProcedureRecord, ProcedureStatus};
use uuid::Uuid;

use crate::config::{BrainConfig, DaemonConfig};
use crate::ledger::DaemonLedger;
use crate::runtime::Daemon;
use argus_events::LedgerSink;

/// Builds the daemon's decision provider from the `[model]` config and the
/// secret store (FR-002). Returns `None` — observe-only — when no provider
/// is configured, the provider is unknown, or the credential is unreadable.
/// Failures log a reason without credential material and never fail startup.
pub fn build_provider(
    config: &DaemonConfig,
    secrets: &crate::cloud::CloudSecretStore,
) -> Option<Arc<dyn DecisionProvider>> {
    let model = &config.model;
    match model.provider.as_str() {
        "" | "none" => None,
        "laya" => {
            let base = model
                .base_url
                .clone()
                .unwrap_or_else(|| "http://127.0.0.1:8000".to_string());
            tracing::info!(base_url = %base, "brain provider: laya sidecar");
            Some(Arc::new(LayaHttpProvider::new(base)))
        }
        "deepseek" => {
            let Ok(Some(secret)) = secrets.read_provider_credential() else {
                tracing::warn!(
                    "deepseek provider configured but no provider credential in the secret \
                     store; the brain runs observe-only"
                );
                return None;
            };
            let base = model
                .base_url
                .clone()
                .unwrap_or_else(|| "https://api.deepseek.com".to_string());
            let mut models = vec![model.model.clone()];
            models.extend(model.fallback_models.iter().cloned());
            // A partial [model] section (mid-setup) yields empty names;
            // the provider's default keeps the adapter well-formed.
            models.retain(|m| !m.is_empty());
            if models.is_empty() {
                models.push("deepseek-chat".to_string());
            }
            tracing::info!(base_url = %base, model = %model.model, "brain provider: deepseek");
            Some(Arc::new(DeepSeekProvider::new(
                base,
                secret.expose().to_string(),
                models,
            )))
        }
        other => {
            tracing::warn!(provider = %other, "unknown model provider; the brain runs observe-only");
            None
        }
    }
}

/// What one brain cycle saw and did — the record behind `brain.diagnose`
/// and the sentinel's brain fields.
#[derive(Debug, Clone, Default)]
pub struct BrainCycle {
    /// The cycle's id (spec 007 FR-002).
    pub cycle_id: Option<Uuid>,
    /// Whether the cycle consulted live environment state — the systemd read
    /// succeeded (spec 008's honest MAP-gate signal: a quiet-but-healthy host
    /// consulted; an unavailable bus did not).
    pub consulted_live: bool,
    /// The evidence keys the cycle gathered (e.g. `unit:<name> state=failed`).
    pub evidence: Vec<String>,
    /// Whether a provider was available to reason with.
    pub provider_available: bool,
    /// The provider's decision summary, when one ran.
    pub decision: Option<String>,
    /// The plan the brain proposed, when the decision passed the gates.
    pub plan_objective: Option<String>,
    /// The plan's step capabilities, so the cycle trace carries the step list
    /// (FR-002) without re-reading the plan.
    pub steps: Vec<String>,
    /// The plan's terminal status, when it ran.
    pub outcome: Option<String>,
    /// Whether the cycle's plan terminated in a validation failure — the
    /// typed revocation signature (spec 008 FR-003), decided at the run site
    /// via `argus_domain::is_validation_failure` so this signal cannot drift
    /// from the runtime's plan-terminal hook.
    pub validation_failed: bool,
}

/// Wraps the configured provider so every call is metered (spec 007 FR-003):
/// the response's `usage` becomes a [`TokenUsageRecord`] correlated to the
/// open cycle, the model, and the measured duration. A response without a
/// usage block is recorded as unknown — never zero, never estimated.
struct MeteredProvider {
    inner: Arc<dyn DecisionProvider>,
    ledger: Arc<DaemonLedger>,
}

impl MeteredProvider {
    fn new(inner: Arc<dyn DecisionProvider>, ledger: Arc<DaemonLedger>) -> Arc<Self> {
        Arc::new(Self { inner, ledger })
    }
}

#[async_trait::async_trait]
impl DecisionProvider for MeteredProvider {
    async fn decide(
        &self,
        request: argus_ai_core::decision::types::DecisionRequest,
    ) -> Result<
        argus_ai_core::decision::types::DecisionResponse,
        argus_ai_core::decision::error::DecisionError,
    > {
        let started = std::time::Instant::now();
        let result = self.inner.decide(request).await;
        if let Ok(response) = &result {
            let usage = response.usage;
            let record = TokenUsageRecord {
                cycle_id: None, // stamped by the ledger's open cycle context
                model: response.model.clone(),
                prompt_tokens: usage.and_then(|u| u.prompt_tokens),
                completion_tokens: usage.and_then(|u| u.completion_tokens),
                total_tokens: usage.and_then(|u| u.total_tokens),
                duration_ms: Some(started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)),
                occurred_at: Utc::now(),
            };
            self.ledger.record_usage(record).await;
        }
        result
    }
}

/// Runs one full brain cycle (FR-003) and records its memory. Used by both
/// the periodic loop and the `brain.diagnose` IPC trigger.
pub async fn cycle(daemon: &Daemon, brain: &BrainConfig) -> BrainCycle {
    // Evidence from live state: the first failed systemd unit is the
    // cycle's subject (the host-health skill reasons about one service).
    let (subjects, consulted_live) = failed_units().await;
    let subjects: Vec<(String, String)> = subjects.into_iter().take(1).collect();
    cycle_with_evidence(daemon, brain, subjects, consulted_live).await
}

/// The cycle over injected subjects — the seam tests use so a brain cycle
/// can be exercised without a system bus.
pub async fn cycle_with_evidence(
    daemon: &Daemon,
    brain: &BrainConfig,
    subjects: Vec<(String, String)>,
    consulted_live: bool,
) -> BrainCycle {
    // Spec 007 FR-002: every cycle carries an id, minted at entry, that its
    // trace, usage records, and action events share.
    let cycle_id = Uuid::new_v4();
    let ledger = daemon.ledger();
    ledger.enter_cycle(cycle_id).await;
    let record = run_cycle(daemon, brain, subjects, cycle_id, consulted_live).await;
    let plan_ids = ledger.exit_cycle(cycle_id).await;

    // One bounded trace per cycle (FR-002): evidence referenced by summary,
    // the decision, the plan objective, and the outcome — linked to the
    // actions it caused through the plan id the run observed.
    ledger
        .record_trace(BrainTraceRecord {
            trace_id: Uuid::new_v4(),
            cycle_id: Some(cycle_id),
            plan_id: plan_ids.first().copied(),
            evidence: record.evidence.clone(),
            decision: record.decision.clone(),
            objective: record.plan_objective.clone(),
            steps: record.steps.clone(),
            outcome: record.outcome.clone(),
            occurred_at: Utc::now(),
        })
        .await;
    BrainCycle {
        cycle_id: Some(cycle_id),
        ..record
    }
}

/// The cycle body, bracketed by the caller's cycle context.
async fn run_cycle(
    daemon: &Daemon,
    brain: &BrainConfig,
    subjects: Vec<(String, String)>,
    cycle_id: Uuid,
    consulted_live: bool,
) -> BrainCycle {
    let _ = cycle_id; // carried by the ledger context; kept for traceability
    let mut record = BrainCycle::default();
    record.consulted_live = consulted_live;
    let mut evidence = argus_ai_core::decision::context::ContextBuilder::new();
    let host = ResourceId::new("host", "local").expect("valid host resource id");
    for (unit, state) in subjects {
        evidence.evidence(host.as_str(), "unit", serde_json::json!(unit));
        evidence.evidence(host.as_str(), "service.state", serde_json::json!(state));
        record.evidence.push(format!("{unit}: {state}"));
    }
    if record.evidence.is_empty() {
        // Nothing to reason about: a healthy host produces no decision.
        record.provider_available = daemon.provider().is_some();
        return record;
    }
    record.provider_available = daemon.provider().is_some();

    // 2. Reason: decide-only (dedup → structured decisions → confidence
    //    gate → typed plan). The plan comes back UNEXECUTED — the brain's
    //    action path is the modern run boundary, not the dispatch port.
    let Some(provider) = daemon.provider() else {
        tracing::info!("brain cycle: no provider configured; observe-only");
        return record;
    };
    let events = argus_events::LocalEventBus::new(16);
    let threshold = brain.confidence_threshold;
    // The provider is metered (spec 007 FR-003): every call lands a usage
    // record correlated to this cycle through the ledger context.
    let metered = MeteredProvider::new(provider.clone(), daemon.ledger());
    match daemon
        .propose_once(metered.as_ref(), evidence, threshold, &events)
        .await
    {
        Ok(Some((plan, dedup_key))) => {
            record.decision = Some("provider decision passed the confidence gate".into());
            record.plan_objective = Some(plan.objective.clone());
            record.steps = plan
                .steps
                .iter()
                .map(|step| step.action.capability.as_str().to_string())
                .collect();
            // Persist the reasoning history so `plan.list` shows the brain's
            // work (FR-008).
            let correlation = uuid::Uuid::new_v4();
            let _ = daemon.repository().put_plan(correlation, &plan).await;
            // 3. Act — through the ordinary boundary, at the *effective*
            // level (spec 008 FR-003): min(managed ceiling, earned rung).
            // The ceiling is the operator's "how far may this instance
            // grow", never a command to act at that level now.
            let effective = daemon.autonomy().effective(brain.autonomy).await;
            let outcome = daemon.run_remediation(&plan, effective, &events).await;
            match outcome {
                crate::control::RunOutcome::Finished(report) => {
                    let resolved = report.status == argus_domain::PlanStatus::Completed;
                    record.validation_failed = argus_domain::is_validation_failure(report.status);
                    for (index, execution) in report.executions.iter().enumerate() {
                        let _ = daemon
                            .repository()
                            .put_execution(
                                format!("{correlation}:{index}")
                                    .parse()
                                    .unwrap_or_else(|_| uuid::Uuid::new_v4()),
                                execution,
                            )
                            .await;
                    }
                    record.outcome = Some(format!("{:?}", report.status));
                    remember(daemon, &record, resolved).await;
                    if !resolved {
                        // The situation persists: allow the next cycle to
                        // reason about it again.
                        daemon.release_dedup(&dedup_key).await;
                    }
                }
                crate::control::RunOutcome::Pending(pending) => {
                    // Paused for an operator approval — stored on the daemon
                    // for `approval.grant`; the dedup key stays claimed so
                    // the loop does not pile up duplicates while a human
                    // decides.
                    record.outcome = Some(format!("awaiting approval {}", pending.token));
                    remember(daemon, &record, false).await;
                }
            }
        }
        Ok(None) => {
            record.decision = Some("no actionable decision this cycle".into());
        }
        Err(error) => {
            record.decision = Some(format!("decision failed: {error}"));
        }
    }
    record
}

/// Record what happened into the memory layers (FR-003): one episode per
/// cycle that acted, and a procedure outcome keyed by the plan objective.
async fn remember(daemon: &Daemon, record: &BrainCycle, resolved: bool) {
    let subject = ResourceId::new("host", "local").expect("valid host resource id");
    let now = Utc::now();
    let episode = Episode {
        id: Uuid::new_v4(),
        subject,
        symptom: "host-health".to_string(),
        resource_class: "host".to_string(),
        root_cause: None,
        remediation: record.plan_objective.clone(),
        outcome: if resolved {
            EpisodeOutcome::Resolved
        } else {
            EpisodeOutcome::Unresolved
        },
        change_proximity: None,
        started_at: now,
        resolved_at: resolved.then_some(now),
    };
    if let Err(error) = daemon.repository().put_episode(episode.id, &episode).await {
        tracing::warn!(%error, "failed to persist the brain episode");
    }

    // Procedure outcomes aggregate under a stable name so the success rate
    // is real history, not per-run noise.
    let name = record
        .plan_objective
        .clone()
        .unwrap_or_else(|| "host-health".to_string());
    let existing = daemon
        .repository()
        .list_procedures()
        .await
        .unwrap_or_default()
        .into_iter()
        .find(|(_, p)| p.name == name && p.trigger == "host-health");
    let mut procedure = match &existing {
        Some((_, p)) => p.clone(),
        None => ProcedureRecord::new(
            Uuid::new_v4(),
            "host-health",
            name,
            ProcedureStatus::Candidate,
            now,
        ),
    };
    procedure.record_outcome(resolved, now);
    let id = existing.map(|(id, _)| id).unwrap_or_else(Uuid::new_v4);
    if let Err(error) = daemon.repository().put_procedure(id, &procedure).await {
        tracing::warn!(%error, "failed to persist the brain procedure outcome");
    }
}

/// Failed systemd units from live state, as `(unit, state)` pairs, plus
/// whether the live read succeeded — a healthy host reads empty *and*
/// consulted (spec 008's MAP gate); an unavailable systemd neither reads
/// nor consults.
async fn failed_units() -> (Vec<(String, String)>, bool) {
    let Ok(client) = argus_systemd::SystemdClient::connect().await else {
        return (Vec::new(), false);
    };
    let Ok(units) = client.list_units().await else {
        return (Vec::new(), false);
    };
    // Sorted by name so the cycle's subject is deterministic no matter what
    // order systemd enumerates units in.
    let mut failed: Vec<(String, String)> = units
        .into_iter()
        .filter(|u| u.active_state == "failed")
        .map(|u| (u.name, format!("{} {}", u.active_state, u.sub_state)))
        .collect();
    failed.sort_by(|a, b| a.0.cmp(&b.0));
    (failed, true)
}

/// Spawns the periodic brain loop (FR-003). Failures tick-to-tick warn and
/// never stop the loop. Control levers are read from `control` each tick, so
/// a managed-configuration change lands on the next cycle (spec 006 FR-002);
/// each cycle's record lands in `state` for the sentinel report (FR-001).
pub fn spawn(
    daemon: Arc<Daemon>,
    config: BrainConfig,
    control: Arc<crate::brain_state::BrainControlHandle>,
    state: Arc<crate::brain_state::BrainState>,
) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(config.interval_seconds.max(5)));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let levers = control.get();
            let period = Duration::from_secs(levers.interval_seconds.max(5));
            if ticker.period() != period {
                ticker = tokio::time::interval(period);
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            }
            let daemon = Arc::clone(&daemon);
            let brain = BrainConfig {
                autonomy: levers.autonomy,
                confidence_threshold: levers.confidence_threshold,
                interval_seconds: levers.interval_seconds,
                runbooks_dir: config.runbooks_dir.clone(),
            };
            let spawned = Arc::clone(&daemon);
            let outcome = tokio::task::spawn(async move { cycle(&spawned, &brain).await }).await;
            match outcome {
                Ok(record) => {
                    state.record(crate::brain_state::BrainCycleRecord {
                        cycle_id: record.cycle_id,
                        evidence: record.evidence.clone(),
                        provider_available: record.provider_available,
                        decision: record.decision.clone(),
                        plan: record.plan_objective.clone(),
                        outcome: record.outcome.clone(),
                        at: chrono::Utc::now(),
                    });
                    // Spec 008: the assimilation machine ticks with the brain
                    // loop — this cycle's shape is the gate evidence (FR-001/
                    // FR-002). All signals are numbers the daemon already
                    // computes: provider health, cloud connectivity, the
                    // self-observability safe mode, and the run outcome.
                    let safe_mode = crate::sentinel::safe_mode_from(
                        &daemon.self_observability().snapshot(),
                        &argus_observability::DegradationThresholds::default(),
                    );
                    let signals = crate::autonomy::CycleSignals {
                        ceiling: levers.autonomy,
                        provider_ready: record.provider_available
                            && daemon.provider_health().status
                                == crate::runtime::ProviderStatus::Ready,
                        cloud_paired: daemon.cloud_paired().await,
                        safe_mode_active: safe_mode != argus_observability::SafeMode::None,
                        // No live incident source is wired yet; the sentinel
                        // view reports the same zero — never invented.
                        open_critical_incidents: 0,
                        // The MAP gate wants proof the environment was
                        // actually observed: a cycle that found nothing to
                        // reason about (the empty-evidence early return)
                        // consulted no live state and is not mapping
                        // evidence — an all-quiet host stays in mapping
                        // until it has something real to look at.
                        live_environment: !record.evidence.is_empty(),
                        // The typed revocation signature decided at the run
                        // site — never a string match on the outcome.
                        failed_validation: record.validation_failed,
                    };
                    tracing::info!(
                        clean = signals.clean(),
                        live = signals.live_environment,
                        provider = signals.provider_ready,
                        paired = signals.cloud_paired,
                        safe = signals.safe_mode_active,
                        incidents = signals.open_critical_incidents,
                        failed_validation = signals.failed_validation,
                        "autonomy tick signals"
                    );
                    daemon.autonomy().on_cycle(signals).await;
                    if !record.evidence.is_empty() {
                        tracing::info!(
                            evidence = ?record.evidence,
                            decision = ?record.decision,
                            outcome = ?record.outcome,
                            "brain cycle complete"
                        );
                    }
                }
                Err(error) => tracing::warn!(%error, "brain cycle task failed"),
            }
        }
    });
}
