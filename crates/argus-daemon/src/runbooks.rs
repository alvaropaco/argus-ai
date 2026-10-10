//! Delivered runbooks: the promotion ladder's production caller (spec 009,
//! ADR-0041).
//!
//! The cloud *authors and delivers*; the host *earns*; the operator *decides*.
//! This module owns the daemon side of that story:
//!
//! - **Delivery is data** (ADR-0041 §2): a `runbook` configuration's TOML is
//!   parsed with the ordinary loader and enters the library as a Candidate
//!   with cloud provenance. A malformed delivery loads nothing. A
//!   re-delivery of the same name supersedes the previous candidate.
//! - **Gates are earned locally, from evidence that already exists**:
//!   [`evaluation_evidence`] checks the runbook's trigger against recorded
//!   episodes (deterministic, fail-closed on an empty history), and
//!   [`simulation_evidence`] simulates every allowed action through the risk
//!   engine (Infeasible rollback or Destructive risk fails). Validation is
//!   recorded only when the procedure actually runs and validates in
//!   operation ([`RunbookManager::on_procedure_validated`]); policy follows
//!   it, in ladder order. No gate is ever recorded out of order or without
//!   its evidence — the ladder's own [`argus_runbooks::GateError`] has the
//!   final say.
//! - **Approval and promotion are operator acts**: [`RunbookManager::decide`]
//!   crosses the decision (cloud control message or CLI) into the local
//!   ladder, whose existing errors reject out-of-order decisions.
//!
//! Delivered candidates persist (spec 009 FR-002) and reload beside the
//! directory-loaded ones at startup; every mutation re-persists, and a
//! persistence failure degrades downward — logged, never higher.
//!
//! Lock discipline: the library lock is always taken before the delivered
//! map's lock, and neither is held across an `await`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use argus_domain::{AuthorizationRequest, CapabilityDescriptor, PolicyOutcome};
use argus_memory::Episode;
use argus_policy::PolicyEvaluator;
use argus_runbooks::{
    DeliveredRunbook, Gate, Runbook, RunbookLibrary, RunbookStatus, RunbookTrigger, parse_runbook,
};
use argus_state::DomainRepository;
use chrono::{DateTime, Utc};
use serde_json::{Map, Value};
use uuid::Uuid;

/// The largest runbook document a delivery may carry (spec 009 FR-001 size
/// bound). A runbook is a procedure, not a payload; bigger content is a
/// mistake worth refusing, not truncating.
pub const MAX_RUNBOOK_TOML_BYTES: usize = 64 * 1024;

/// The cloud provenance of a delivered candidate: the configuration version
/// that delivered it (spec 009 Design Notes — the audit story the runbook
/// card shows).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Provenance {
    pub configuration_id: Uuid,
    pub version_id: Uuid,
    pub version_number: i64,
    pub delivered_at: DateTime<Utc>,
}

impl Provenance {
    fn to_json(&self) -> Map<String, Value> {
        serde_json::json!({
            "configuration_id": self.configuration_id.to_string(),
            "version_id": self.version_id.to_string(),
            "version_number": self.version_number,
            "delivered_at": self.delivered_at.to_rfc3339(),
        })
        .as_object()
        .cloned()
        .unwrap_or_default()
    }
}

/// The result of one delivery review: which technical gates were recorded,
/// and why the rest were not — logged, never fabricated (spec 009 FR-003).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryReport {
    pub name: String,
    /// The gates recorded at review, in ladder order.
    pub recorded: Vec<Gate>,
    /// `(gate, reason)` for each technical gate that was not recorded.
    pub withheld: Vec<(Gate, String)>,
}

/// The local runbook library behind the delivery, gate, and decision paths.
///
/// The library sits behind a lock so delivery (cloud session task), the
/// opportunistic gate hook (brain loop), and decisions (control messages and
/// IPC) can mutate it without a restart. Directory-loaded runbooks keep their
/// exact startup behavior (spec 009 out-of-scope note); every mutating path
/// here starts from a *delivered* candidate.
pub struct RunbookManager {
    library: RwLock<RunbookLibrary>,
    /// The delivery record per delivered name — provenance plus the copy
    /// persisted across restarts. Directory-loaded runbooks are absent.
    delivered: Mutex<HashMap<String, DeliveredRunbook>>,
    repository: Arc<dyn DomainRepository>,
    /// Serializes every mutate-then-persist sequence (`deliver`, `decide`,
    /// `record_earned_gate`): an interleaved superseding delivery must never
    /// let a stale library snapshot be persisted under newer provenance.
    order: tokio::sync::Mutex<()>,
}

impl RunbookManager {
    /// A manager over an already-built library (tests and callers that load
    /// persisted rows themselves); [`Self::load`] is the production path.
    pub fn new(library: RunbookLibrary, repository: Arc<dyn DomainRepository>) -> Self {
        Self {
            library: RwLock::new(library),
            delivered: Mutex::new(HashMap::new()),
            repository,
            order: tokio::sync::Mutex::new(()),
        }
    }

    /// Loads the startup library: directory-loaded runbooks first (the
    /// spec-005 behavior, unchanged), then persisted deliveries merged over
    /// them — cloud supersedes file, the same precedence the managed settings
    /// use. A corrupt or unreadable row is skipped with a log; a persistence
    /// failure leaves the dir-loaded library only (fail downward, AC-006).
    pub async fn load(
        config: &crate::config::DaemonConfig,
        repository: Arc<dyn DomainRepository>,
    ) -> Arc<Self> {
        let library = match &config.brain.runbooks_dir {
            Some(dir) => {
                let result = argus_runbooks::load_runbooks(std::path::Path::new(dir));
                tracing::info!("{}", result.summary());
                result.library
            }
            None => RunbookLibrary::default(),
        };

        let manager = Self::new(library, repository);

        match manager.repository.list_runbooks().await {
            Ok(deliveries) => {
                for record in deliveries {
                    let name = record.name().to_string();
                    let mut library = manager
                        .library
                        .write()
                        .expect("runbook library lock is not poisoned");
                    library.remove(&name);
                    if let Err(reason) = library.register(record.runbook.clone()) {
                        tracing::warn!(name = %name, %reason, "persisted delivered runbook skipped");
                        continue;
                    }
                    manager
                        .delivered
                        .lock()
                        .expect("delivered map lock is not poisoned")
                        .insert(name, record);
                }
                let count = manager
                    .delivered
                    .lock()
                    .expect("delivered map lock is not poisoned")
                    .len();
                if count > 0 {
                    tracing::info!("{count} delivered runbook(s) restored from persistence");
                }
            }
            Err(error) => {
                tracing::warn!(
                    %error,
                    "delivered runbooks could not be loaded; the library holds directory-loaded \
                     runbooks only"
                );
            }
        }

        Arc::new(manager)
    }

    /// Every runbook, ordered by name (clones — the caller never holds the
    /// library lock).
    pub fn list(&self) -> Vec<Runbook> {
        self.library
            .read()
            .expect("runbook library lock is not poisoned")
            .list()
            .into_iter()
            .cloned()
            .collect()
    }

    /// The named runbook, when present.
    pub fn by_name(&self, name: &str) -> Option<Runbook> {
        self.library
            .read()
            .expect("runbook library lock is not poisoned")
            .by_name(name)
            .cloned()
    }

    /// The cloud provenance of a delivered runbook, when it has one.
    pub fn provenance(&self, name: &str) -> Option<Provenance> {
        self.delivered
            .lock()
            .expect("delivered map lock is not poisoned")
            .get(name)
            .map(|record| Provenance {
                configuration_id: record.configuration_id,
                version_id: record.version_id,
                version_number: record.version_number,
                delivered_at: record.delivered_at,
            })
    }

    /// Delivers one runbook document (spec 009 FR-001/FR-003): parse with the
    /// ordinary loader, review the technical gates against the evidence the
    /// caller supplies, persist, then supersede into the library. Malformed
    /// content loads nothing (`Err` carries the loader reason). Persistence
    /// failure loads nothing either — an undurable delivery is not an applied
    /// one.
    pub async fn deliver(
        &self,
        toml_text: &str,
        configuration_id: Uuid,
        version_id: Uuid,
        version_number: i64,
        episodes: &[Episode],
        descriptors: &[CapabilityDescriptor],
    ) -> Result<DeliveryReport, String> {
        // One ordered operation: gate review, persist, and supersede may not
        // interleave with a decision or another delivery.
        let _order = self.order.lock().await;

        if toml_text.len() > MAX_RUNBOOK_TOML_BYTES {
            return Err(format!(
                "the runbook document is {} bytes; the delivery bound is {MAX_RUNBOOK_TOML_BYTES}",
                toml_text.len()
            ));
        }
        // Fail-closed on the loader's verdict — the same parse a file would
        // walk. A delivery never carries earned status (ADR-0041 §2): the
        // loader replays declared gates through the ladder, so a document
        // declaring them would land pre-promoted — the exact alternative
        // ADR-0041 rejected. Gates are earned here or not at all.
        let mut runbook = parse_runbook(toml_text)?;
        if !runbook.gates().is_empty() {
            return Err("a delivered runbook cannot declare gates; it earns them here".to_string());
        }

        // Delivery review (FR-003): evaluation then simulation, in ladder
        // order, recorded only when the evidence agrees. A withheld gate is
        // logged with its reason — the candidate ships gateless, never
        // fabricated upward.
        let mut recorded = Vec::new();
        let mut withheld = Vec::new();
        for (gate, evidence) in [
            (
                Gate::Evaluation,
                evaluation_evidence(&runbook, episodes).err(),
            ),
            (
                Gate::Simulation,
                simulation_evidence(&runbook, descriptors).err(),
            ),
        ] {
            // `None` evidence means the check passed; the ladder still has
            // the final word on the order.
            match evidence {
                None => match runbook.record_gate(gate) {
                    Ok(()) => recorded.push(gate),
                    Err(error) => withheld.push((gate, error.to_string())),
                },
                Some(reason) => {
                    tracing::info!(
                        runbook = %runbook.name,
                        gate = ?gate,
                        %reason,
                        "delivery review withheld a gate"
                    );
                    withheld.push((gate, reason));
                }
            }
        }

        let delivered_at = Utc::now();
        let record = DeliveredRunbook {
            runbook: runbook.clone(),
            configuration_id,
            version_id,
            version_number,
            delivered_at,
        };

        // Persist before registering: an undurable delivery is refused, so
        // memory and disk never disagree about what was accepted.
        if let Err(error) = self.repository.put_runbook(&record).await {
            let reason = format!("could not persist the delivered runbook: {error}");
            tracing::warn!(runbook = %runbook.name, %reason);
            return Err(reason);
        }

        {
            let mut library = self
                .library
                .write()
                .expect("runbook library lock is not poisoned");
            // Supersede: a re-delivery of the same name replaces the previous
            // candidate version (FR-001).
            library.remove(&runbook.name);
            library
                .register(runbook.clone())
                .map_err(|reason| format!("the delivered runbook was refused: {reason}"))?;
        }
        self.delivered
            .lock()
            .expect("delivered map lock is not poisoned")
            .insert(runbook.name.clone(), record);

        tracing::info!(
            runbook = %runbook.name,
            configuration_id = %configuration_id,
            version = version_number,
            gates = ?recorded,
            "runbook delivered as a candidate with cloud provenance"
        );

        Ok(DeliveryReport {
            name: runbook.name,
            recorded,
            withheld,
        })
    }

    /// An operator decision crossing into the local ladder (spec 009 FR-004):
    /// `approve` (Candidate → Approved, requires the four technical gates) or
    /// `promote` (Approved → Promoted). The ladder's own errors are the
    /// validation — out-of-order or repeated decisions are refused with them,
    /// never coerced. Only delivered runbooks participate: a directory-loaded
    /// runbook is file-owned, and a decision on it would be silently reverted
    /// at the next restart. Delivered runbooks re-persist; a persistence
    /// failure degrades downward (the ladder state stands, logged).
    pub async fn decide(
        &self,
        name: &str,
        decision: Decision,
        decided_by: &str,
    ) -> Result<&'static str, String> {
        // One ordered operation: the ladder mutation and its persistence may
        // not interleave with a delivery or another decision.
        let _order = self.order.lock().await;

        if !self
            .delivered
            .lock()
            .expect("delivered map lock is not poisoned")
            .contains_key(name)
        {
            return Err(
                "only delivered runbooks participate in the ladder; this one is file-owned"
                    .to_string(),
            );
        }

        let outcome = {
            let mut library = self
                .library
                .write()
                .expect("runbook library lock is not poisoned");
            let runbook = library
                .by_name_mut(name)
                .ok_or_else(|| format!("no runbook named '{name}' is loaded"))?;
            match decision {
                Decision::Approve => runbook.approve().map(|_| "approved"),
                Decision::Promote => runbook.promote().map(|_| "promoted"),
            }
        };

        match outcome {
            Ok(status) => {
                tracing::info!(runbook = %name, decision = ?decision, decided_by = %decided_by, status = %status);
                self.audit_decision(name, decision, decided_by, status, None)
                    .await;
                self.persist_delivered(name).await;
                Ok(status)
            }
            Err(error) => {
                let reason = error.to_string();
                tracing::info!(runbook = %name, decision = ?decision, decided_by = %decided_by, %reason, "runbook decision refused by the ladder");
                self.audit_decision(name, decision, decided_by, "refused", Some(&reason))
                    .await;
                Err(reason)
            }
        }
    }

    /// The audit trail for an operator decision (spec 009 FR-004, AC-004):
    /// recorded for refusals too — the ladder's rejections are part of the
    /// story. A failed audit write is logged, never an error upward.
    async fn audit_decision(
        &self,
        name: &str,
        decision: Decision,
        decided_by: &str,
        status: &str,
        refusal: Option<&str>,
    ) {
        let event = argus_domain::DomainEvent::new(
            Uuid::new_v4(),
            argus_domain::EventType::new("runbook.decided")
                .expect("runbook.decided is a valid event type"),
            Utc::now(),
            "argusd",
            name,
            argus_domain::Severity::Info,
            None,
            None,
            serde_json::json!({
                "decision": match decision {
                    Decision::Approve => "approve",
                    Decision::Promote => "promote",
                },
                "decided_by": decided_by,
                "status": status,
                "refusal": refusal,
            }),
        );
        if let Err(error) = self.repository.put_audit_event(&event).await {
            tracing::warn!(runbook = %name, %error, "the runbook decision could not be audited");
        }
    }

    /// The opportunistic Validation/Policy hook (spec 009 FR-003, AC-003):
    /// called when the brain's remediation *for this trigger* executed and
    /// validated (the caller passes only completed runs — the evidence).
    /// Delivered candidates matching the trigger earn the validation gate,
    /// then the policy gate, in ladder order. Attribution is the author's
    /// own contract: a candidate that declares no `[[validation]]` criteria
    /// cannot be attributed a validated run and is left alone. A candidate
    /// missing earlier gates is rejected by the ladder and waits — never
    /// guessed. Only delivered candidates participate: directory-loaded
    /// runbooks keep their unchanged spec-004 behavior (spec 009
    /// out-of-scope note).
    pub async fn on_procedure_validated(
        &self,
        trigger: &RunbookTrigger,
        descriptors: &[CapabilityDescriptor],
        policy: &dyn PolicyEvaluator,
    ) {
        let names: Vec<String> = {
            let library = self
                .library
                .read()
                .expect("runbook library lock is not poisoned");
            library
                .matching(trigger)
                .into_iter()
                .filter(|runbook| runbook.status() == RunbookStatus::Candidate)
                .filter(|runbook| !runbook.validation.is_empty())
                .filter(|runbook| {
                    self.delivered
                        .lock()
                        .expect("delivered map lock is not poisoned")
                        .contains_key(&runbook.name)
                })
                .map(|runbook| runbook.name.clone())
                .collect()
        };

        for name in names {
            // The run validated in operation: live attempt history for this
            // candidate, independent of whether any gate lands.
            self.record_outcome(&name, true).await;

            // Validation: the run just supplied the evidence.
            self.record_earned_gate(&name, Gate::Validation).await;

            // Policy, next on the ladder and only when validation stands:
            // the candidate's capabilities must pass the policy evaluator.
            let verdict = {
                let library = self
                    .library
                    .read()
                    .expect("runbook library lock is not poisoned");
                match library.by_name(&name) {
                    Some(runbook) if runbook.has_reached(Gate::Validation) => {
                        Some(policy_compatible(runbook, descriptors, policy).err())
                    }
                    _ => None,
                }
            };
            match verdict {
                Some(None) => self.record_earned_gate(&name, Gate::Policy).await,
                Some(Some(reason)) => tracing::info!(
                    runbook = %name,
                    %reason,
                    "policy gate withheld: the candidate capabilities are not policy-compatible"
                ),
                None => {}
            }
        }
    }

    /// Records one validated run into a delivered candidate's attempt history
    /// (attempts and success rate) and re-persists it. Locking: callers hold
    /// the ordering mutex.
    async fn record_outcome(&self, name: &str, success: bool) {
        {
            let mut library = self
                .library
                .write()
                .expect("runbook library lock is not poisoned");
            match library.by_name_mut(name) {
                Some(runbook) => runbook.record_outcome(success),
                None => return,
            }
        }
        self.persist_delivered(name).await;
    }

    /// Records one earned gate for a delivered candidate, re-persisting the
    /// ladder progress. Ladder rejections and persistence failures log and
    /// degrade downward — the runbook never moves higher by accident.
    async fn record_earned_gate(&self, name: &str, gate: Gate) {
        // One ordered operation: the mutation and its persistence may not
        // interleave with a delivery or a decision.
        let _order = self.order.lock().await;

        let outcome = {
            let mut library = self
                .library
                .write()
                .expect("runbook library lock is not poisoned");
            match library.by_name_mut(name) {
                Some(runbook) => runbook.record_gate(gate),
                None => return,
            }
        };
        match outcome {
            Ok(()) => {
                tracing::info!(runbook = %name, gate = ?gate, "promotion gate recorded");
                self.persist_delivered(name).await;
            }
            Err(error) => {
                tracing::debug!(runbook = %name, gate = ?gate, %error, "gate not recorded");
            }
        }
    }

    /// Re-persists a delivered candidate's current ladder state. No-op for
    /// directory-loaded runbooks (file-owned) and a logged degradation when
    /// the store fails — never an error upward.
    async fn persist_delivered(&self, name: &str) {
        let runbook = {
            let library = self
                .library
                .read()
                .expect("runbook library lock is not poisoned");
            library.by_name(name).cloned()
        };
        let Some(runbook) = runbook else {
            return;
        };
        let record = {
            let mut delivered = self
                .delivered
                .lock()
                .expect("delivered map lock is not poisoned");
            let Some(record) = delivered.get_mut(name) else {
                return;
            };
            record.runbook = runbook;
            record.clone()
        };
        if let Err(error) = self.repository.put_runbook(&record).await {
            tracing::warn!(
                runbook = %name,
                %error,
                "delivered runbook state could not be re-persisted; progress stays in memory"
            );
        }
    }

    /// The additive sentinel field (spec 009 FR-005): one entry per runbook —
    /// name, trigger, status, gates, provenance, attempt history. `None` when
    /// the library is empty, so the lean build's snapshots are byte-identical
    /// to before (skip-if-none, the 008 pattern).
    pub fn view(&self) -> Option<Map<String, Value>> {
        let items: Vec<Value> = {
            let library = self
                .library
                .read()
                .expect("runbook library lock is not poisoned");
            if library.is_empty() {
                return None;
            }
            library
                .list()
                .into_iter()
                .map(|runbook| {
                    let provenance = {
                        self.delivered
                            .lock()
                            .expect("delivered map lock is not poisoned")
                            .get(&runbook.name)
                            .map(|record| Provenance {
                                configuration_id: record.configuration_id,
                                version_id: record.version_id,
                                version_number: record.version_number,
                                delivered_at: record.delivered_at,
                            })
                    };
                    let mut entry = serde_json::json!({
                        "name": runbook.name,
                        "trigger": runbook.trigger,
                        "status": runbook.status(),
                        "gates": runbook.gates(),
                        "attempts": runbook.attempts(),
                        "success_rate": runbook.historical_success_rate(),
                    });
                    if let Some(provenance) = provenance
                        && let Some(object) = entry.as_object_mut()
                    {
                        object.insert("provenance".into(), Value::Object(provenance.to_json()));
                    }
                    entry
                })
                .collect()
        };
        let count = items.len();
        serde_json::json!({ "items": items, "count": count })
            .as_object()
            .cloned()
    }
}

/// An operator decision on a runbook candidate (spec 009 FR-004).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Candidate → Approved; requires the four technical gates.
    Approve,
    /// Approved → Promoted; the runbook then drives procedures.
    Promote,
}

impl Decision {
    /// Parses the wire spelling carried by `runbook.decision` and the CLI.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "approve" => Some(Self::Approve),
            "promote" => Some(Self::Promote),
            _ => None,
        }
    }
}

/// The Evaluation gate's evidence check (spec 009 FR-003): the host's
/// recorded episode history corroborates the procedure. Deterministic and
/// fail-closed — a host with no episodes fails honestly, a trigger with no
/// matching resolved episode fails honestly, and nothing is ever inferred.
///
/// The deterministic match: the runbook's trigger signature equals an
/// episode's recorded symptom, that episode ended `Resolved`, and it carries
/// the remediation that resolved it. That is "steps against recorded episodes
/// via the runbooks criterion" without inventing a connection the memory
/// layer does not hold.
pub fn evaluation_evidence(runbook: &Runbook, episodes: &[Episode]) -> Result<(), String> {
    if episodes.is_empty() {
        return Err(
            "this host has no recorded episodes; the evaluation gate needs local evidence"
                .to_string(),
        );
    }

    let signature = match &runbook.trigger {
        RunbookTrigger::Symptom(symptom) => symptom.trim().to_lowercase(),
        RunbookTrigger::Signal(signal) => signal.trim().to_lowercase(),
        RunbookTrigger::Manual => {
            return Err(
                "a manual runbook has no trigger signature to evaluate against recorded episodes"
                    .to_string(),
            );
        }
    };

    let corroborating = episodes
        .iter()
        .filter(|episode| {
            episode.symptom.trim().to_lowercase() == signature
                && episode.outcome == argus_memory::EpisodeOutcome::Resolved
                && episode.remediation.is_some()
        })
        .count();

    if corroborating == 0 {
        return Err(format!(
            "no recorded episode with symptom '{signature}' ended resolved with a remediation; \
             the evaluation gate has no local evidence"
        ));
    }
    Ok(())
}

/// The Simulation gate's evidence check (spec 009 FR-003): every allowed
/// action is simulated through the risk engine (CAP-19), assembled from the
/// capability registry's own descriptors — the runbook widens nothing.
/// Acceptance: every action simulates, no rollback is Infeasible, and no
/// action is Destructive. An unregistered capability fails closed.
pub fn simulation_evidence(
    runbook: &Runbook,
    descriptors: &[CapabilityDescriptor],
) -> Result<(), String> {
    let subject = argus_domain::ResourceId::new("host", "local")
        .expect("the local host resource id is valid");

    for action in &runbook.allowed_actions {
        let Some(descriptor) = descriptors
            .iter()
            .find(|descriptor| descriptor.id() == action)
        else {
            return Err(format!(
                "candidate action '{}' is not in the capability registry; the simulation gate \
                 refuses what it cannot simulate",
                action.as_str()
            ));
        };

        let input = argus_risk::SimulationInput {
            capability: action.clone(),
            subject: subject.clone(),
            risk: descriptor.risk_class(),
            reversibility: descriptor.reversibility(),
            blast_radius: descriptor.effective_blast_radius(),
            dependents: Vec::new(),
            declared_recovery_time_seconds: None,
            declared_rollback: runbook.rollback.first().cloned(),
            alternative: None,
        };
        let estimate = argus_risk::simulate_impact(&input);
        if descriptor.risk_class() == argus_domain::RiskClass::Destructive {
            return Err(format!(
                "candidate action '{}' is Destructive; the simulation gate refuses it",
                action.as_str()
            ));
        }
        if estimate.rollback == argus_risk::RollbackFeasibility::Infeasible {
            return Err(format!(
                "candidate action '{}' simulates with an Infeasible rollback; the simulation gate \
                 refuses it",
                action.as_str()
            ));
        }
    }
    Ok(())
}

/// The Policy gate's evidence check (spec 009 FR-003): every candidate
/// capability is policy-compatible — nothing the evaluator denies outright.
/// A `RequireApproval` verdict stays compatible: the runbook's procedure
/// would cross the ordinary approval machinery like any other plan.
pub fn policy_compatible(
    runbook: &Runbook,
    descriptors: &[CapabilityDescriptor],
    policy: &dyn PolicyEvaluator,
) -> Result<(), String> {
    let subject = argus_domain::ResourceId::new("host", "local")
        .expect("the local host resource id is valid");
    let context = argus_domain::RequestContext::new(
        Uuid::new_v4(),
        semver::Version::new(0, 0, 0),
        argus_domain::Principal::new(None, None),
        Utc::now(),
    );

    for action in &runbook.allowed_actions {
        let Some(descriptor) = descriptors
            .iter()
            .find(|descriptor| descriptor.id() == action)
        else {
            return Err(format!(
                "candidate action '{}' is not in the capability registry; the policy gate refuses \
                 what it cannot classify",
                action.as_str()
            ));
        };
        let request = AuthorizationRequest::new(
            argus_domain::CapabilityRequest::new(
                action.clone(),
                argus_domain::Principal::new(None, None),
                Some(subject.clone()),
                Value::Null,
                context.clone(),
            ),
            descriptor.risk_class(),
            descriptor.effective_blast_radius(),
        );
        let decision = policy.evaluate(&request);
        if decision.outcome == PolicyOutcome::Deny {
            return Err(format!(
                "candidate action '{}' is denied by policy ({}): {}",
                action.as_str(),
                decision.policy_id,
                decision.reason
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use argus_domain::{BlastRadius, CapabilityId, Reversibility, RiskClass};
    use argus_memory::EpisodeOutcome;
    use argus_policy::BootstrapPolicyEvaluator;
    use argus_state::RepositoryError;
    use semver::Version;

    fn restart_descriptor() -> CapabilityDescriptor {
        CapabilityDescriptor::new(
            CapabilityId::new("host.service.restart").unwrap(),
            "argusd",
            "host.service.restart",
            RiskClass::LowRisk,
            Version::new(0, 1, 0),
            serde_json::json!({}),
            serde_json::json!({}),
            Reversibility::Reversible,
        )
        .with_blast_radius(BlastRadius::Host)
    }

    fn cordon_descriptor() -> CapabilityDescriptor {
        CapabilityDescriptor::new(
            CapabilityId::new("k8s.node.cordon").unwrap(),
            "argusd",
            "k8s.node.cordon",
            RiskClass::LowRisk,
            Version::new(0, 1, 0),
            serde_json::json!({}),
            serde_json::json!({}),
            Reversibility::None,
        )
        .with_blast_radius(BlastRadius::Host)
    }

    fn pod_delete_descriptor() -> CapabilityDescriptor {
        CapabilityDescriptor::new(
            CapabilityId::new("k8s.pod.delete").unwrap(),
            "argusd",
            "k8s.pod.delete",
            RiskClass::Destructive,
            Version::new(0, 1, 0),
            serde_json::json!({}),
            serde_json::json!({}),
            Reversibility::None,
        )
        .with_blast_radius(BlastRadius::Environment)
    }

    fn runbook(trigger: RunbookTrigger, actions: Vec<&str>) -> Runbook {
        Runbook::candidate(
            Uuid::new_v4(),
            "learned-procedure",
            trigger,
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

    fn episode(symptom: &str, outcome: EpisodeOutcome, remediation: Option<&str>) -> Episode {
        Episode {
            id: Uuid::new_v4(),
            subject: argus_domain::ResourceId::new("service", "api").unwrap(),
            symptom: symptom.to_string(),
            resource_class: "service".to_string(),
            root_cause: None,
            remediation: remediation.map(str::to_string),
            outcome,
            change_proximity: None,
            started_at: Utc::now(),
            resolved_at: None,
        }
    }

    // --- Evaluation evidence ---

    #[test]
    fn evaluation_passes_on_a_resolved_matching_episode() {
        let rb = runbook(
            RunbookTrigger::Symptom("restart-loop".into()),
            vec!["host.service.restart"],
        );
        let episodes = vec![
            episode("oom-killed", EpisodeOutcome::Unresolved, Some("restart")),
            episode(
                "restart-loop",
                EpisodeOutcome::Resolved,
                Some("restart nginx"),
            ),
        ];
        assert_eq!(evaluation_evidence(&rb, &episodes), Ok(()));
    }

    #[test]
    fn evaluation_fails_closed_on_an_empty_episode_history() {
        let rb = runbook(
            RunbookTrigger::Symptom("restart-loop".into()),
            vec!["host.service.restart"],
        );
        let reason = evaluation_evidence(&rb, &[]).unwrap_err();
        assert!(reason.contains("no recorded episodes"), "{reason}");
    }

    #[test]
    fn evaluation_fails_when_no_episode_matches_the_trigger() {
        let rb = runbook(
            RunbookTrigger::Symptom("disk-pressure".into()),
            vec!["host.service.restart"],
        );
        let episodes = vec![episode(
            "restart-loop",
            EpisodeOutcome::Resolved,
            Some("restart"),
        )];
        let reason = evaluation_evidence(&rb, &episodes).unwrap_err();
        assert!(reason.contains("disk-pressure"), "{reason}");
    }

    #[test]
    fn evaluation_demands_a_resolved_outcome_with_a_remediation() {
        let rb = runbook(
            RunbookTrigger::Symptom("restart-loop".into()),
            vec!["host.service.restart"],
        );
        // Recurred: the remediation did not hold.
        let recurred = vec![episode(
            "restart-loop",
            EpisodeOutcome::Recurred,
            Some("restart"),
        )];
        assert!(evaluation_evidence(&rb, &recurred).is_err());
        // Resolved but never recorded what resolved it.
        let unexplained = vec![episode("restart-loop", EpisodeOutcome::Resolved, None)];
        assert!(evaluation_evidence(&rb, &unexplained).is_err());
    }

    #[test]
    fn evaluation_fails_honestly_for_manual_and_signal_triggers() {
        let manual = runbook(RunbookTrigger::Manual, vec![]);
        let reason = evaluation_evidence(
            &manual,
            &[episode("x", EpisodeOutcome::Resolved, Some("y"))],
        )
        .unwrap_err();
        assert!(reason.contains("manual"), "{reason}");

        let signal = runbook(RunbookTrigger::Signal("disk.used_percent".into()), vec![]);
        assert!(
            evaluation_evidence(
                &signal,
                &[episode("x", EpisodeOutcome::Resolved, Some("y"))]
            )
            .is_err()
        );
    }

    // --- Simulation evidence ---

    #[test]
    fn simulation_passes_when_every_action_is_registered_and_reversible() {
        let rb = runbook(
            RunbookTrigger::Symptom("restart-loop".into()),
            vec!["host.service.restart"],
        );
        assert_eq!(simulation_evidence(&rb, &[restart_descriptor()]), Ok(()));
    }

    #[test]
    fn simulation_fails_on_an_unregistered_capability() {
        let rb = runbook(
            RunbookTrigger::Symptom("restart-loop".into()),
            vec!["host.container.exec"],
        );
        let reason = simulation_evidence(&rb, &[restart_descriptor()]).unwrap_err();
        assert!(
            reason.contains("not in the capability registry"),
            "{reason}"
        );
    }

    #[test]
    fn simulation_fails_on_a_destructive_action() {
        let rb = runbook(
            RunbookTrigger::Symptom("restart-loop".into()),
            vec!["host.service.restart", "k8s.pod.delete"],
        );
        let reason =
            simulation_evidence(&rb, &[restart_descriptor(), pod_delete_descriptor()]).unwrap_err();
        assert!(reason.contains("Destructive"), "{reason}");
    }

    #[test]
    fn simulation_fails_on_an_infeasible_rollback() {
        // Registered, not Destructive, but irreversible with no declared
        // rollback: the estimate is Infeasible.
        let rb = runbook(
            RunbookTrigger::Symptom("restart-loop".into()),
            vec!["k8s.node.cordon"],
        );
        let reason = simulation_evidence(&rb, &[cordon_descriptor()]).unwrap_err();
        assert!(reason.contains("Infeasible"), "{reason}");
    }

    #[test]
    fn simulation_passes_vacuously_when_the_runbook_declares_no_actions() {
        // A runbook that authorizes nothing has nothing to simulate; it also
        // grants nothing. Deterministic, and honest in both directions.
        let rb = runbook(RunbookTrigger::Symptom("restart-loop".into()), vec![]);
        assert_eq!(simulation_evidence(&rb, &[]), Ok(()));
    }

    // --- Policy compatibility ---

    #[test]
    fn policy_compatible_passes_permitted_capabilities_and_fails_denied_ones() {
        let permitted = runbook(
            RunbookTrigger::Symptom("restart-loop".into()),
            vec!["host.service.restart"],
        );
        assert_eq!(
            policy_compatible(
                &permitted,
                &[restart_descriptor()],
                &BootstrapPolicyEvaluator::with_local_remediation()
            ),
            Ok(())
        );

        let denied = runbook(
            RunbookTrigger::Symptom("restart-loop".into()),
            vec!["host.service.restart", "k8s.pod.delete"],
        );
        let reason = policy_compatible(
            &denied,
            &[restart_descriptor(), pod_delete_descriptor()],
            &BootstrapPolicyEvaluator::with_local_remediation(),
        )
        .unwrap_err();
        assert!(reason.contains("k8s.pod.delete"), "{reason}");
    }

    // --- The manager ---

    async fn manager() -> (Arc<RunbookManager>, Arc<argus_state::SqliteRepository>) {
        let repository = Arc::new(argus_state::SqliteRepository::open_in_memory().unwrap());
        let config = crate::config::DaemonConfig::default();
        let manager = RunbookManager::load(
            &config,
            Arc::clone(&repository) as Arc<dyn DomainRepository>,
        )
        .await;
        (manager, repository)
    }

    fn valid_toml(name: &str) -> String {
        format!(
            r#"
name = "{name}"
trigger = {{ symptom = "restart-loop" }}
allowed_actions = ["host.service.restart"]
rollback = ["host.service.restart"]

[[validation]]
description = "unit active again"
attribute = "unit.active_state"
comparison = {{ equal = "active" }}
"#
        )
    }

    /// A deliverable runbook that declares no `[[validation]]` criteria.
    fn valid_toml_without_validation(name: &str) -> String {
        format!(
            r#"
name = "{name}"
trigger = {{ symptom = "restart-loop" }}
allowed_actions = ["host.service.restart"]
rollback = ["host.service.restart"]
"#
        )
    }

    #[tokio::test]
    async fn a_valid_delivery_lands_a_candidate_with_provenance_and_supersedes() {
        let (manager, repository) = manager().await;
        let episodes = vec![episode(
            "restart-loop",
            EpisodeOutcome::Resolved,
            Some("restart"),
        )];
        let descriptors = vec![restart_descriptor()];

        let report = manager
            .deliver(
                &valid_toml("learned-procedure"),
                Uuid::new_v4(),
                Uuid::new_v4(),
                3,
                &episodes,
                &descriptors,
            )
            .await
            .unwrap();

        assert_eq!(report.name, "learned-procedure");
        assert_eq!(
            report.recorded,
            vec![Gate::Evaluation, Gate::Simulation],
            "both technical gates pass on corroborating episodes and a reversible action"
        );
        assert!(report.withheld.is_empty());

        let delivered = manager.by_name("learned-procedure").unwrap();
        assert_eq!(delivered.status(), RunbookStatus::Candidate);
        assert_eq!(delivered.gates(), &[Gate::Evaluation, Gate::Simulation]);
        let provenance = manager.provenance("learned-procedure").unwrap();
        assert_eq!(provenance.version_number, 3);

        // Supersede: a re-delivery of the same name replaces the candidate.
        let report = manager
            .deliver(
                &valid_toml("learned-procedure"),
                Uuid::new_v4(),
                Uuid::new_v4(),
                4,
                &episodes,
                &descriptors,
            )
            .await
            .unwrap();
        assert_eq!(manager.list().len(), 1, "superseded, not duplicated");
        assert_eq!(
            manager
                .provenance("learned-procedure")
                .unwrap()
                .version_number,
            4
        );
        assert!(report.recorded.contains(&Gate::Evaluation));

        // The delivery persisted (FR-002), carrying the recorded gates.
        let persisted = repository.list_runbooks().await.unwrap();
        assert_eq!(persisted.len(), 1);
        assert_eq!(persisted[0].version_number, 4);
        assert_eq!(persisted[0].runbook.gates(), delivered.gates());
    }

    #[tokio::test]
    async fn a_failing_simulation_leaves_the_candidate_with_only_its_earned_gates() {
        let (manager, _repository) = manager().await;
        let episodes = vec![episode(
            "restart-loop",
            EpisodeOutcome::Resolved,
            Some("restart"),
        )];
        // The candidate action simulates Infeasible (irreversible, no rollback).
        let toml = valid_toml("risky-procedure").replace("host.service.restart", "k8s.node.cordon");

        let report = manager
            .deliver(
                &toml,
                Uuid::new_v4(),
                Uuid::new_v4(),
                1,
                &episodes,
                &[cordon_descriptor()],
            )
            .await
            .unwrap();

        assert_eq!(report.recorded, vec![Gate::Evaluation]);
        assert_eq!(report.withheld.len(), 1);
        assert_eq!(report.withheld[0].0, Gate::Simulation);
        assert!(report.withheld[0].1.contains("Infeasible"));
        assert_eq!(
            manager.by_name("risky-procedure").unwrap().gates(),
            &[Gate::Evaluation],
            "the candidate carries exactly the gates its evidence earned"
        );
    }

    #[tokio::test]
    async fn a_malformed_delivery_loads_nothing_and_carries_the_loader_reason() {
        let (manager, _repository) = manager().await;
        let error = manager
            .deliver("name = ", Uuid::new_v4(), Uuid::new_v4(), 1, &[], &[])
            .await
            .unwrap_err();
        assert!(error.contains("invalid runbook file"), "{error}");
        assert!(manager.list().is_empty(), "nothing loaded");
    }

    #[tokio::test]
    async fn an_oversized_delivery_is_refused_before_parsing() {
        let (manager, _repository) = manager().await;
        let body = format!("name = \"x\"\n#{}", "a".repeat(MAX_RUNBOOK_TOML_BYTES));
        let error = manager
            .deliver(&body, Uuid::new_v4(), Uuid::new_v4(), 1, &[], &[])
            .await
            .unwrap_err();
        assert!(error.contains("delivery bound"), "{error}");
    }

    #[tokio::test]
    async fn a_delivery_without_local_evidence_is_gateless_not_fabricated() {
        let (manager, _repository) = manager().await;
        // No episodes at all: both technical gates fail honestly.
        let report = manager
            .deliver(
                &valid_toml("fresh-procedure"),
                Uuid::new_v4(),
                Uuid::new_v4(),
                1,
                &[],
                &[restart_descriptor()],
            )
            .await
            .unwrap();
        assert!(report.recorded.is_empty());
        assert_eq!(report.withheld.len(), 2);
        assert!(report.withheld[0].1.contains("no recorded episodes"));
        assert_eq!(manager.by_name("fresh-procedure").unwrap().gates(), &[]);
    }

    #[tokio::test]
    async fn gates_survive_a_restart_beside_directory_loaded_runbooks() {
        let repository = Arc::new(argus_state::SqliteRepository::open_in_memory().unwrap());
        let episodes = vec![episode(
            "restart-loop",
            EpisodeOutcome::Resolved,
            Some("restart"),
        )];
        let descriptors = vec![restart_descriptor()];

        // First boot with a runbooks directory.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("file-loaded.toml"),
            valid_toml("file-loaded"),
        )
        .unwrap();
        let mut config = crate::config::DaemonConfig::default();
        config.brain.runbooks_dir = Some(dir.path().to_string_lossy().into_owned());
        let first = RunbookManager::load(
            &config,
            Arc::clone(&repository) as Arc<dyn DomainRepository>,
        )
        .await;
        first
            .deliver(
                &valid_toml("delivered"),
                Uuid::new_v4(),
                Uuid::new_v4(),
                2,
                &episodes,
                &descriptors,
            )
            .await
            .unwrap();

        // Second boot: the delivered candidate and its gates reload beside the
        // directory-loaded one (AC-006).
        let second = RunbookManager::load(&config, repository).await;
        assert_eq!(second.list().len(), 2);
        let delivered = second.by_name("delivered").unwrap();
        assert_eq!(delivered.gates(), &[Gate::Evaluation, Gate::Simulation]);
        assert!(second.provenance("delivered").is_some());
        assert!(second.provenance("file-loaded").is_none());
    }

    #[tokio::test]
    async fn decisions_cross_the_local_ladder_and_reject_out_of_order() {
        let (manager, repository) = manager().await;
        let episodes = vec![episode(
            "restart-loop",
            EpisodeOutcome::Resolved,
            Some("restart"),
        )];
        let descriptors = vec![restart_descriptor()];
        let policy = BootstrapPolicyEvaluator::with_local_remediation();
        manager
            .deliver(
                &valid_toml("learned-procedure"),
                Uuid::new_v4(),
                Uuid::new_v4(),
                1,
                &episodes,
                &descriptors,
            )
            .await
            .unwrap();

        // Promotion before approval is refused with the ladder's error.
        let error = manager
            .decide("learned-procedure", Decision::Promote, "uid=0")
            .await
            .unwrap_err();
        assert!(error.contains("approval"), "{error}");

        // Approval before validation+policy is refused.
        let error = manager
            .decide("learned-procedure", Decision::Approve, "uid=0")
            .await
            .unwrap_err();
        assert!(
            error.contains("requires the evaluation"),
            "the ladder's own NotReadyForApproval wording: {error}"
        );

        // The honest path: validation in operation, then policy, then the
        // operator's approve → promote.
        manager
            .on_procedure_validated(
                &RunbookTrigger::Symptom("restart-loop".into()),
                &descriptors,
                &policy,
            )
            .await;
        assert_eq!(
            manager.by_name("learned-procedure").unwrap().gates(),
            &[
                Gate::Evaluation,
                Gate::Simulation,
                Gate::Validation,
                Gate::Policy
            ]
        );
        // The validated run is live history now, not dead zeros.
        let runbook = manager.by_name("learned-procedure").unwrap();
        assert_eq!(runbook.attempts(), 1);
        assert_eq!(runbook.historical_success_rate(), Some(1.0));
        assert_eq!(
            manager
                .decide("learned-procedure", Decision::Approve, "uid=0")
                .await
                .unwrap(),
            "approved"
        );
        assert_eq!(
            manager
                .decide("learned-procedure", Decision::Promote, "uid=0")
                .await
                .unwrap(),
            "promoted"
        );
        assert_eq!(
            manager.by_name("learned-procedure").unwrap().status(),
            RunbookStatus::Promoted
        );

        // A repeat decision is refused (idempotent by name+decision).
        let error = manager
            .decide("learned-procedure", Decision::Promote, "uid=0")
            .await
            .unwrap_err();
        assert!(error.contains("already"), "{error}");

        // An unknown name is refused.
        assert!(
            manager
                .decide("no-such-runbook", Decision::Approve, "uid=0")
                .await
                .is_err()
        );

        // The decisions re-persisted the promoted state (AC-006).
        let persisted = repository.list_runbooks().await.unwrap();
        assert_eq!(persisted[0].runbook.status(), RunbookStatus::Promoted);

        // And the result is audited — the promotion and the refusal both
        // left their trail (AC-004, FR-004).
        let audit = repository.list_audit_events().await.unwrap();
        let decided: Vec<_> = audit
            .iter()
            .filter(|event| event.event_type().as_str() == "runbook.decided")
            .collect();
        // Two out-of-order refusals, the approve, the promote, and the
        // repeat-promote refusal: every decision left its trail.
        assert_eq!(
            decided.len(),
            5,
            "{:?}",
            decided.iter().map(|e| e.payload()).collect::<Vec<_>>()
        );
        let payloads: Vec<_> = decided
            .iter()
            .map(|event| event.payload().clone())
            .collect();
        assert!(
            payloads
                .iter()
                .any(|p| p["decision"] == "promote" && p["status"] == "promoted"),
            "{payloads:?}"
        );
        assert!(
            payloads
                .iter()
                .any(|p| p["decision"] == "promote" && p["status"] == "refused"),
            "the refusal is audited too: {payloads:?}"
        );
    }

    #[tokio::test]
    async fn the_opportunistic_hook_waits_when_earlier_gates_are_missing() {
        let (manager, _repository) = manager().await;
        // No episodes: the candidate is gateless.
        manager
            .deliver(
                &valid_toml("gateless"),
                Uuid::new_v4(),
                Uuid::new_v4(),
                1,
                &[],
                &[restart_descriptor()],
            )
            .await
            .unwrap();
        let descriptors = vec![restart_descriptor()];
        manager
            .on_procedure_validated(
                &RunbookTrigger::Symptom("restart-loop".into()),
                &descriptors,
                &BootstrapPolicyEvaluator::with_local_remediation(),
            )
            .await;
        assert_eq!(
            manager.by_name("gateless").unwrap().gates(),
            &[],
            "validation without its prerequisites is rejected by the ladder — the gates wait"
        );
    }

    #[tokio::test]
    async fn the_opportunistic_hook_touches_delivered_candidates_only() {
        // A directory-loaded runbook at the same trigger.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file.toml"), valid_toml("file-loaded")).unwrap();
        let mut config = crate::config::DaemonConfig::default();
        config.brain.runbooks_dir = Some(dir.path().to_string_lossy().into_owned());
        let repository = Arc::new(argus_state::SqliteRepository::open_in_memory().unwrap());
        let manager = RunbookManager::load(
            &config,
            Arc::clone(&repository) as Arc<dyn DomainRepository>,
        )
        .await;

        let descriptors = vec![restart_descriptor()];
        manager
            .on_procedure_validated(
                &RunbookTrigger::Symptom("restart-loop".into()),
                &descriptors,
                &BootstrapPolicyEvaluator::with_local_remediation(),
            )
            .await;
        assert_eq!(
            manager.by_name("file-loaded").unwrap().gates(),
            &[],
            "directory-loaded runbooks keep their unchanged spec-004 behavior"
        );
        // A decision on a file-owned runbook is refused: it would be silently
        // reverted at the next restart (only delivered runbooks re-persist).
        let error = manager
            .decide("file-loaded", Decision::Approve, "uid=0")
            .await
            .unwrap_err();
        assert!(error.contains("file-owned"), "{error}");
        assert_eq!(
            manager.by_name("file-loaded").unwrap().status(),
            RunbookStatus::Candidate,
            "the refusal left the runbook untouched"
        );
    }

    #[tokio::test]
    async fn a_delivery_declaring_gates_is_refused_wholesale() {
        let (manager, repository) = manager().await;
        // The full ladder declared in the document: the loader would replay
        // it into a Promoted runbook — delivery is not promotion (ADR-0041).
        let toml = r#"
name = "pre-promoted"
trigger = { symptom = "restart-loop" }
allowed_actions = ["host.service.restart"]
rollback = ["host.service.restart"]
gates = ["evaluation", "simulation", "validation", "policy", "approval", "promotion"]

[[validation]]
description = "unit active again"
attribute = "unit.active_state"
comparison = { equal = "active" }
"#;
        let error = manager
            .deliver(
                toml,
                Uuid::new_v4(),
                Uuid::new_v4(),
                1,
                &[episode(
                    "restart-loop",
                    EpisodeOutcome::Resolved,
                    Some("restart"),
                )],
                &[restart_descriptor()],
            )
            .await
            .unwrap_err();
        assert!(
            error.contains("cannot declare gates"),
            "the refusal names the rule: {error}"
        );
        assert!(manager.by_name("pre-promoted").is_none(), "nothing landed");
        assert!(manager.list().is_empty());
        assert!(
            repository.list_runbooks().await.unwrap().is_empty(),
            "the refused delivery persisted nothing"
        );
    }

    #[tokio::test]
    async fn the_opportunistic_hook_requires_declared_validation_criteria() {
        let (manager, _repository) = manager().await;
        // Validation criteria are the author's own attribution contract: a
        // candidate that declares none cannot be attributed a validated run.
        let toml = valid_toml_without_validation("no-criteria");
        manager
            .deliver(
                &toml,
                Uuid::new_v4(),
                Uuid::new_v4(),
                1,
                &[episode(
                    "restart-loop",
                    EpisodeOutcome::Resolved,
                    Some("restart"),
                )],
                &[restart_descriptor()],
            )
            .await
            .unwrap();
        let descriptors = vec![restart_descriptor()];
        manager
            .on_procedure_validated(
                &RunbookTrigger::Symptom("restart-loop".into()),
                &descriptors,
                &BootstrapPolicyEvaluator::with_local_remediation(),
            )
            .await;
        let runbook = manager.by_name("no-criteria").unwrap();
        assert_eq!(
            runbook.gates(),
            &[Gate::Evaluation, Gate::Simulation],
            "no declared validation criteria: no opportunistic gates"
        );
        assert_eq!(
            runbook.attempts(),
            0,
            "an unattributable candidate invents no history"
        );
    }

    #[tokio::test]
    async fn the_view_is_skip_if_none_and_carries_status_gates_provenance() {
        let (manager, _repository) = manager().await;
        assert!(manager.view().is_none(), "skip-if-none on an empty library");

        let episodes = vec![episode(
            "restart-loop",
            EpisodeOutcome::Resolved,
            Some("restart"),
        )];
        manager
            .deliver(
                &valid_toml("learned-procedure"),
                Uuid::new_v4(),
                Uuid::new_v4(),
                9,
                &episodes,
                &[restart_descriptor()],
            )
            .await
            .unwrap();

        let view = manager.view().unwrap();
        assert_eq!(view["count"], 1);
        let item = view["items"].as_array().unwrap()[0].clone();
        assert_eq!(item["name"], "learned-procedure");
        assert_eq!(item["status"], "candidate");
        assert_eq!(
            item["gates"],
            serde_json::json!(["evaluation", "simulation"])
        );
        assert_eq!(item["provenance"]["version_number"], 9);
    }

    /// A store that forwards everything but can be told to fail `put_runbook`,
    /// so the degraded-persistence path is exercised without a corrupted
    /// database.
    struct FlakyRunbookStore {
        inner: Arc<dyn DomainRepository>,
        fail_puts: std::sync::atomic::AtomicBool,
    }

    #[async_trait::async_trait]
    impl DomainRepository for FlakyRunbookStore {
        async fn save_environment(
            &self,
            id: &argus_domain::EnvironmentId,
        ) -> Result<(), RepositoryError> {
            self.inner.save_environment(id).await
        }

        async fn get_environment(
            &self,
        ) -> Result<Option<argus_domain::EnvironmentId>, RepositoryError> {
            self.inner.get_environment().await
        }

        async fn put_observation(
            &self,
            observation: &argus_domain::Observation,
        ) -> Result<(), RepositoryError> {
            self.inner.put_observation(observation).await
        }

        async fn list_observations(
            &self,
        ) -> Result<Vec<argus_domain::Observation>, RepositoryError> {
            self.inner.list_observations().await
        }

        async fn put_audit_event(
            &self,
            event: &argus_domain::DomainEvent,
        ) -> Result<(), RepositoryError> {
            self.inner.put_audit_event(event).await
        }

        async fn save_health(
            &self,
            health: &argus_domain::HealthStatus,
        ) -> Result<(), RepositoryError> {
            self.inner.save_health(health).await
        }

        async fn get_health(&self) -> Result<Option<argus_domain::HealthStatus>, RepositoryError> {
            self.inner.get_health().await
        }

        async fn put_runbook(&self, runbook: &DeliveredRunbook) -> Result<(), RepositoryError> {
            if self.fail_puts.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(RepositoryError::Failed("store unavailable".into()));
            }
            self.inner.put_runbook(runbook).await
        }

        async fn list_runbooks(&self) -> Result<Vec<DeliveredRunbook>, RepositoryError> {
            self.inner.list_runbooks().await
        }
    }

    #[tokio::test]
    async fn a_decision_persistence_failure_degrades_downward() {
        let sqlite = Arc::new(argus_state::SqliteRepository::open_in_memory().unwrap());
        let store = Arc::new(FlakyRunbookStore {
            inner: sqlite as Arc<dyn DomainRepository>,
            fail_puts: std::sync::atomic::AtomicBool::new(false),
        });
        let config = crate::config::DaemonConfig::default();
        let manager =
            RunbookManager::load(&config, Arc::clone(&store) as Arc<dyn DomainRepository>).await;
        let episodes = vec![episode(
            "restart-loop",
            EpisodeOutcome::Resolved,
            Some("restart"),
        )];
        manager
            .deliver(
                &valid_toml("learned-procedure"),
                Uuid::new_v4(),
                Uuid::new_v4(),
                1,
                &episodes,
                &[restart_descriptor()],
            )
            .await
            .unwrap();
        manager
            .on_procedure_validated(
                &RunbookTrigger::Symptom("restart-loop".into()),
                &[restart_descriptor()],
                &BootstrapPolicyEvaluator::with_local_remediation(),
            )
            .await;

        // Break persistence underneath the manager: decisions still move the
        // ladder; the re-persist failure is logged, never raised (spec 009
        // NFR: fail-closed, never higher).
        store
            .fail_puts
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            manager
                .decide("learned-procedure", Decision::Approve, "uid=0")
                .await
                .unwrap(),
            "approved"
        );
        assert_eq!(
            manager.by_name("learned-procedure").unwrap().status(),
            RunbookStatus::Approved
        );
    }

    #[tokio::test]
    async fn a_delivery_persistence_failure_loads_nothing() {
        let sqlite = Arc::new(argus_state::SqliteRepository::open_in_memory().unwrap());
        let store = Arc::new(FlakyRunbookStore {
            inner: sqlite as Arc<dyn DomainRepository>,
            fail_puts: std::sync::atomic::AtomicBool::new(true),
        });
        let config = crate::config::DaemonConfig::default();
        let manager =
            RunbookManager::load(&config, Arc::clone(&store) as Arc<dyn DomainRepository>).await;
        let error = manager
            .deliver(
                &valid_toml("undurable"),
                Uuid::new_v4(),
                Uuid::new_v4(),
                1,
                &[episode(
                    "restart-loop",
                    EpisodeOutcome::Resolved,
                    Some("restart"),
                )],
                &[restart_descriptor()],
            )
            .await
            .unwrap_err();
        assert!(error.contains("could not persist"), "{error}");
        assert!(
            manager.by_name("undurable").is_none(),
            "an undurable delivery is not a loaded one"
        );
    }
}
