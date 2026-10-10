//! The daemon's ledger sink (spec 007): every record is persisted locally at
//! full fidelity and, when upload is enabled, a redacted projection lands in
//! the bounded upstream buffer the cloud session flushes.
//!
//! Two invariants shape this module:
//!
//! 1. **Emission never blocks or fails an execution.** The sink methods log
//!    persistence failures and return; the only await is the upstream
//!    buffer's bounded capacity wait, which is the mechanism that keeps
//!    action events from being dropped under a sustained burst (FR-005).
//! 2. **Raw stays local.** The wire payloads are built through
//!    `argus_cloud::mapping::redact`, and a payload that fails to serialize
//!    is dropped, never uploaded raw (FR-004).
//!
//! The cycle context is how brain cycles chain their records without
//! signature churn (spec 007 design notes): the brain opens a frame at cycle
//! entry; action/usage records emitted while it is open are stamped with the
//! cycle id, and the plan correlations it observed are returned at exit so
//! the cycle trace can link to its actions.

use std::sync::Arc;

use argus_cloud::buffer::{LedgerBuffer, UsageSum};
use argus_domain::{
    ActionEventRecord, BrainTraceRecord, DomainEvent, EventType, Severity, TokenUsageRecord,
};
use argus_events::{EventBus, LedgerSink, LocalEventBus};
use argus_state::DomainRepository;
use chrono::Utc;
use serde_json::{Map, Value};
use tokio::sync::Mutex;
use uuid::Uuid;

use argus_cloud::mapping::redact as mapping_redact;

/// One open brain cycle: actions and usage observed during it carry its id,
/// and the plan correlations it observed are collected for the trace.
struct CycleFrame {
    cycle_id: Uuid,
    plan_ids: Vec<Uuid>,
}

/// The daemon-side [`LedgerSink`].
pub struct DaemonLedger {
    repository: Arc<dyn DomainRepository>,
    /// The upstream redacted buffer; the cloud session drains it. Shared so
    /// the supervisor can flush without touching the sink.
    buffer: Arc<Mutex<LedgerBuffer>>,
    upload_enabled: bool,
    retention_days: i64,
    events: Arc<LocalEventBus>,
    cycle: std::sync::Mutex<Vec<CycleFrame>>,
}

impl DaemonLedger {
    pub fn new(
        repository: Arc<dyn DomainRepository>,
        upload_enabled: bool,
        retention_days: i64,
    ) -> Arc<Self> {
        Arc::new(Self {
            repository,
            buffer: Arc::new(Mutex::new(LedgerBuffer::new())),
            upload_enabled,
            retention_days,
            events: Arc::new(LocalEventBus::new(64)),
            cycle: std::sync::Mutex::new(Vec::new()),
        })
    }

    /// The upstream buffer, for the cloud session's flush step.
    pub fn buffer(&self) -> Arc<Mutex<LedgerBuffer>> {
        Arc::clone(&self.buffer)
    }

    /// The bus the ledger publishes its own event vocabulary on (brain.trace,
    /// token.usage); local subscribers see the summary shapes only.
    pub fn events(&self) -> Arc<LocalEventBus> {
        Arc::clone(&self.events)
    }

    /// Marks the start of a brain cycle; records emitted until the matching
    /// [`Self::exit_cycle`] are stamped with its id.
    pub async fn enter_cycle(&self, cycle_id: Uuid) {
        self.cycle
            .lock()
            .expect("cycle context is not poisoned")
            .push(CycleFrame {
                cycle_id,
                plan_ids: Vec::new(),
            });
    }

    /// Closes a brain cycle, returning the plan correlations it caused so the
    /// cycle trace can link reasoning to actions.
    pub async fn exit_cycle(&self, cycle_id: Uuid) -> Vec<Uuid> {
        let mut cycles = self.cycle.lock().expect("cycle context is not poisoned");
        if let Some(position) = cycles.iter().position(|frame| frame.cycle_id == cycle_id) {
            let frame = cycles.remove(position);
            frame.plan_ids
        } else {
            Vec::new()
        }
    }

    /// Drops ledger rows older than the configured retention (bounded growth).
    pub async fn prune(&self) {
        if let Err(error) = self.repository.retain_ledger(self.retention_days).await {
            tracing::warn!(%error, "ledger retention prune failed");
        }
    }

    fn open_cycle(&self) -> Option<Uuid> {
        let cycles = self.cycle.lock().expect("cycle context is not poisoned");
        cycles.last().map(|frame| frame.cycle_id)
    }

    fn stamp_plan(&self, cycle_id: Uuid, plan_id: Uuid) {
        let mut cycles = self.cycle.lock().expect("cycle context is not poisoned");
        if let Some(frame) = cycles.last_mut()
            && frame.cycle_id == cycle_id
            && !frame.plan_ids.contains(&plan_id)
        {
            frame.plan_ids.push(plan_id);
        }
    }

    /// Builds the redacted wire payload for an action event; `None` when
    /// nothing survives serialization — which drops the upload, never leaks
    /// (FR-004). Fields are truncated to the cloud contract's bounds, so an
    /// over-long capability or target is clipped rather than rejected whole.
    fn action_payload(event: &ActionEventRecord, cycle_id: Option<Uuid>) -> Option<Value> {
        use argus_cloud::protocol::messages::{MAX_LEDGER_KIND_LEN, MAX_LEDGER_TEXT_LEN};
        let args: Option<Map<String, Value>> = match event.args {
            Value::Object(ref map) if !map.is_empty() => mapping_redact::redact_value(&event.args)
                .as_object()
                .cloned(),
            _ => None,
        };
        let validation = event.validation.as_ref().map(mapping_redact::redact_value);
        let payload = ActionEventWire {
            event_id: event.event_id,
            correlation_id: event.correlation_id,
            causation_id: event.causation_id,
            cycle_id: cycle_id.or(event.cycle_id),
            plan_id: event.plan_id,
            kind: bound_text(&event.kind, MAX_LEDGER_KIND_LEN),
            target: event
                .target
                .as_deref()
                .map(|target| bound_text(target, MAX_LEDGER_TEXT_LEN)),
            args,
            verdict: event.verdict.clone(),
            policy_id: event
                .policy_id
                .as_deref()
                .map(|id| bound_text(id, MAX_LEDGER_KIND_LEN)),
            outcome: event.outcome.clone(),
            duration_ms: event.duration_ms,
            validation,
            occurred_at: event.occurred_at,
        };
        serde_json::to_value(&payload).ok()
    }

    /// Builds the redacted wire payload for a trace, with every field
    /// truncated to the cloud contract's bounds — an over-bound cycle is
    /// clipped to fit, never rejected whole.
    fn trace_payload(trace: &BrainTraceRecord) -> Option<Value> {
        use argus_cloud::protocol::messages::{
            MAX_LEDGER_EVIDENCE, MAX_LEDGER_STEPS, MAX_LEDGER_TEXT_LEN,
        };
        let redact =
            |text: &str| mapping_redact::redact_text(&bound_text(text, MAX_LEDGER_TEXT_LEN));
        let evidence: Vec<String> = trace
            .evidence
            .iter()
            .take(MAX_LEDGER_EVIDENCE)
            .map(|line| redact(line))
            .collect();
        let steps: Vec<String> = trace
            .steps
            .iter()
            .take(MAX_LEDGER_STEPS)
            .map(|line| redact(line))
            .collect();
        let payload = BrainTraceWire {
            trace_id: trace.trace_id,
            cycle_id: trace.cycle_id,
            plan_id: trace.plan_id,
            evidence,
            decision: trace.decision.as_deref().map(redact),
            objective: trace.objective.as_deref().map(redact),
            steps,
            outcome: trace.outcome.as_deref().map(redact),
            coalesced_before: 0,
            occurred_at: trace.occurred_at,
        };
        serde_json::to_value(&payload).ok()
    }
}

/// Clips a free-text field to `max` characters (char boundaries, not bytes).
fn bound_text(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

#[async_trait::async_trait]
impl LedgerSink for DaemonLedger {
    async fn record_action(&self, mut event: ActionEventRecord) {
        // Stamp the open cycle (if any) and remember the plan linkage.
        if let Some(cycle_id) = self.open_cycle() {
            event.cycle_id = Some(cycle_id);
            if let Some(plan_id) = event.plan_id {
                self.stamp_plan(cycle_id, plan_id);
            }
        }

        // 1. Local source of truth, raw and append-only (FR-001).
        if let Err(error) = self.repository.put_action_event(&event).await {
            tracing::warn!(%error, event_id = %event.event_id, "action event not persisted");
        }

        // 2. Upstream, redacted and bounded (FR-004, FR-005). The buffer lock
        // is taken only for the attempt — never across the capacity wait — so
        // the flusher can drain and free room while this producer retries.
        if self.upload_enabled
            && let Some(payload) = Self::action_payload(&event, event.cycle_id)
        {
            let deadline = tokio::time::Instant::now() + argus_cloud::buffer::LEDGER_PUSH_WAIT;
            loop {
                let outcome = {
                    let mut buffer = self.buffer.lock().await;
                    buffer.try_push_action(payload.clone(), Utc::now())
                }; // guard released here, before any sleep
                match outcome {
                    Ok(dropped) => {
                        if !dropped.is_empty() {
                            tracing::warn!(
                                count = dropped.len(),
                                "upstream ledger buffer gave way under a sustained burst"
                            );
                        }
                        break;
                    }
                    Err(_) => {
                        if tokio::time::Instant::now() >= deadline {
                            // Last resort after the bounded wait: make room,
                            // counted; the record is safe in the local ledger.
                            let dropped = {
                                let mut buffer = self.buffer.lock().await;
                                let dropped = buffer.sacrifice_actions(
                                    argus_cloud::buffer::LEDGER_ACTION_CAPACITY / 4,
                                );
                                // Push while still holding the lock: room exists now.
                                let _ = buffer.try_push_action(payload.clone(), Utc::now());
                                dropped
                            };
                            if !dropped.is_empty() {
                                tracing::warn!(
                                    count = dropped.len(),
                                    "upstream ledger buffer overflowed after the bounded wait;                                      oldest action events gave way"
                                );
                            }
                            break;
                        }
                        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                    }
                }
            }
        }
    }

    async fn record_trace(&self, trace: BrainTraceRecord) {
        if let Err(error) = self.repository.put_brain_trace(&trace).await {
            tracing::warn!(%error, trace_id = %trace.trace_id, "brain trace not persisted");
        }
        // The summary event rides the local bus too (safe fields only), so
        // local subscribers see the cycle vocabulary without the payloads.
        let _ = self
            .events
            .publish(&DomainEvent::new(
                trace.trace_id,
                EventType::new(argus_events::types::BRAIN_TRACE)
                    .expect("brain.trace is a valid event type"),
                trace.occurred_at,
                "argusd",
                "argusd",
                Severity::Info,
                trace.cycle_id,
                trace.plan_id,
                serde_json::json!({
                    "cycle_id": trace.cycle_id,
                    "plan_id": trace.plan_id,
                    "objective": trace.objective,
                    "outcome": trace.outcome,
                    "evidence_count": trace.evidence.len(),
                    "steps": trace.steps.len(),
                }),
            ))
            .await;
        if self.upload_enabled
            && let Some(payload) = Self::trace_payload(&trace)
        {
            self.buffer.lock().await.push_trace(payload, Utc::now());
        }
    }

    async fn record_usage(&self, mut usage: TokenUsageRecord) {
        if usage.cycle_id.is_none()
            && let Some(cycle_id) = self.open_cycle()
        {
            usage.cycle_id = Some(cycle_id);
        }
        if let Err(error) = self.repository.put_token_usage(&usage).await {
            tracing::warn!(%error, "token usage not persisted");
        }
        let _ = self
            .events
            .publish(&DomainEvent::new(
                Uuid::new_v4(),
                EventType::new(argus_events::types::TOKEN_USAGE)
                    .expect("token.usage is a valid event type"),
                usage.occurred_at,
                "argusd",
                "argusd",
                Severity::Info,
                usage.cycle_id,
                None,
                serde_json::json!({
                    "model": usage.model,
                    // Absent stays absent: unknown is the honest reading (FR-003).
                    "prompt_tokens": usage.prompt_tokens,
                    "completion_tokens": usage.completion_tokens,
                    "total_tokens": usage.total_tokens,
                    "duration_ms": usage.duration_ms,
                }),
            ))
            .await;
        if self.upload_enabled {
            let known = usage.prompt_tokens.is_some()
                || usage.completion_tokens.is_some()
                || usage.total_tokens.is_some();
            self.buffer.lock().await.push_usage(UsageSum {
                // The batch's stable identity: retries re-batch under the same
                // id, so a lost-ack retry dedups at the cloud instead of
                // double-counting (FR-005).
                usage_id: Uuid::new_v4(),
                cycle_id: usage.cycle_id,
                model: usage.model,
                prompt_tokens: usage.prompt_tokens,
                completion_tokens: usage.completion_tokens,
                total_tokens: usage.total_tokens,
                calls: 1,
                unknown_usage: u32::from(!known),
                duration_ms: usage.duration_ms.unwrap_or(0),
            });
        }
    }
}

/// The wire shapes (mirroring `argus_cloud::protocol::messages`) — restated
/// here only as the serialization target for the daemon's payload builders.
/// Field spellings are the contract's; drift is a cloud rejection, not a
/// cosmetic difference.
use argus_cloud::protocol::messages::{
    ActionEventPayload as ActionEventWire, BrainTracePayload as BrainTraceWire,
};

#[cfg(test)]
mod tests {
    use super::*;
    use argus_state::InMemoryRepository;

    fn record() -> ActionEventRecord {
        ActionEventRecord {
            event_id: Uuid::new_v4(),
            correlation_id: Uuid::new_v4(),
            causation_id: None,
            cycle_id: None,
            plan_id: Some(Uuid::new_v4()),
            kind: "host.service.restart".into(),
            target: Some("nginx.service".into()),
            args: serde_json::json!({ "unit": "nginx.service", "DEPLOY_TOKEN": "sk-secret" }),
            verdict: "allow".into(),
            policy_id: Some("bootstrap.remediation".into()),
            outcome: "ok".into(),
            duration_ms: Some(12),
            validation: None,
            occurred_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn actions_persist_raw_locally_and_buffer_redacted_upstream() {
        let ledger = DaemonLedger::new(Arc::new(InMemoryRepository::new()), true, 30);
        let event = record();
        ledger.record_action(event.clone()).await;

        let stored = ledger
            .repository
            .list_action_events(&Default::default())
            .await
            .unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(
            stored[0].args["DEPLOY_TOKEN"], "sk-secret",
            "local fidelity keeps the raw value (AC-005)"
        );

        let payload = ledger.buffer.lock().await.drain(Utc::now()).remove(0);
        assert_eq!(payload.kind, argus_cloud::buffer::ReportKind::ActionLedger);
        assert_eq!(
            payload.payload["args"]["DEPLOY_TOKEN"],
            argus_cloud::mapping::redact::REDACTED,
            "the upload is redacted (AC-005)"
        );
        assert_eq!(payload.payload["args"]["unit"], "nginx.service");
        assert_eq!(payload.payload["verdict"], "allow");
    }

    #[tokio::test]
    async fn records_inside_a_cycle_carry_its_id_and_feed_the_plan_linkage() {
        let ledger = DaemonLedger::new(Arc::new(InMemoryRepository::new()), false, 30);
        let cycle = Uuid::new_v4();
        ledger.enter_cycle(cycle).await;

        let event = record();
        ledger.record_action(event.clone()).await;
        let plans = ledger.exit_cycle(cycle).await;

        let stored = ledger
            .repository
            .list_action_events(&Default::default())
            .await
            .unwrap();
        assert_eq!(
            stored[0].cycle_id,
            Some(cycle),
            "the cycle stamps its actions"
        );
        assert_eq!(
            plans,
            vec![event.plan_id.unwrap()],
            "the trace learns its plan id"
        );
    }

    #[tokio::test]
    async fn an_over_bound_trace_is_truncated_to_the_contract_not_rejected() {
        let trace = BrainTraceRecord {
            trace_id: Uuid::new_v4(),
            cycle_id: Some(Uuid::new_v4()),
            plan_id: None,
            evidence: (0..80)
                .map(|i| format!("e{i}: {}", "x".repeat(3000)))
                .collect(),
            decision: Some("d".repeat(5000)),
            objective: Some("restore the thing".into()),
            steps: (0..60).map(|i| format!("cap{i}")).collect(),
            outcome: Some("Completed".into()),
            occurred_at: Utc::now(),
        };
        let payload = DaemonLedger::trace_payload(&trace).expect("payload builds");
        use argus_cloud::protocol::messages::{
            MAX_LEDGER_EVIDENCE, MAX_LEDGER_STEPS, MAX_LEDGER_TEXT_LEN,
        };
        assert_eq!(
            payload["evidence"].as_array().unwrap().len(),
            MAX_LEDGER_EVIDENCE,
            "evidence clipped to the contract bound"
        );
        assert_eq!(payload["steps"].as_array().unwrap().len(), MAX_LEDGER_STEPS);
        for field in ["decision", "objective", "outcome"] {
            let text = payload[field].as_str().unwrap();
            assert!(text.chars().count() <= MAX_LEDGER_TEXT_LEN, "{field}");
        }
        assert!(
            payload["evidence"][0].as_str().unwrap().chars().count() <= MAX_LEDGER_TEXT_LEN,
            "each evidence line is clipped"
        );
    }

    #[tokio::test]
    async fn a_disabled_upload_persists_without_buffering() {
        let ledger = DaemonLedger::new(Arc::new(InMemoryRepository::new()), false, 30);
        ledger.record_action(record()).await;
        assert!(ledger.buffer.lock().await.is_empty());
        assert_eq!(
            ledger
                .repository
                .list_action_events(&Default::default())
                .await
                .unwrap()
                .len(),
            1,
            "the ledger is always local, upload or not"
        );
    }
}
