//! The ledger sink: where action/trace/usage records are handed off at the
//! policy/executor chokepoints (spec 007).
//!
//! Emission is fire-and-forget by contract: sink methods return nothing and
//! never fail, because a ledger write must never block or fail an execution
//! (spec 007 always-constraints). The daemon wires a sink that persists each
//! record locally at full fidelity and buffers a redacted copy for upload;
//! [`NoopLedgerSink`] is the fail-safe default for callers (and tests) that
//! carry no ledger.

use argus_domain::{ActionEventRecord, BrainTraceRecord, TokenUsageRecord};
use async_trait::async_trait;

#[async_trait]
pub trait LedgerSink: Send + Sync {
    /// Records one execution attempt (any verdict). Raw fields stay local;
    /// the implementation owns redaction before upload.
    async fn record_action(&self, event: ActionEventRecord);

    /// Records one bounded trace (a brain cycle, or a plan-run outcome).
    async fn record_trace(&self, trace: BrainTraceRecord);

    /// Records one provider call's token usage; missing counts stay unknown.
    async fn record_usage(&self, usage: TokenUsageRecord);
}

/// The no-op sink: records nothing, never fails. The default for the control
/// loop's ports, so every caller compiles without a ledger.
pub struct NoopLedgerSink;

#[async_trait]
impl LedgerSink for NoopLedgerSink {
    async fn record_action(&self, _event: ActionEventRecord) {}
    async fn record_trace(&self, _trace: BrainTraceRecord) {}
    async fn record_usage(&self, _usage: TokenUsageRecord) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn sample_action() -> ActionEventRecord {
        ActionEventRecord {
            event_id: uuid::Uuid::new_v4(),
            correlation_id: uuid::Uuid::new_v4(),
            causation_id: None,
            cycle_id: None,
            plan_id: None,
            kind: "host.service.restart".into(),
            target: Some("nginx.service".into()),
            args: serde_json::json!({ "unit": "nginx.service" }),
            verdict: argus_domain::VERDICT_ALLOW.into(),
            policy_id: Some("bootstrap.remediation".into()),
            outcome: argus_domain::OUTCOME_OK.into(),
            duration_ms: Some(12),
            validation: None,
            occurred_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn the_noop_sink_accepts_every_record_and_returns_nothing() {
        let sink = NoopLedgerSink;
        sink.record_action(sample_action()).await;
        sink.record_trace(BrainTraceRecord {
            trace_id: uuid::Uuid::new_v4(),
            cycle_id: None,
            plan_id: None,
            evidence: vec![],
            decision: None,
            objective: None,
            steps: vec![],
            outcome: None,
            occurred_at: Utc::now(),
        })
        .await;
        sink.record_usage(TokenUsageRecord {
            cycle_id: None,
            model: None,
            prompt_tokens: None,
            completion_tokens: None,
            total_tokens: None,
            duration_ms: None,
            occurred_at: Utc::now(),
        })
        .await;
    }
}
