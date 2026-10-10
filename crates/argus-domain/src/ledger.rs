//! The action ledger: append-only records of everything that crossed the
//! policy/executor boundary, every brain trace, and every token-usage
//! metering record (spec 007).
//!
//! These are the shared vocabulary for the whole nervous system: the daemon
//! emits them at the chokepoints, `argus-state` persists them at full local
//! fidelity, and `argus-cloud` projects them (redacted) onto the wire. They
//! carry explicit `cycle_id`/`plan_id` fields so reasoning → plan → actions →
//! validation reads back as one chain without threading ids through
//! signatures.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

// --- Verdict vocabulary (FR-001) ---

/// The policy allowed the action.
pub const VERDICT_ALLOW: &str = "allow";
/// The policy (or the governor) refused the action.
pub const VERDICT_DENY: &str = "deny";
/// The action needs an operator decision before it can run.
pub const VERDICT_REQUIRES_APPROVAL: &str = "requires_approval";

// --- Terminal outcome vocabulary (FR-001) ---

/// The action ran and succeeded (including the already-desired no-op).
pub const OUTCOME_OK: &str = "ok";
/// The action ran and failed, or a rollback step failed.
pub const OUTCOME_FAILED: &str = "failed";
/// The action took effect and was undone by its declarative rollback.
pub const OUTCOME_ROLLED_BACK: &str = "rolled_back";
/// The action was refused at the boundary; nothing executed.
pub const OUTCOME_DENIED: &str = "denied";
/// The action hit its execution timeout. (No daemon path emits it yet; the
/// vocabulary exists so a timeout outcome is representable end to end.)
pub const OUTCOME_TIMEOUT: &str = "timeout";
/// The action is paused awaiting an operator decision.
pub const OUTCOME_AWAITING_APPROVAL: &str = "awaiting_approval";

/// One execution attempt at the policy/executor boundary, regardless of
/// verdict (FR-001). Persisted append-only at full fidelity — including the
/// raw `args`, which never leave the host (FR-004 redacts the wire copy).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionEventRecord {
    pub event_id: Uuid,
    /// The plan run's correlation id — the same id the audit events carry.
    pub correlation_id: Uuid,
    pub causation_id: Option<Uuid>,
    /// The brain cycle that caused this action, when one did (stamped by the
    /// ledger while a cycle is open; `None` for CLI/cloud-initiated runs).
    pub cycle_id: Option<Uuid>,
    /// Explicit plan linkage so a trace's actions are queryable without
    /// signature churn (spec 007 design notes).
    pub plan_id: Option<Uuid>,
    /// The capability id, e.g. `host.service.restart`.
    pub kind: String,
    /// The unit / container / cgroup path / namespace-name the action names.
    pub target: Option<String>,
    /// Raw action arguments. Local fidelity only; the cloud receives the
    /// redacted projection.
    pub args: Value,
    /// One of the `VERDICT_*` values.
    pub verdict: String,
    /// The policy rule (or `governor`) that produced the verdict.
    pub policy_id: Option<String>,
    /// One of the `OUTCOME_*` values.
    pub outcome: String,
    pub duration_ms: Option<u64>,
    /// The post-execution validation outcome, when the action ran.
    pub validation: Option<Value>,
    pub occurred_at: DateTime<Utc>,
}

/// One bounded brain-cycle (or plan-run) trace (FR-002): evidence referenced
/// by summary — never full payloads — plus the decision, the plan objective,
/// and the outcome. `cycle_id` links a trace to the usage records and the
/// action events of the same cycle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BrainTraceRecord {
    pub trace_id: Uuid,
    pub cycle_id: Option<Uuid>,
    pub plan_id: Option<Uuid>,
    /// Evidence references (ids/counts), not payloads.
    pub evidence: Vec<String>,
    /// The decision summary.
    pub decision: Option<String>,
    pub objective: Option<String>,
    /// The plan's step list, summarized.
    pub steps: Vec<String>,
    /// The plan's terminal status, or `awaiting_approval` while paused.
    pub outcome: Option<String>,
    pub occurred_at: DateTime<Utc>,
}

/// Token usage for one provider call (FR-003). A missing provider `usage`
/// block leaves the counts `None` — unknown, never zero and never estimated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TokenUsageRecord {
    pub cycle_id: Option<Uuid>,
    pub model: Option<String>,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub duration_ms: Option<u64>,
    pub occurred_at: DateTime<Utc>,
}

/// The read filter behind `list_action_events` (FR-006's local twin): the
/// fields the ledger query actually narrows on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActionEventFilter {
    pub since: Option<DateTime<Utc>>,
    pub kind: Option<String>,
    /// Terminal-outcome filter (`OUTCOME_*` values).
    pub status: Option<String>,
    /// Maximum rows returned; zero is lifted to a default bound.
    pub limit: usize,
}

/// The default `limit` when a filter carries zero.
pub const DEFAULT_ACTION_EVENT_LIMIT: usize = 200;
