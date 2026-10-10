//! The brain: the running observe → reason → act loop (spec 005), perceiving
//! a typed situation set (spec 011): failed systemd units (`host-health`,
//! unchanged) plus resource-pressure threshold crossings (`memory-pressure`,
//! `disk-pressure`) read kernel-native each cycle.
//!
//! The loop turns each situation into typed evidence, drives any promoted
//! runbook whose trigger matches the situation's signature through its own
//! deterministic procedure, and otherwise poses the host-health structured
//! decisions — all situations' evidence in one request — to the configured
//! provider through [`Daemon::diagnose_once`]. Every plan executes through
//! the ordinary safety boundary (`run_remediation`) at the operator's
//! configured autonomy — L0 by default, so a fresh install observes and
//! explains but executes nothing. Model output is data: it can answer
//! questions, never grant authority.

use std::sync::Arc;

use chrono::Utc;
use tokio::time::Duration;

use argus_ai_core::decision::adapters::{DeepSeekProvider, LayaHttpProvider};
use argus_ai_core::decision::context::ContextBuilder;
use argus_ai_core::decision::provider::DecisionProvider;
use argus_domain::{Action, BrainTraceRecord, Plan, PlanStep, ResourceId, TokenUsageRecord};
use argus_memory::{Episode, EpisodeOutcome, ProcedureRecord, ProcedureStatus};
use uuid::Uuid;

use crate::config::{BrainConfig, DaemonConfig, SituationsConfig};
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
    /// The promoted runbook whose procedure ran, when the plan was a
    /// deterministic procedure plan (spec 010 FR-002). `None` for provider
    /// plans.
    pub runbook: Option<String>,
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
    // Spec 011 FR-001: the cycle's situation set, collected from
    // kernel-native sources — failed systemd units (today's behavior,
    // unchanged) plus the live pressure readings.
    let (failed_units, units_live) = failed_units().await;
    // Fail-closed (AC-005): a failed reading contributes no situation and is
    // logged — never a fabricated one, and the failed-unit collection is
    // never degraded by a pressure read.
    let (memory, memory_live) = match argus_sensors::meminfo::read() {
        Ok(memory) => (Some(memory), true),
        Err(error) => {
            tracing::warn!(%error, "meminfo read failed; no memory-pressure situation this cycle");
            (None, false)
        }
    };
    let (disk, disk_live) = match argus_sensors::disk::usage(argus_sensors::disk::DEFAULT_MOUNT) {
        Ok(disk) => (Some(disk), true),
        Err(error) => {
            tracing::warn!(%error, "statvfs read failed; no disk-pressure situation this cycle");
            (None, false)
        }
    };
    let readings = Readings {
        failed_units,
        memory,
        disk,
    };
    let situations = situations_from(&readings, &daemon.config().situations);
    // FR-004: a pressure situation whose reading dropped below the threshold
    // has resolved — its dedup key releases so a recrossing re-reasons (held
    // while a plan for the situation is pending approval).
    release_resolved_pressure(daemon, &readings, &daemon.config().situations).await;
    // The honest MAP-gate signal (spec 008): true when ANY situation
    // collection read succeeded — a quiet-but-healthy host consults (it read
    // the sources and found no situation); a host where every source failed
    // did not.
    let consulted_live = units_live || memory_live || disk_live;
    cycle_with_situations(daemon, brain, situations, consulted_live).await
}

/// The cycle over injected subjects — the seam tests use so a brain cycle
/// can be exercised without a system bus. Subjects map to `host-health`
/// situations, the established vocabulary.
pub async fn cycle_with_evidence(
    daemon: &Daemon,
    brain: &BrainConfig,
    subjects: Vec<(String, String)>,
    consulted_live: bool,
) -> BrainCycle {
    let situations = subjects
        .into_iter()
        .map(|(unit, state)| Situation {
            signature: HOST_HEALTH.to_string(),
            subject: unit,
            detail: state,
        })
        .collect();
    cycle_with_situations(daemon, brain, situations, consulted_live).await
}

/// The cycle over injected situations — the seam tests use so a brain cycle
/// can be exercised without a system bus or kernel readings (spec 011).
pub async fn cycle_with_situations(
    daemon: &Daemon,
    brain: &BrainConfig,
    situations: Vec<Situation>,
    consulted_live: bool,
) -> BrainCycle {
    // Spec 007 FR-002: every cycle carries an id, minted at entry, that its
    // trace, usage records, and action events share.
    let cycle_id = Uuid::new_v4();
    let ledger = daemon.ledger();
    ledger.enter_cycle(cycle_id).await;
    let record = run_cycle(daemon, brain, situations, cycle_id, consulted_live).await;
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

/// The cycle body, bracketed by the caller's cycle context: reason per
/// situation in collection order (spec 011 FR-002), act on each procedure
/// plan through the ordinary boundary, and fall to ONE provider request over
/// the merged evidence when no procedure plan fired (ADR-0043 §3).
async fn run_cycle(
    daemon: &Daemon,
    brain: &BrainConfig,
    situations: Vec<Situation>,
    cycle_id: Uuid,
    consulted_live: bool,
) -> BrainCycle {
    let _ = cycle_id; // carried by the ledger context; kept for traceability
    let mut record = BrainCycle {
        consulted_live,
        ..BrainCycle::default()
    };
    let host = ResourceId::new("host", "local").expect("valid host resource id");

    // Spec 011 FR-005: the sentinel's pressure section rides the SAME
    // collector the brain reasons from — one perception, two consumers,
    // never two truths. Present while a reading crosses its threshold,
    // cleared when it does not (no latching).
    daemon.note_pressure(pressure_signals(&situations));

    // 1. Evidence — per situation, in collection order (deterministic: failed
    //    units first, then memory, then disk). Every line names the reading
    //    that triggered the situation (ADR-0043 §4): failed-unit evidence
    //    exactly as today; pressure lines `{signature}: {detail}` where the
    //    detail carries the reading.
    let mut per_situation: Vec<(Situation, ContextBuilder)> = Vec::new();
    let mut merged = ContextBuilder::new();
    for situation in &situations {
        let mut evidence = ContextBuilder::new();
        add_situation_evidence(&mut evidence, host.as_str(), situation);
        add_situation_evidence(&mut merged, host.as_str(), situation);
        record.evidence.push(evidence_line(situation));
        per_situation.push((situation.clone(), evidence));
    }
    if record.evidence.is_empty() {
        // Nothing to reason about: a healthy host produces no decision.
        record.provider_available = daemon.provider().is_some();
        return record;
    }
    record.provider_available = daemon.provider().is_some();

    // 2. Reason — procedure plans first, per situation (spec 011 FR-002,
    //    ADR-0042 §1): when a situation's signature matches a promoted
    //    runbook's trigger, the runbook's own deterministic procedure crosses
    //    the ordinary boundary below and the provider is never consulted — a
    //    procedure plan is not AI output at all. No matching promoted
    //    runbook — or any construction failure, fail-closed — and the
    //    situation's evidence holds for the provider request.
    let events = argus_events::LocalEventBus::new(16);
    let threshold = brain.confidence_threshold;
    let mut procedure_fired = false;
    for (situation, evidence) in &per_situation {
        let attributed = match promoted_runbook(daemon, &situation.signature) {
            Some(runbook) => {
                let dedup_key = situation_dedup_key(situation, evidence);
                match daemon
                    .propose_procedure(&runbook, situation, &dedup_key)
                    .await
                {
                    Ok(attributed) => attributed,
                    Err(error) => {
                        tracing::warn!(
                            runbook = %runbook.name,
                            %error,
                            "procedure plan attempt failed; falling through to the provider"
                        );
                        None
                    }
                }
            }
            None => None,
        };
        if let Some((plan, dedup_key)) = attributed {
            procedure_fired = true;
            act(
                daemon,
                brain,
                &mut record,
                &plan,
                dedup_key,
                &situation.signature,
                &events,
            )
            .await;
        }
    }

    // 3. The provider path — ONE request over ALL situations' merged
    //    evidence when no procedure plan fired (spec 011 FR-002): the
    //    existing flow, evidence superset. A procedure plan anywhere in the
    //    cycle suppresses it (a procedure plan is not AI output at all);
    //    provider plans answer the host-health skill, so their episodes keep
    //    today's signature.
    if !procedure_fired {
        let Some(provider) = daemon.provider() else {
            tracing::info!("brain cycle: no provider configured; observe-only");
            return record;
        };
        // The request's dedup key derives from the STABLE situation
        // identities — the joined per-situation keys — never the raw
        // evidence: the readings jitter, the situation set does not (spec
        // 011 NFR). Claimed before the propose (the procedure path's
        // mechanism), so the two paths can never double-act on one
        // situation set; released on a no-decision result here and on
        // non-resolution in `act` — the evidence key's exact semantics.
        let provider_key = situation_set_key(&situations);
        let claimed = match daemon.claim_dedup(&provider_key).await {
            Ok(claimed) => claimed,
            Err(error) => {
                record.decision = Some(format!("decision failed: {error}"));
                return record;
            }
        };
        if !claimed {
            // The situation set is already reasoned about — a plan is in
            // flight, or it resolved and persists.
            record.decision = Some("no actionable decision this cycle".into());
            return record;
        }
        // The provider is metered (spec 007 FR-003): every call lands a usage
        // record correlated to this cycle through the ledger context.
        let metered = MeteredProvider::new(provider.clone(), daemon.ledger());
        match daemon
            .propose_once(metered.as_ref(), merged, threshold, &events)
            .await
        {
            Ok(Some((plan, _evidence_key))) => {
                act(
                    daemon,
                    brain,
                    &mut record,
                    &plan,
                    provider_key,
                    HOST_HEALTH,
                    &events,
                )
                .await;
            }
            Ok(None) => {
                // Nothing actionable: release the set (fail-closed, the
                // evidence key's release-on-nothing) so the next cycle can
                // reason again.
                daemon.release_dedup(&provider_key).await;
                record.decision = Some("no actionable decision this cycle".into());
                return record;
            }
            Err(error) => {
                record.decision = Some(format!("decision failed: {error}"));
                return record;
            }
        }
    }
    record
}

/// Adds one situation's structured evidence to a builder: failed-unit entries
/// exactly as today; pressure entries naming the signature and the reading.
fn add_situation_evidence(evidence: &mut ContextBuilder, host: &str, situation: &Situation) {
    match situation.signature.as_str() {
        HOST_HEALTH => {
            evidence.evidence(host, "unit", serde_json::json!(situation.subject));
            evidence.evidence(host, "service.state", serde_json::json!(situation.detail));
        }
        _ => {
            evidence.evidence(host, "situation", serde_json::json!(situation.signature));
            evidence.evidence(host, "pressure", serde_json::json!(situation.detail));
        }
    }
}

/// The trace line for one situation: the failed-unit form today's traces
/// carry, or `{signature}: {detail}` for pressure — the reading that
/// triggered everything (spec 011 FR-005, ADR-0043 §4).
fn evidence_line(situation: &Situation) -> String {
    match situation.signature.as_str() {
        HOST_HEALTH => format!("{}: {}", situation.subject, situation.detail),
        _ => format!("{}: {}", situation.signature, situation.detail),
    }
}

/// Acts one decided plan through the ordinary boundary and records what
/// happened — the shared tail of the procedure and provider paths. The cycle
/// record's decision/plan fields reflect the FIRST acting situation of the
/// cycle (collection order); every acting situation lands its own episode,
/// runbook outcome, and dedup handling (spec 011 FR-002).
async fn act(
    daemon: &Daemon,
    brain: &BrainConfig,
    record: &mut BrainCycle,
    plan: &Plan,
    dedup_key: String,
    signature: &str,
    events: &dyn argus_events::EventBus,
) {
    let first = record.plan_objective.is_none();
    if first {
        record.decision = Some(match plan.runbook.as_deref() {
            Some(runbook) => format!("procedure plan from promoted runbook '{runbook}'"),
            None => "provider decision passed the confidence gate".into(),
        });
        record.plan_objective = Some(plan.objective.clone());
        record.runbook = plan.runbook.clone();
        record.steps = plan
            .steps
            .iter()
            .map(|step| step.action.capability.as_str().to_string())
            .collect();
    }
    // Persist the reasoning history so `plan.list` shows the brain's
    // work (FR-008).
    let correlation = uuid::Uuid::new_v4();
    let _ = daemon.repository().put_plan(correlation, plan).await;
    // Act — through the ordinary boundary, at the *effective* level (spec
    // 008 FR-003): min(managed ceiling, earned rung). The ceiling is the
    // operator's "how far may this instance grow", never a command to act
    // at that level now.
    let effective = daemon.autonomy().effective(brain.autonomy).await;
    let outcome = daemon.run_remediation(plan, effective, events).await;
    match outcome {
        crate::control::RunOutcome::Finished(report) => {
            let resolved = report.status == argus_domain::PlanStatus::Completed;
            record.validation_failed |= argus_domain::is_validation_failure(report.status);
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
            if first {
                record.outcome = Some(format!("{:?}", report.status));
            }
            remember(daemon, signature, Some(plan.objective.clone()), resolved).await;
            // Spec 010 FR-003/FR-004: the terminal outcome flows to the
            // attributed runbook — Validation → Policy plus a successful
            // history entry when the run completed, an unsuccessful entry
            // when it was attempted and failed. A refusal (no executions)
            // records nothing: a procedure that never ran is not an attempt.
            // Plans without attribution (the provider path) are untouched;
            // the spec-009 trigger-vocabulary fallback is retired.
            daemon.note_runbook_outcome(plan, &report).await;
            if resolved {
                // The situation is resolved; the dedup key stays claimed.
            } else {
                // The situation persists: allow the next cycle to
                // reason about it again.
                daemon.release_dedup(&dedup_key).await;
            }
        }
        crate::control::RunOutcome::Pending(pending) => {
            // Paused for an operator approval — stored on the daemon
            // for `approval.grant`; the dedup key stays claimed so
            // the loop does not pile up duplicates while a human
            // decides. On the grant, the same attributed plan resumes
            // and its outcome flows to the runbook (spec 010 AC-006);
            // an attributed plan carries its dedup key so a resumed
            // failure releases the situation (AC-004). Provider plans
            // keep their unchanged pause behavior.
            let mut pending = pending;
            let token = pending.token;
            if plan.runbook.is_some() {
                pending.dedup_key = Some(dedup_key);
                daemon.store_pending(pending);
            }
            if first {
                record.outcome = Some(format!("awaiting approval {token}"));
            }
            remember(daemon, signature, Some(plan.objective.clone()), false).await;
        }
    }
}

/// The first promoted runbook (name-ordered) whose trigger matches the
/// situation's signature — the runbook whose procedure this cycle would run.
/// The signature IS the vocabulary (spec 011 FR-002): a runbook authored for
/// `memory-pressure` matches the moment a memory-pressure situation exists.
/// Candidates and approved-but-unpromoted runbooks never drive procedures
/// (ADR-0036 §3).
fn promoted_runbook(daemon: &Daemon, signature: &str) -> Option<argus_runbooks::Runbook> {
    let trigger = argus_runbooks::RunbookTrigger::Symptom(signature.to_string());
    daemon.runbooks().list().into_iter().find(|runbook| {
        runbook.status() == argus_runbooks::RunbookStatus::Promoted
            && runbook.matches_trigger(&trigger)
    })
}

/// Builds the deterministic procedure plan from a promoted runbook (spec 010
/// FR-001, generalized per signature by spec 011 FR-002): objective
/// `procedure: <name>`, one step per allowed action targeting the situation's
/// subject with the runbook's declared rollback entry paired where present,
/// blast radius the worst of its actions' registered descriptors, confidence
/// the runbook's historical success rate when known (else a conservative 0.5).
///
/// Step arguments derive per signature (spec 011): `host-health` targets the
/// failed unit (`{"unit": …}`); pressure signatures carry the situation's
/// subject as the only argument candidate (`{"subject": …}`) — a mount for
/// disk, `mem` for memory. Every step validates against its capability's
/// registered input schema at construction ([`argus_domain::input_matches`]):
/// a non-conforming step is skipped, an unregistered capability is
/// non-conforming by definition (a procedure plan widens nothing), and a
/// runbook whose actions cannot express the situation yields no plan at all —
/// the caller falls through to the provider honestly. Attribution rides the
/// plan ([`Plan::runbook`]): the plan *is* the runbook's procedure, never an
/// inferred link.
pub fn procedure_plan(
    runbook: &argus_runbooks::Runbook,
    situation: &Situation,
    descriptors: &[argus_domain::CapabilityDescriptor],
) -> Option<Plan> {
    let arguments = match situation.signature.as_str() {
        HOST_HEALTH => serde_json::json!({ "unit": situation.subject }),
        _ => serde_json::json!({ "subject": situation.subject }),
    };

    let mut steps = Vec::new();
    let mut blast_radius = argus_domain::BlastRadius::None;
    for (index, capability) in runbook.allowed_actions.iter().enumerate() {
        let Some(descriptor) = descriptors.iter().find(|d| d.id() == capability) else {
            tracing::debug!(
                runbook = %runbook.name,
                capability = %capability.as_str(),
                "procedure step skipped: capability not registered"
            );
            continue;
        };
        if !argus_domain::input_matches(descriptor.input_schema(), &arguments) {
            tracing::debug!(
                runbook = %runbook.name,
                capability = %capability.as_str(),
                "procedure step skipped: the situation's subject does not satisfy the capability schema"
            );
            continue;
        }
        // The runbook's declared rollback entry for this action, paired only
        // when it too can take the situation's arguments — never invented.
        let rollback = runbook.rollback.get(index).and_then(|rollback| {
            let conforming = descriptors
                .iter()
                .find(|d| d.id() == rollback)
                .is_some_and(|d| argus_domain::input_matches(d.input_schema(), &arguments));
            conforming.then_some(Action {
                capability: rollback.clone(),
                resource: None,
                arguments: arguments.clone(),
            })
        });
        steps.push(PlanStep {
            action: Action {
                capability: capability.clone(),
                resource: None,
                arguments: arguments.clone(),
            },
            rollback,
        });
        blast_radius = worst_blast_radius(blast_radius, descriptor.effective_blast_radius());
    }
    if steps.is_empty() {
        tracing::info!(
            runbook = %runbook.name,
            "procedure plan has no schema-valid steps for this situation; the provider path runs"
        );
        return None;
    }
    Some(Plan {
        objective: format!("procedure: {}", runbook.name),
        steps,
        preconditions: Vec::new(),
        expected_outcomes: Vec::new(),
        blast_radius,
        confidence: runbook
            .historical_success_rate()
            .map(f64::from)
            .unwrap_or(0.5),
        status: argus_domain::PlanStatus::Proposed,
        runbook: Some(runbook.name.clone()),
    })
}

/// The wider of two blast radii — a procedure plan's radius is the worst of
/// its actions' registered descriptors (spec 010 FR-001).
fn worst_blast_radius(
    a: argus_domain::BlastRadius,
    b: argus_domain::BlastRadius,
) -> argus_domain::BlastRadius {
    use argus_domain::BlastRadius::{Environment, Fleet, Host, None as NoRadius};
    match (a, b) {
        (Fleet, _) | (_, Fleet) => Fleet,
        (Environment, _) | (_, Environment) => Environment,
        (Host, _) | (_, Host) => Host,
        _ => NoRadius,
    }
}

/// Record what happened into the memory layers (FR-003): one episode per
/// acting situation — its symptom the SITUATION's signature (spec 011
/// FR-002; failed units keep `host-health`) — and a procedure outcome keyed
/// by the plan objective under the same signature, so the success rate is
/// real history per remediation, not per-run noise.
async fn remember(daemon: &Daemon, signature: &str, objective: Option<String>, resolved: bool) {
    let subject = ResourceId::new("host", "local").expect("valid host resource id");
    let now = Utc::now();
    let episode = Episode {
        id: Uuid::new_v4(),
        subject,
        symptom: signature.to_string(),
        resource_class: "host".to_string(),
        root_cause: None,
        remediation: objective.clone(),
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

    let name = objective.unwrap_or_else(|| signature.to_string());
    let existing = daemon
        .repository()
        .list_procedures()
        .await
        .unwrap_or_default()
        .into_iter()
        .find(|(_, p)| p.name == name && p.trigger == signature);
    let mut procedure = match &existing {
        Some((_, p)) => p.clone(),
        None => ProcedureRecord::new(
            Uuid::new_v4(),
            signature,
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
    let client = match argus_systemd::SystemdClient::connect().await {
        Ok(client) => client,
        Err(error) => {
            tracing::warn!(%error, "systemd connect failed; the cycle consults nothing");
            return (Vec::new(), false);
        }
    };
    let units = match client.list_units().await {
        Ok(units) => units,
        Err(error) => {
            tracing::warn!(%error, "systemd list_units failed; the cycle consults nothing");
            return (Vec::new(), false);
        }
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

/// The situation signature of failed systemd units — the established
/// vocabulary (spec 011 FR-001), unchanged.
pub const HOST_HEALTH: &str = "host-health";
/// The live memory-pressure signature: `memory.used_percent` at/above the
/// configured threshold, subject `mem` (spec 011 FR-001).
pub const MEMORY_PRESSURE: &str = "memory-pressure";
/// The live disk-pressure signature: root-filesystem usage at/above the
/// configured threshold, subject the mount (spec 011 FR-001).
pub const DISK_PRESSURE: &str = "disk-pressure";

/// One situation the brain perceived this cycle (ADR-0043 §1): a signature —
/// the vocabulary runbooks declare against — the subject it names, and the
/// reading that triggered it, in evidence form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Situation {
    /// The situation type (`host-health`, `memory-pressure`, `disk-pressure`).
    pub signature: String,
    /// What the situation is about: the failed unit, or the pressured
    /// resource (`mem`, the mount).
    pub subject: String,
    /// The reading that triggered the situation: the unit state for
    /// `host-health`; `mem at 91.4%` for pressure. The number an operator
    /// auditing a procedure plan sees (ADR-0043 §4).
    pub detail: String,
}

impl Situation {
    /// The situation's stable identity — `(signature, subject)`. The
    /// pressure situations dedup on it directly (stable while the crossing
    /// persists, distinct per situation — spec 011 NFR, AC-004); the
    /// provider path joins these identities into its set key
    /// ([`situation_set_key`]). The host-health PROCEDURE path keeps its
    /// evidence-derived key (byte-identical), via [`situation_dedup_key`].
    pub fn dedup_key(&self) -> String {
        format!("{}:{}", self.signature, self.subject)
    }
}

/// The raw readings one cycle collected (spec 011 FR-001) — the collector's
/// INPUT, so thresholds and situation shape are testable without a kernel.
#[derive(Debug, Clone, Default)]
pub struct Readings {
    /// Failed systemd units as `(unit, state)` pairs, name-sorted — the
    /// host-health source. Empty when the bus is unavailable.
    pub failed_units: Vec<(String, String)>,
    /// The `/proc/meminfo` reading; `None` when the read failed — that
    /// situation contributes nothing (fail-closed, AC-005).
    pub memory: Option<argus_sensors::meminfo::MemInfo>,
    /// The statvfs reading for the root mount; `None` when the read failed.
    pub disk: Option<argus_sensors::disk::DiskUsage>,
}

/// The pure situation collector (spec 011 FR-001): readings in, situations
/// out, in the cycle's deterministic order — failed units first (the
/// established vocabulary, capped at one like today), then memory, then
/// disk. A situation is present while its reading crosses the threshold and
/// absent when it does not: no hysteresis, no latching. A failed reading
/// contributes no situation (fail-closed, AC-005) — never a fabricated one.
pub fn situations_from(readings: &Readings, config: &SituationsConfig) -> Vec<Situation> {
    let mut situations = Vec::new();
    // Failed systemd units → `host-health`, unchanged. Capped at one: the
    // host-health skill reasons about a single service.
    if let Some((unit, state)) = readings.failed_units.first() {
        situations.push(Situation {
            signature: HOST_HEALTH.to_string(),
            subject: unit.clone(),
            detail: state.clone(),
        });
    }
    if let Some(memory) = &readings.memory {
        let used = memory.used_percent();
        if used >= config.memory_used_percent {
            tracing::info!(
                reading = used,
                threshold = config.memory_used_percent,
                "memory-pressure situation collected: mem at {used}%"
            );
            situations.push(Situation {
                signature: MEMORY_PRESSURE.to_string(),
                subject: "mem".to_string(),
                detail: format!("mem at {used:.1}%"),
            });
        }
    }
    if let Some(disk) = &readings.disk
        && disk.used_percent >= config.disk_used_percent
    {
        tracing::info!(
            reading = disk.used_percent,
            threshold = config.disk_used_percent,
            "disk-pressure situation collected: {} at {}%",
            disk.mount,
            disk.used_percent
        );
        situations.push(Situation {
            signature: DISK_PRESSURE.to_string(),
            subject: disk.mount.clone(),
            detail: format!("{} at {:.1}%", disk.mount, disk.used_percent),
        });
    }
    situations
}

/// The pressure dedup keys whose readings are present and BELOW threshold —
/// genuine resolutions (spec 011 FR-004): the situation is absent and its
/// dedup key releases, so a recrossing re-reasons. A failed reading releases
/// nothing: absence by read failure is not resolution (fail-closed).
pub fn resolved_pressure_keys(readings: &Readings, config: &SituationsConfig) -> Vec<String> {
    let mut keys = Vec::new();
    if let Some(memory) = &readings.memory
        && memory.used_percent() < config.memory_used_percent
    {
        keys.push(format!("{MEMORY_PRESSURE}:mem"));
    }
    if let Some(disk) = &readings.disk
        && disk.used_percent < config.disk_used_percent
    {
        keys.push(format!("{DISK_PRESSURE}:{}", disk.mount));
    }
    keys
}

/// The cycle's resolution step (spec 011 FR-004): releases each pressure key
/// whose reading settled below its threshold — except while a paused
/// (approval-pending) plan carries that exact key: its situation is still
/// being handled, and a momentary resolution must not free a recrossing to
/// propose a second plan around an open approval. Host-health releases ride
/// today's run-outcome mechanics, untouched.
pub async fn release_resolved_pressure(
    daemon: &Daemon,
    readings: &Readings,
    config: &SituationsConfig,
) {
    for key in resolved_pressure_keys(readings, config) {
        if daemon.pending_holds_dedup(&key) {
            tracing::debug!(
                %key,
                "pressure resolution holds its dedup key: a plan for the situation is pending approval"
            );
            continue;
        }
        daemon.release_dedup(&key).await;
    }
}

/// The situation's dedup key for the procedure path. `host-health` keeps the
/// evidence-derived key ([`argus_ai_core::decision::host_health::
/// observation_dedup_key`] — byte-identical to today); pressure situations
/// key on `(signature, subject)`, so a stable crossing reasons once even as
/// the reading fluctuates (spec 011 NFR).
fn situation_dedup_key(situation: &Situation, evidence: &ContextBuilder) -> String {
    if situation.signature == HOST_HEALTH {
        use argus_ai_core::decision::host_health::observation_dedup_key;
        observation_dedup_key(evidence)
    } else {
        situation.dedup_key()
    }
}

/// The provider request's dedup key: the cycle's situation identities —
/// [`Situation::dedup_key()`] per situation — sorted and joined. Stable while
/// the situation set persists (the readings jitter; the identities do not)
/// and distinct per set, so one provider request reasons one situation set
/// once instead of re-planning on every reading wiggle (spec 011 NFR).
fn situation_set_key(situations: &[Situation]) -> String {
    let mut keys: Vec<String> = situations.iter().map(Situation::dedup_key).collect();
    keys.sort();
    keys.join("|")
}

/// The current resource-pressure crossings as sentinel pressure signals
/// (spec 011 FR-005): one per pressure situation at `Warning` — a threshold
/// crossing is a warning, never an invented error. Host-health is not
/// pressure.
fn pressure_signals(situations: &[Situation]) -> Vec<crate::sentinel::PressureSignal> {
    situations
        .iter()
        .filter(|s| s.signature == MEMORY_PRESSURE || s.signature == DISK_PRESSURE)
        .map(|s| crate::sentinel::PressureSignal {
            subject: s.subject.clone(),
            severity: argus_domain::Severity::Warning,
        })
        .collect()
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
                        runbook: record.runbook.clone(),
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
                        // The MAP gate wants proof live state was consulted:
                        // ANY situation collection read succeeded (spec 011)
                        // — the systemd read, the meminfo read, or the
                        // statvfs read. A quiet-but-healthy host consults;
                        // a host where every source failed does not.
                        live_environment: record.consulted_live,
                        // The typed revocation signature decided at the run
                        // site — never a string match on the outcome.
                        failed_validation: record.validation_failed,
                    };
                    tracing::debug!(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SituationsConfig;
    use argus_domain::{BlastRadius, CapabilityDescriptor, CapabilityId, Reversibility, RiskClass};
    use argus_runbooks::{Runbook, RunbookTrigger};
    use semver::Version;

    /// The bootstrap service schema (runtime.rs `bootstrap_descriptors`): a
    /// unit name is required and nothing else is permitted.
    fn service_schema() -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["unit"],
            "properties": { "unit": { "type": "string" } },
            "additionalProperties": false,
        })
    }

    fn restart_descriptor() -> CapabilityDescriptor {
        CapabilityDescriptor::new(
            CapabilityId::new("host.service.restart").unwrap(),
            "argusd",
            "host.service.restart",
            RiskClass::LowRisk,
            Version::new(0, 1, 0),
            service_schema(),
            serde_json::json!({}),
            Reversibility::Reversible,
        )
        .with_blast_radius(BlastRadius::Host)
    }

    /// A pod restart targets pods by `name` — a vocabulary no situation in
    /// this test (a failed unit, or a pressure subject) can express.
    fn pod_restart_descriptor() -> CapabilityDescriptor {
        CapabilityDescriptor::new(
            CapabilityId::new("k8s.pod.restart").unwrap(),
            "argusd",
            "k8s.pod.restart",
            RiskClass::Controlled,
            Version::new(0, 1, 0),
            serde_json::json!({
                "type": "object",
                "required": ["name"],
                "properties": { "name": { "type": "string" } },
                "additionalProperties": false,
            }),
            serde_json::json!({}),
            Reversibility::Reversible,
        )
        .with_blast_radius(BlastRadius::Environment)
    }

    /// A drain-style action whose schema accepts the pressure vocabulary's
    /// one argument candidate (`subject`) — the stand-in for a capability a
    /// memory-pressure procedure could drive.
    fn drain_descriptor() -> CapabilityDescriptor {
        CapabilityDescriptor::new(
            CapabilityId::new("host.cache.drain").unwrap(),
            "argusd",
            "host.process.signal",
            RiskClass::Controlled,
            Version::new(0, 1, 0),
            serde_json::json!({
                "type": "object",
                "required": ["subject"],
                "properties": { "subject": { "type": "string" } },
                "additionalProperties": false,
            }),
            serde_json::json!({}),
            Reversibility::Reversible,
        )
        .with_blast_radius(BlastRadius::Host)
    }

    fn runbook(name: &str, actions: Vec<&str>, rollback: Vec<&str>) -> Runbook {
        Runbook::candidate(
            Uuid::new_v4(),
            name,
            RunbookTrigger::Symptom("host-health".into()),
            vec![],
            vec![],
            vec![],
            actions
                .into_iter()
                .map(|action| CapabilityId::new(action).unwrap())
                .collect(),
            rollback
                .into_iter()
                .map(|action| CapabilityId::new(action).unwrap())
                .collect(),
            vec![],
        )
    }

    fn runbook_for(name: &str, signature: &str, actions: Vec<&str>) -> Runbook {
        Runbook::candidate(
            Uuid::new_v4(),
            name,
            RunbookTrigger::Symptom(signature.into()),
            vec![],
            vec![],
            vec![],
            actions
                .into_iter()
                .map(|action| CapabilityId::new(action).unwrap())
                .collect(),
            vec![],
            vec![],
        )
    }

    /// The host-health situation the procedure-plan tests reason about: one
    /// failed unit, today's vocabulary.
    fn host_health() -> Situation {
        Situation {
            signature: HOST_HEALTH.to_string(),
            subject: "argus-test.service".to_string(),
            detail: "failed failed".to_string(),
        }
    }

    /// A memory reading at the given used percent: `(total − available) /
    /// total` matches exactly at one decimal.
    fn meminfo_at(used_percent: f64) -> argus_sensors::meminfo::MemInfo {
        let total = 1_000_000u64;
        let available = (total as f64 * (1.0 - used_percent / 100.0)).round() as u64;
        argus_sensors::meminfo::MemInfo {
            mem_total: total,
            mem_available: available,
            ..argus_sensors::meminfo::MemInfo::default()
        }
    }

    #[test]
    fn a_matching_runbook_builds_an_attributed_procedure_plan() {
        let rb = runbook(
            "restart-failed",
            vec!["host.service.restart"],
            vec!["host.service.restart"],
        );
        let plan = procedure_plan(&rb, &host_health(), &[restart_descriptor()]).expect("a plan");

        // Attribution by construction: the plan carries the runbook's name,
        // both in the field and in the objective approvals read.
        assert_eq!(plan.runbook.as_deref(), Some("restart-failed"));
        assert_eq!(plan.objective, "procedure: restart-failed");

        // One step per allowed action, targeting the situation's subject,
        // with the runbook's rollback entry paired.
        assert_eq!(plan.steps.len(), 1);
        let step = &plan.steps[0];
        assert_eq!(step.action.capability.as_str(), "host.service.restart");
        assert_eq!(
            step.action.arguments,
            serde_json::json!({ "unit": "argus-test.service" })
        );
        let rollback = step.rollback.as_ref().expect("the declared rollback pairs");
        assert_eq!(rollback.capability.as_str(), "host.service.restart");
        assert_eq!(
            rollback.arguments,
            serde_json::json!({ "unit": "argus-test.service" })
        );

        // Blast radius from the registered descriptor; confidence the
        // conservative 0.5 while the runbook has no history.
        assert_eq!(plan.blast_radius, BlastRadius::Host);
        assert_eq!(plan.confidence, 0.5);
        assert_eq!(plan.status, argus_domain::PlanStatus::Proposed);
    }

    #[test]
    fn a_pressure_runbook_derives_its_arguments_from_the_subject() {
        // Spec 011 FR-002: step arguments derive per signature — a
        // memory-pressure runbook's steps carry the situation's subject as
        // the one argument candidate, and `input_matches` decides.
        let rb = runbook_for("drain-cache", MEMORY_PRESSURE, vec!["host.cache.drain"]);
        let situation = Situation {
            signature: MEMORY_PRESSURE.to_string(),
            subject: "mem".to_string(),
            detail: "mem at 91.4%".to_string(),
        };
        let plan = procedure_plan(&rb, &situation, &[drain_descriptor()]).expect("a plan");
        assert_eq!(plan.runbook.as_deref(), Some("drain-cache"));
        assert_eq!(
            plan.steps[0].action.arguments,
            serde_json::json!({ "subject": "mem" })
        );
    }

    #[test]
    fn non_conforming_and_unregistered_steps_are_skipped() {
        // The pod action's schema wants `name`, which the situation (a failed
        // unit) cannot supply; the cordon capability is not registered at all.
        let rb = runbook(
            "mixed-procedure",
            vec!["k8s.pod.restart", "host.service.restart", "k8s.node.cordon"],
            vec![],
        );
        let plan = procedure_plan(
            &rb,
            &host_health(),
            &[restart_descriptor(), pod_restart_descriptor()],
        )
        .expect("the conforming step survives");

        assert_eq!(plan.steps.len(), 1, "only the schema-valid step remains");
        assert_eq!(
            plan.steps[0].action.capability.as_str(),
            "host.service.restart"
        );
        assert!(plan.steps[0].rollback.is_none(), "no rollback is invented");
        // The worst registered descriptor of the surviving steps.
        assert_eq!(plan.blast_radius, BlastRadius::Host);
    }

    #[test]
    fn a_runbook_that_cannot_express_the_situation_yields_no_plan() {
        // The pod action's schema wants `name`; neither a failed unit nor a
        // pressure subject can supply it — zero valid steps falls through to
        // the provider honestly (spec 011, the disk-pressure design note).
        let rb = runbook_for("pod-procedure", MEMORY_PRESSURE, vec!["k8s.pod.restart"]);
        let situation = Situation {
            signature: MEMORY_PRESSURE.to_string(),
            subject: "mem".to_string(),
            detail: "mem at 91.4%".to_string(),
        };
        assert!(
            procedure_plan(&rb, &situation, &[pod_restart_descriptor()]).is_none(),
            "a capability that cannot express the subject builds no plan"
        );
    }

    #[test]
    fn confidence_is_the_runbooks_history_and_blast_radius_its_worst_descriptor() {
        let mut rb = runbook(
            "widespread-procedure",
            vec!["host.service.restart", "k8s.pod.restart"],
            vec![],
        );
        rb.record_outcome(true);
        rb.record_outcome(false);
        let plan = procedure_plan(
            &rb,
            &host_health(),
            &[restart_descriptor(), pod_restart_descriptor()],
        )
        .expect("the service step survives");
        assert_eq!(plan.steps.len(), 1);
        assert!(
            (plan.confidence - 0.5).abs() < f64::EPSILON,
            "historical success rate 1/2, not the fixed default: {}",
            plan.confidence
        );
        // The surviving step is the Host-radius restart; a fleet-radius
        // action would widen it — covered by the pairing of worst_blast_radius.
        assert_eq!(plan.blast_radius, BlastRadius::Host);
        use argus_domain::BlastRadius::{Environment, Fleet, Host, None as NoRadius};
        assert_eq!(worst_blast_radius(NoRadius, Host), Host);
        assert_eq!(worst_blast_radius(Host, Environment), Environment);
        assert_eq!(worst_blast_radius(Environment, Fleet), Fleet);
        assert_eq!(worst_blast_radius(NoRadius, NoRadius), NoRadius);
    }

    // --- Spec 011: the pure situation collector ---

    #[test]
    fn a_memory_crossing_collects_the_pressure_situation() {
        let readings = Readings {
            memory: Some(meminfo_at(91.4)),
            ..Readings::default()
        };
        let situations = situations_from(&readings, &SituationsConfig::default());
        assert_eq!(situations.len(), 1);
        assert_eq!(situations[0].signature, MEMORY_PRESSURE);
        assert_eq!(situations[0].subject, "mem");
        assert_eq!(situations[0].detail, "mem at 91.4%");
        // The reading IS the evidence (ADR-0043 §4): the trigger line names
        // the number that crossed.
        assert_eq!(
            evidence_line(&situations[0]),
            "memory-pressure: mem at 91.4%"
        );
    }

    #[test]
    fn the_crossing_is_at_and_above_the_threshold_never_below() {
        // Exactly at the default 90: present (>=, AC-001).
        let at = Readings {
            memory: Some(meminfo_at(90.0)),
            ..Readings::default()
        };
        assert_eq!(situations_from(&at, &SituationsConfig::default()).len(), 1);
        // One tick below: absent (no hysteresis, no latching).
        let below = Readings {
            memory: Some(meminfo_at(89.9)),
            ..Readings::default()
        };
        assert!(
            situations_from(&below, &SituationsConfig::default()).is_empty(),
            "below the threshold there is no situation (AC-002)"
        );
    }

    #[test]
    fn a_failed_reading_contributes_no_situation() {
        // meminfo/statvfs unavailable → `None` → no pressure situation,
        // never a fabricated one (AC-005). Failed-unit collection rides its
        // own source and proceeds.
        let readings = Readings {
            failed_units: vec![("argus-test.service".into(), "failed failed".into())],
            memory: None,
            disk: None,
        };
        let situations = situations_from(&readings, &SituationsConfig::default());
        assert_eq!(situations.len(), 1);
        assert_eq!(situations[0].signature, HOST_HEALTH);
    }

    #[test]
    fn situations_collect_in_deterministic_order_units_then_memory_then_disk() {
        let readings = Readings {
            failed_units: vec![
                ("b.service".into(), "failed failed".into()),
                ("a.service".into(), "failed failed".into()),
            ],
            memory: Some(meminfo_at(95.0)),
            disk: Some(argus_sensors::disk::DiskUsage::from_blocks(
                "/", 4096, 100, 2,
            )),
        };
        let situations = situations_from(&readings, &SituationsConfig::default());
        assert_eq!(situations.len(), 3, "one failed unit (capped), mem, disk");
        assert_eq!(situations[0].signature, HOST_HEALTH);
        assert_eq!(
            situations[0].subject, "b.service",
            "name-sorted, capped at one"
        );
        assert_eq!(situations[1].signature, MEMORY_PRESSURE);
        assert_eq!(situations[2].signature, DISK_PRESSURE);
        assert_eq!(situations[2].subject, "/");
        assert_eq!(situations[2].detail, "/ at 98.0%");
        // Separate dedup keys per (signature, subject) — AC-004.
        let keys: std::collections::HashSet<String> =
            situations.iter().map(Situation::dedup_key).collect();
        assert_eq!(keys.len(), 3, "every situation dedups apart");
    }

    #[test]
    fn pressure_dedup_keys_are_signature_and_subject_not_the_fluctuating_reading() {
        let first = Situation {
            signature: MEMORY_PRESSURE.to_string(),
            subject: "mem".to_string(),
            detail: "mem at 91.4%".to_string(),
        };
        let second = Situation {
            signature: MEMORY_PRESSURE.to_string(),
            subject: "mem".to_string(),
            detail: "mem at 92.1%".to_string(),
        };
        assert_eq!(
            first.dedup_key(),
            second.dedup_key(),
            "a stable crossing reasons once (spec 011 NFR)"
        );
    }

    #[test]
    fn resolved_pressure_keys_release_exactly_the_settled_crossings() {
        let readings = Readings {
            memory: Some(meminfo_at(50.0)),
            disk: Some(argus_sensors::disk::DiskUsage::from_blocks(
                "/", 4096, 100, 2,
            )),
            failed_units: vec![("argus-test.service".into(), "failed failed".into())],
        };
        // Memory settled below the threshold; disk still crossing.
        let keys = resolved_pressure_keys(&readings, &SituationsConfig::default());
        assert_eq!(keys, vec!["memory-pressure:mem".to_string()]);
        // A failed reading releases nothing: absence is not resolution.
        let failed_read = Readings {
            memory: None,
            disk: None,
            failed_units: vec![],
        };
        assert!(resolved_pressure_keys(&failed_read, &SituationsConfig::default()).is_empty());
    }

    #[test]
    fn pressure_signals_surface_only_pressure_situations() {
        let situations = vec![
            host_health(),
            Situation {
                signature: MEMORY_PRESSURE.to_string(),
                subject: "mem".to_string(),
                detail: "mem at 91.4%".to_string(),
            },
        ];
        let signals = pressure_signals(&situations);
        assert_eq!(signals.len(), 1, "host-health is not pressure");
        assert_eq!(signals[0].subject, "mem");
        assert_eq!(signals[0].severity, argus_domain::Severity::Warning);
    }
}
