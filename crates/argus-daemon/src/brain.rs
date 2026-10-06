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
use argus_domain::ResourceId;
use argus_memory::{Episode, EpisodeOutcome, ProcedureRecord, ProcedureStatus};
use uuid::Uuid;

use crate::config::{BrainConfig, DaemonConfig};
use crate::runtime::Daemon;

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
    /// The evidence keys the cycle gathered (e.g. `unit:<name> state=failed`).
    pub evidence: Vec<String>,
    /// Whether a provider was available to reason with.
    pub provider_available: bool,
    /// The provider's decision summary, when one ran.
    pub decision: Option<String>,
    /// The plan the brain proposed, when the decision passed the gates.
    pub plan_objective: Option<String>,
    /// The plan's terminal status, when it ran.
    pub outcome: Option<String>,
}

/// Runs one full brain cycle (FR-003) and records its memory. Used by both
/// the periodic loop and the `brain.diagnose` IPC trigger.
pub async fn cycle(daemon: &Daemon, brain: &BrainConfig) -> BrainCycle {
    // Evidence from live state: the first failed systemd unit is the
    // cycle's subject (the host-health skill reasons about one service).
    let subjects: Vec<(String, String)> = failed_units().await.into_iter().take(1).collect();
    cycle_with_evidence(daemon, brain, subjects).await
}

/// The cycle over injected subjects — the seam tests use so a brain cycle
/// can be exercised without a system bus.
pub async fn cycle_with_evidence(
    daemon: &Daemon,
    brain: &BrainConfig,
    subjects: Vec<(String, String)>,
) -> BrainCycle {
    let mut record = BrainCycle::default();
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
    match daemon
        .propose_once(provider.as_ref(), evidence, threshold, &events)
        .await
    {
        Ok(Some((plan, dedup_key))) => {
            record.decision = Some("provider decision passed the confidence gate".into());
            record.plan_objective = Some(plan.objective.clone());
            // Persist the reasoning history so `plan.list` shows the brain's
            // work (FR-008).
            let correlation = uuid::Uuid::new_v4();
            let _ = daemon.repository().put_plan(correlation, &plan).await;
            // 3. Act — through the ordinary boundary, at the operator's level.
            let outcome = daemon.run_remediation(&plan, brain.autonomy, &events).await;
            match outcome {
                crate::control::RunOutcome::Finished(report) => {
                    let resolved = report.status == argus_domain::PlanStatus::Completed;
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

/// Failed systemd units from live state, as `(unit, state)` pairs. An
/// unavailable systemd (no system bus) contributes nothing — the cycle
/// simply finds no evidence.
async fn failed_units() -> Vec<(String, String)> {
    let Ok(client) = argus_systemd::SystemdClient::connect().await else {
        return Vec::new();
    };
    let Ok(units) = client.list_units().await else {
        return Vec::new();
    };
    units
        .into_iter()
        .filter(|u| u.active_state == "failed")
        .map(|u| (u.name, format!("{} {}", u.active_state, u.sub_state)))
        .collect()
}

/// Spawns the periodic brain loop (FR-003). Failures tick-to-tick warn and
/// never stop the loop.
pub fn spawn(daemon: Arc<Daemon>, brain: BrainConfig) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(brain.interval_seconds.max(5)));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let daemon = Arc::clone(&daemon);
            let config = brain.clone();
            let outcome = tokio::task::spawn(async move { cycle(&daemon, &config).await }).await;
            match outcome {
                Ok(record) => {
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
