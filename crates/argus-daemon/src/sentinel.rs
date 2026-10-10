//! Sentinel mode (spec 003 M6, CAP-21/FR-022, CAP-22/FR-023, T033) and its
//! self-observability wiring (CAP-24/FR-025, T034).
//!
//! The sentinel is the continuous watch: one deterministic evaluation over
//! the daemon's live state — health, provider, pending approvals, open
//! incidents, active risks, labeled predictions, recent actions — producing
//! both the surfaced `SentinelView` (for the TUI/cloud) and the per-situation
//! escalation decision. The autonomous path reuses the same
//! policy → typed-executor → validation boundary as every other plan; the
//! sentinel never acquires a side channel.

use argus_domain::{
    Action, AutonomyMode, BlastRadius, Plan, PlanStep, PolicyOutcome, Reversibility, RiskClass,
    Severity,
};
use argus_observability::{DegradationThresholds, SafeMode, SelfHealth};
pub use argus_policy::Environment;
use argus_policy::{Escalation, EscalationInput, EvidenceQuality, decide_escalation};
use argus_risk::Risk;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

/// The environment-health rollup surfaced in the TUI/cloud (CAP-21).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentHealth {
    Healthy,
    Degraded,
    Critical,
}

/// The sentinel view: everything the 24/7 watch surfaces (FR-022). Every
/// field is supplied state; the sentinel adds nothing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SentinelView {
    pub environment_health: EnvironmentHealth,
    pub safe_mode: SafeModeSerde,
    pub provider_ready: bool,
    pub open_incidents: u32,
    pub active_risks: u32,
    pub pending_approvals: u32,
    /// Labeled prediction prose currently held (CAP-11 strings).
    pub predictions: Vec<String>,
    pub recent_actions: u32,
    /// Resource-pressure subjects with their worst severity.
    pub pressure: Vec<PressureSignal>,
    pub generated_at: DateTime<Utc>,
    /// The graduated-autonomy line (spec 008 FR-005): assimilation phase
    /// with gate progress, earned rung, ceiling, effective level, and the
    /// remaining blast-radius budgets. Additive and skip-if-none — the cloud
    /// validates the view loosely, and snapshots without a machine render
    /// exactly as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autonomy: Option<Map<String, Value>>,
    /// The runbook table (spec 009 FR-005): name, status, gates, provenance
    /// per candidate. Additive and skip-if-none like the autonomy line — a
    /// snapshot from a host with no runbooks renders exactly as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runbooks: Option<Map<String, Value>>,
}

/// One resource-pressure signal in the view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PressureSignal {
    pub subject: String,
    pub severity: Severity,
}

/// Serde-friendly mirror of [`SafeMode`] (the observability enum is plain).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SafeModeSerde {
    None,
    StaleInputs,
    ExecutionSuspended,
}

impl From<SafeMode> for SafeModeSerde {
    fn from(mode: SafeMode) -> Self {
        match mode {
            SafeMode::None => Self::None,
            SafeMode::StaleInputs => Self::StaleInputs,
            SafeMode::ExecutionSuspended => Self::ExecutionSuspended,
        }
    }
}

/// The typed inputs one sentinel evaluation runs on.
#[derive(Debug, Clone)]
pub struct SentinelInputs {
    pub autonomy: AutonomyMode,
    pub environment: Environment,
    pub provider_ready: bool,
    pub open_incidents: u32,
    pub risks: Vec<Risk>,
    pub predictions: Vec<String>,
    pub pending_approvals: u32,
    pub recent_actions: u32,
    pub self_health: SelfHealth,
    /// The escalation inputs for the situation under evaluation, when one is
    /// pending a decision.
    pub situation: Option<EscalationInput>,
    /// The autonomy machine's view map (spec 008 FR-005), when one is wired;
    /// carried through to the view verbatim — the sentinel adds nothing.
    /// (Named apart from `autonomy`, which is the escalation's level input.)
    pub autonomy_view: Option<Map<String, Value>>,
    /// The runbook table (spec 009 FR-005), when the library is non-empty;
    /// carried through to the view verbatim — the sentinel adds nothing.
    pub runbooks_view: Option<Map<String, Value>>,
}

/// One sentinel evaluation's outcome: the surfaced view and the decision.
#[derive(Debug, Clone, PartialEq)]
pub struct SentinelDecision {
    pub view: SentinelView,
    /// The escalation for `situation`, when inputs carried one.
    pub escalation: Option<Escalation>,
}

impl SentinelDecision {
    /// Whether the sentinel's decision includes an AUTO-FIX (the caller still
    /// executes through `run_remediation`; this is never itself authority).
    pub fn requests_autofix(&self) -> bool {
        self.escalation == Some(Escalation::AutoFix)
    }
}

/// Run one sentinel evaluation (deterministic; `now` is injected).
pub fn sentinel_evaluate(inputs: &SentinelInputs, now: DateTime<Utc>) -> SentinelDecision {
    let thresholds = DegradationThresholds::default();
    // Reconstruct enough of the self-health to decide the safe mode: the
    // sentinel consumes the snapshot, not the live counters.
    let safe_mode = safe_mode_from(&inputs.self_health, &thresholds);

    let environment_health = if safe_mode == SafeMode::ExecutionSuspended
        || inputs.open_incidents > 0 && inputs.risks.iter().any(|r| r.severity == Severity::Error)
    {
        EnvironmentHealth::Critical
    } else if !inputs.provider_ready
        || safe_mode != SafeMode::None
        || inputs.open_incidents > 0
        || inputs.risks.iter().any(|r| r.severity == Severity::Warning)
    {
        EnvironmentHealth::Degraded
    } else {
        EnvironmentHealth::Healthy
    };

    let pressure = inputs
        .risks
        .iter()
        .map(|r| PressureSignal {
            subject: r.subject.as_str().to_string(),
            severity: r.severity,
        })
        .collect();

    let view = SentinelView {
        environment_health,
        safe_mode: safe_mode.into(),
        provider_ready: inputs.provider_ready,
        open_incidents: inputs.open_incidents,
        active_risks: inputs.risks.len() as u32,
        pending_approvals: inputs.pending_approvals,
        predictions: inputs.predictions.clone(),
        recent_actions: inputs.recent_actions,
        pressure,
        generated_at: now,
        autonomy: inputs.autonomy_view.clone(),
        runbooks: inputs.runbooks_view.clone(),
    };

    // In a safe mode the escalation degrades: stale inputs or suspended
    // execution can never reach AUTO-FIX (CAP-24).
    let escalation = inputs.situation.as_ref().map(|situation| {
        let mut bounded = *situation;
        if safe_mode != SafeMode::None {
            bounded.policy = PolicyOutcome::RequireApproval;
        }
        decide_escalation(&bounded)
    });

    SentinelDecision { view, escalation }
}

/// Derive the safe mode from a health snapshot (mirror of
/// `SelfObservability::safe_mode` for snapshot consumers).
pub fn safe_mode_from(health: &SelfHealth, thresholds: &DegradationThresholds) -> SafeMode {
    if health.executor_errors > thresholds.max_executor_errors
        || health.failed_validations > thresholds.max_failed_validations
    {
        return SafeMode::ExecutionSuspended;
    }
    if health
        .event_lag_ms
        .is_some_and(|lag| std::time::Duration::from_millis(lag) > thresholds.max_event_lag)
    {
        return SafeMode::StaleInputs;
    }
    SafeMode::None
}

/// Build the deterministic one-step candidate plan an AUTO-FIX escalation
/// executes through the ordinary remediation path. The action's risk class,
/// reversibility, and blast radius must match what the escalation already
/// weighed — the builder refuses to construct a plan the decision would not
/// have allowed.
pub fn autofix_plan(
    escalation: &EscalationInput,
    capability: &argus_domain::CapabilityId,
    arguments: serde_json::Value,
    rollback: Option<argus_domain::CapabilityId>,
) -> Option<Plan> {
    // Only an AutoFix-shaped situation may build an autofix plan, and only
    // when the deterministic decision actually returns AutoFix.
    if decide_escalation(escalation) != Escalation::AutoFix {
        return None;
    }
    if !matches!(
        escalation.risk,
        RiskClass::Read | RiskClass::LowRisk | RiskClass::Controlled
    ) {
        return None;
    }
    if !escalation.reversible {
        return None;
    }
    if !matches!(
        escalation.blast_radius,
        BlastRadius::None | BlastRadius::Host
    ) {
        return None;
    }
    let _ = (
        Reversibility::Reversible,
        EvidenceQuality::Adequate,
        Uuid::new_v4(),
    );
    Some(Plan {
        objective: format!(
            "sentinel auto-fix: {} ({} escalation)",
            capability.as_str(),
            "auto"
        ),
        steps: vec![PlanStep {
            action: Action {
                capability: capability.clone(),
                resource: None,
                arguments,
            },
            rollback: rollback.map(|c| Action {
                capability: c,
                resource: None,
                arguments: serde_json::Value::Null,
            }),
        }],
        preconditions: Vec::new(),
        expected_outcomes: vec![format!("{} applied", capability.as_str())],
        blast_radius: escalation.blast_radius,
        confidence: escalation.confidence as f64,
        status: argus_domain::PlanStatus::Proposed,
    })
}

/// Convenience for tests/consumers: a situation input with the sentinel's
/// defaults filled in.
pub fn situation_input(
    autonomy: AutonomyMode,
    environment: Environment,
    confidence: f32,
    reversible: bool,
    risk: RiskClass,
    historical_success: Option<f32>,
    policy: PolicyOutcome,
) -> EscalationInput {
    EscalationInput {
        evidence_quality: EvidenceQuality::Adequate,
        confidence,
        reversible,
        blast_radius: BlastRadius::Host,
        risk,
        criticality: argus_policy::Criticality::Standard,
        environment,
        historical_success,
        autonomy,
        policy,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn healthy_self() -> SelfHealth {
        SelfHealth {
            uptime_seconds: 60,
            event_lag_ms: Some(100),
            ..SelfHealth::default()
        }
    }

    fn inputs() -> SentinelInputs {
        SentinelInputs {
            autonomy: AutonomyMode::L4Autonomous,
            environment: Environment::Production,
            provider_ready: true,
            open_incidents: 1,
            risks: vec![Risk {
                kind: argus_risk::RiskKind::Reliability,
                subject: argus_domain::ResourceId::new("service", "api").unwrap(),
                severity: Severity::Warning,
                evidence: vec!["restart loop".into()],
                recommendation: None,
            }],
            predictions: vec!["PREDICTED (linear-trend, confidence 0.72): disk 90%".into()],
            pending_approvals: 0,
            recent_actions: 3,
            self_health: healthy_self(),
            situation: Some(situation_input(
                AutonomyMode::L4Autonomous,
                Environment::Production,
                0.9,
                true,
                RiskClass::LowRisk,
                Some(0.9),
                PolicyOutcome::Allow,
            )),
            autonomy_view: None,
            runbooks_view: None,
        }
    }

    #[test]
    fn a_degraded_environment_surfaces_degraded_health() {
        let mut i = inputs();
        i.open_incidents = 0;
        i.risks.clear();
        let d = sentinel_evaluate(&i, Utc::now());
        assert_eq!(d.view.environment_health, EnvironmentHealth::Healthy);

        let mut degraded = inputs();
        degraded.provider_ready = false;
        let d = sentinel_evaluate(&degraded, Utc::now());
        assert_eq!(d.view.environment_health, EnvironmentHealth::Degraded);
        assert!(!d.view.provider_ready);
    }

    #[test]
    fn error_risks_and_suspended_execution_surface_critical() {
        let mut critical = inputs();
        critical.risks[0].severity = Severity::Error;
        critical.open_incidents = 1;
        let d = sentinel_evaluate(&critical, Utc::now());
        assert_eq!(d.view.environment_health, EnvironmentHealth::Critical);

        let suspended = inputs();
        let mut health = healthy_self();
        health.executor_errors = 10;
        let mut s = suspended;
        s.self_health = health;
        let d = sentinel_evaluate(&s, Utc::now());
        assert_eq!(d.view.environment_health, EnvironmentHealth::Critical);
        assert_eq!(d.view.safe_mode, SafeModeSerde::ExecutionSuspended);
    }

    #[test]
    fn a_strong_l4_situation_escalates_to_autofix() {
        let d = sentinel_evaluate(&inputs(), Utc::now());
        assert_eq!(d.escalation, Some(Escalation::AutoFix));
        assert!(d.requests_autofix());
        // The view carries the prediction prose verbatim — labeled.
        assert!(d.view.predictions[0].starts_with("PREDICTED"));
        assert_eq!(d.view.pressure.len(), 1);
    }

    #[test]
    fn safe_mode_degrades_any_escalation_to_ask_human() {
        let mut i = inputs();
        let mut health = healthy_self();
        health.event_lag_ms = Some(10 * 60 * 1000); // 10 minutes stale
        i.self_health = health;
        let d = sentinel_evaluate(&i, Utc::now());
        assert_eq!(d.view.safe_mode, SafeModeSerde::StaleInputs);
        // Stale inputs: even an L4 allow degrades to AskHuman, never AutoFix.
        assert_eq!(d.escalation, Some(Escalation::AskHuman));
        assert!(!d.requests_autofix());
    }

    #[test]
    fn l0_caps_the_escalation_at_observe() {
        let mut i = inputs();
        i.autonomy = AutonomyMode::L0Observe;
        if let Some(s) = i.situation.as_mut() {
            s.autonomy = AutonomyMode::L0Observe;
        }
        let d = sentinel_evaluate(&i, Utc::now());
        assert_eq!(d.escalation, Some(Escalation::Observe));
    }

    #[test]
    fn autofix_plans_are_only_built_for_allowed_situations() {
        let situation = situation_input(
            AutonomyMode::L4Autonomous,
            Environment::Production,
            0.9,
            true,
            RiskClass::LowRisk,
            Some(0.9),
            PolicyOutcome::Allow,
        );
        let cap = argus_domain::CapabilityId::new("host.service.restart").unwrap();
        let plan = autofix_plan(
            &situation,
            &cap,
            serde_json::json!({ "unit": "nginx" }),
            Some(argus_domain::CapabilityId::new("host.service.start").unwrap()),
        )
        .unwrap();
        assert_eq!(plan.steps.len(), 1);
        assert!(plan.steps[0].rollback.is_some());

        // A high-risk situation never yields a plan.
        let high = EscalationInput {
            risk: RiskClass::HighRisk,
            ..situation
        };
        assert!(autofix_plan(&high, &cap, serde_json::json!({}), None).is_none());

        // A denied situation never yields a plan.
        let denied = EscalationInput {
            policy: PolicyOutcome::Deny,
            ..situation
        };
        assert!(autofix_plan(&denied, &cap, serde_json::json!({}), None).is_none());
    }

    #[test]
    fn the_sentinel_view_serializes_for_the_tui_and_cloud() {
        let d = sentinel_evaluate(&inputs(), Utc::now());
        let json = serde_json::to_string(&d.view).unwrap();
        let back: SentinelView = serde_json::from_str(&json).unwrap();
        assert_eq!(back, d.view);
    }

    #[test]
    fn the_autonomy_line_is_additive_and_skip_if_none() {
        // Without a machine the view renders exactly as before: the field is
        // absent from the payload, and old readers see no change.
        let d = sentinel_evaluate(&inputs(), Utc::now());
        let json = serde_json::to_value(&d.view).unwrap();
        assert!(json.get("autonomy").is_none(), "skip-if-none");

        // With one, the map rides verbatim — the sentinel adds nothing.
        let mut inputs = inputs();
        inputs.autonomy_view = Some(
            serde_json::json!({
                "phase": "shadow",
                "phase_progress": { "cycles": 7, "required": 10 },
                "earned": "l0_observe",
                "ceiling": "l2_recommend",
                "effective": "l0_observe",
                "budgets": { "low_risk_per_hour": { "remaining": 17, "limit": 20 } },
            })
            .as_object()
            .cloned()
            .unwrap(),
        );
        let d = sentinel_evaluate(&inputs, Utc::now());
        let json = serde_json::to_value(&d.view).unwrap();
        assert_eq!(json["autonomy"]["phase"], "shadow");
        assert_eq!(json["autonomy"]["phase_progress"]["cycles"], 7);
        assert_eq!(json["autonomy"]["effective"], "l0_observe");
        // And the shape round-trips for the cloud (loose validation upstream).
        let back: SentinelView = serde_json::from_value(json).unwrap();
        assert_eq!(back, d.view);
    }

    #[test]
    fn the_runbook_table_is_additive_and_skip_if_none() {
        // Without a library the view renders exactly as before: the field is
        // absent from the payload, and old readers see no change (spec 009
        // FR-005, the lean build's unchanged behavior).
        let d = sentinel_evaluate(&inputs(), Utc::now());
        let json = serde_json::to_value(&d.view).unwrap();
        assert!(json.get("runbooks").is_none(), "skip-if-none");

        // With one, the table rides verbatim — the sentinel adds nothing.
        let mut inputs = inputs();
        inputs.runbooks_view = Some(
            serde_json::json!({
                "count": 1,
                "items": [{
                    "name": "learned-procedure",
                    "status": "candidate",
                    "gates": ["evaluation", "simulation"],
                    "provenance": { "version_number": 9 },
                }],
            })
            .as_object()
            .cloned()
            .unwrap(),
        );
        let d = sentinel_evaluate(&inputs, Utc::now());
        let json = serde_json::to_value(&d.view).unwrap();
        assert_eq!(json["runbooks"]["count"], 1);
        assert_eq!(json["runbooks"]["items"][0]["name"], "learned-procedure");
        assert_eq!(json["runbooks"]["items"][0]["status"], "candidate");
        // And the shape round-trips for the cloud (loose validation upstream).
        let back: SentinelView = serde_json::from_value(json).unwrap();
        assert_eq!(back, d.view);
    }
}
