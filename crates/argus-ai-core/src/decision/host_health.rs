//! The first operational skill and the thin reasoning loop.
//!
//! One skill (host-service health triage) with typed `choice`/`noul` questions,
//! and a loop that turns evidence into a validated, policy-gated, recorded
//! action. The loop depends on ports ([`ActionPort`], [`RecordPort`]) so it is
//! deterministic and testable; the daemon wires policy+executor and evidence.

use std::collections::BTreeMap;

use argus_domain::{Action, CapabilityId, Plan, PlanStep};
use async_trait::async_trait;
use serde_json::Value;

use crate::decision::context::{ContextBuilder, REDACTED};
use crate::decision::error::DecisionError;
use crate::decision::gateway::{DecisionOutcome, propose_plan};
use crate::decision::provenance::DecisionProvenance;
use crate::decision::provider::DecisionProvider;
use crate::decision::types::{
    DecisionAnswer, DecisionQuestion, DecisionRequest, DecisionResponse, NoulCriteria,
};

/// The skill's typed questions: is the service degraded, and which remediation.
pub fn host_health_questions() -> BTreeMap<String, DecisionQuestion> {
    BTreeMap::from([
        (
            "degraded".to_string(),
            DecisionQuestion::noul(
                "Is the host service degraded, failing, or unhealthy?",
                NoulCriteria {
                    yes: Some("down, failing, or restart-looping".into()),
                    no: Some("healthy".into()),
                },
            ),
        ),
        (
            "remediation".to_string(),
            DecisionQuestion::choice(
                "Which remediation fits?",
                BTreeMap::from([
                    (
                        "restart".to_string(),
                        Some("restart the service".to_string()),
                    ),
                    ("noop".to_string(), Some("do nothing".to_string())),
                ]),
            ),
        ),
    ])
}

/// Builds the decision request from evidence.
pub fn host_health_request(evidence: ContextBuilder) -> DecisionRequest {
    DecisionRequest {
        model: None,
        state: evidence.into_state(),
        questions: host_health_questions(),
    }
}

/// The typed action a validated response implies, if any.
///
/// A restart names the service unit to restart, read from the `unit` evidence
/// attribute. Without that evidence there is no well-formed action, so none is
/// proposed (fail closed — nothing executes).
fn remediation_action(response: &DecisionResponse, state: &Value) -> Option<Action> {
    match response.answers.get("remediation") {
        Some(DecisionAnswer::Choice { choice, .. }) if choice == "restart" => {
            let unit = unit_from_evidence(state)?;
            Some(Action {
                capability: CapabilityId::new(CapabilityId::HOST_SERVICE_RESTART)
                    .expect("bootstrap capability id is valid"),
                resource: None,
                arguments: serde_json::json!({ "unit": unit }),
            })
        }
        _ => None,
    }
}

/// The `unit` evidence value, as a string, when the state carries exactly one
/// usable unit.
///
/// Fails closed: a unit is usable only when the evidence names exactly one
/// distinct, non-blank, non-redacted value. Blank, whitespace-only, and
/// [`REDACTED`] units are treated as no usable unit, and more than one distinct
/// unit means the target is ambiguous, so no action is proposed.
fn unit_from_evidence(state: &Value) -> Option<String> {
    let mut units: Vec<&str> = state
        .get("evidence")?
        .as_array()?
        .iter()
        .filter(|entry| entry.get("attribute").and_then(Value::as_str) == Some("unit"))
        .filter_map(|entry| entry.get("value").and_then(Value::as_str))
        .map(str::trim)
        .filter(|unit| !unit.is_empty())
        .collect();

    units.sort_unstable();
    units.dedup();

    match units.as_slice() {
        [unit] if *unit != REDACTED => Some((*unit).to_string()),
        _ => None,
    }
}

/// The declarative rollback action for a step, when one exists (ADR-0028 §4).
///
/// A stop is undone by a start and vice versa. A restart has no inverse: it
/// neither creates nor removes a running unit, so a blind reversal would only
/// disturb it further — its rollback is `None`.
fn rollback_for(action: &Action) -> Option<Action> {
    let inverse = match action.capability.as_str() {
        CapabilityId::HOST_SERVICE_STOP => Some(CapabilityId::HOST_SERVICE_START),
        CapabilityId::HOST_SERVICE_START => Some(CapabilityId::HOST_SERVICE_STOP),
        _ => None,
    };
    inverse.map(|capability| Action {
        capability: CapabilityId::new(capability).expect("bootstrap capability ids are valid"),
        resource: action.resource.clone(),
        arguments: action.arguments.clone(),
    })
}

/// The deterministic observation dedup key: the canonical, order-independent
/// serialization of the evidence state.
///
/// The same observation produces the same key, so a repeated observation can be
/// recognized before it spawns a plan (ADR-0028 §5). The key is content-derived
/// because observation ids are random today.
pub fn observation_dedup_key(evidence: &ContextBuilder) -> String {
    evidence.canonical_key()
}

/// A port the loop uses to execute an action; the daemon wires policy+executor.
#[async_trait]
pub trait ActionPort: Send + Sync {
    async fn execute(&self, action: &Action) -> Result<Value, DecisionError>;
}

/// A port the loop uses to record its outcome; the daemon wires evidence+audit.
#[async_trait]
pub trait RecordPort: Send + Sync {
    async fn record(
        &self,
        plan: &Plan,
        executed: bool,
        provenance: &DecisionProvenance,
    ) -> Result<(), DecisionError>;

    /// Records a repeated observation that was deduplicated (no plan spawned).
    ///
    /// Defaults to a no-op so existing recorders need not change; the daemon
    /// overrides it to write the dedup to the audit trail.
    async fn record_dedup(&self, _key: &str) -> Result<(), DecisionError> {
        Ok(())
    }
}

/// A port the loop uses to deduplicate repeated observations.
#[async_trait]
pub trait DedupPort: Send + Sync {
    /// Claims `key`; returns `Ok(true)` when this is a new observation (proceed),
    /// or `Ok(false)` when it was already seen (duplicate — spawn no plan).
    async fn claim(&self, key: &str) -> Result<bool, DecisionError>;

    /// Releases a claimed key when no plan was produced, so a later identical
    /// observation is not suppressed by a transient no-plan.
    ///
    /// Defaults to a no-op so existing dedup ports need not change; the daemon's
    /// in-memory port clears the key.
    async fn release(&self, _key: &str) -> Result<(), DecisionError> {
        Ok(())
    }
}

/// A port the loop uses to re-observe live state, record the evidence, and
/// publish the validation outcome after the step loop completes (ADR-0031 §3, §4).
///
/// The validator itself (`argus_validate`) is pure; this port is where the
/// daemon supplies the re-observed state, so the loop stays free of host IO and
/// testable without one.
#[async_trait]
pub trait ValidationPort: Send + Sync {
    /// Re-observes the plan's executed steps against live state, records the
    /// outcome as evidence, and publishes `VALIDATION_PASSED`/`VALIDATION_FAILED`.
    ///
    /// A live-state read failure must fail closed (no pass is published and the
    /// failure is recorded), not propagate as an error that aborts the
    /// already-executed plan.
    async fn validate(&self, plan: &Plan) -> Result<(), DecisionError>;
}

/// Runs one host-health step: decide, validate, gate on confidence, propose a
/// plan, execute through the port, and record the outcome.
///
/// A repeated observation (same dedup key) spawns no plan: the repeat is a
/// recorded no-op. Returns the proposed plan when it clears the threshold, or
/// `None` when the planner proposes nothing actionable (fail-closed — nothing
/// executes). An execution or recording failure is recorded before it propagates.
pub async fn run_host_health(
    provider: &dyn DecisionProvider,
    evidence: ContextBuilder,
    threshold: f64,
    actions: &dyn ActionPort,
    recorder: &dyn RecordPort,
    dedup: &dyn DedupPort,
    validator: &dyn ValidationPort,
) -> Result<Option<Plan>, DecisionError> {
    let key = observation_dedup_key(&evidence);
    if !dedup.claim(&key).await? {
        recorder.record_dedup(&key).await?;
        return Ok(None);
    }

    let request = host_health_request(evidence);
    let state = request.state.clone();
    let response =
        match crate::decision::gateway::decide_outcome_with(provider, request, threshold).await? {
            DecisionOutcome::NoDecision(_) => {
                dedup.release(&key).await?;
                return Ok(None);
            }
            DecisionOutcome::Decided(response) => response,
        };
    let provenance = DecisionProvenance::from_response(&response, &state);

    let Some(action) = remediation_action(&response, &state) else {
        dedup.release(&key).await?;
        return Ok(None);
    };
    let rollback = rollback_for(&action);
    let step = PlanStep { action, rollback };
    let Some(plan) = propose_plan(&response, threshold, "restore the host service", vec![step])
    else {
        dedup.release(&key).await?;
        return Ok(None);
    };

    // Execute every step the plan carries, and always record the outcome —
    // including a failed execution — so the audit trail is never missing.
    for step in &plan.steps {
        if let Err(error) = actions.execute(&step.action).await {
            let _ = recorder.record(&plan, false, &provenance).await;
            return Err(error);
        }
    }
    recorder.record(&plan, true, &provenance).await?;
    // Re-observe after the loop and publish the validation outcome (ADR-0031 §6).
    // A validation failure here must not fail the loop: the plan already executed
    // and was recorded, so a persistence/publish error is logged, not propagated.
    if let Err(error) = validator.validate(&plan).await {
        tracing::warn!(error = %error, "post-execution validation failed");
    }
    Ok(Some(plan))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use crate::decision::adapters::FakeDecisionProvider;
    use crate::decision::context::MAX_CONTEXT_ENTRIES;

    #[derive(Default)]
    struct RecordingPorts {
        executed: Mutex<Vec<String>>,
        arguments: Mutex<Vec<Value>>,
        recorded: Mutex<Vec<(String, bool, DecisionProvenance)>>,
        deduped: Mutex<Vec<String>>,
        validated: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl ActionPort for RecordingPorts {
        async fn execute(&self, action: &Action) -> Result<Value, DecisionError> {
            self.executed
                .lock()
                .unwrap()
                .push(action.capability.as_str().to_string());
            self.arguments
                .lock()
                .unwrap()
                .push(action.arguments.clone());
            Ok(Value::Null)
        }
    }

    #[async_trait]
    impl RecordPort for RecordingPorts {
        async fn record(
            &self,
            plan: &Plan,
            executed: bool,
            provenance: &DecisionProvenance,
        ) -> Result<(), DecisionError> {
            self.recorded.lock().unwrap().push((
                plan.objective.clone(),
                executed,
                provenance.clone(),
            ));
            Ok(())
        }

        async fn record_dedup(&self, key: &str) -> Result<(), DecisionError> {
            self.deduped.lock().unwrap().push(key.to_string());
            Ok(())
        }
    }

    #[async_trait]
    impl DedupPort for RecordingPorts {
        async fn claim(&self, _key: &str) -> Result<bool, DecisionError> {
            Ok(true)
        }
    }

    #[async_trait]
    impl ValidationPort for RecordingPorts {
        async fn validate(&self, plan: &Plan) -> Result<(), DecisionError> {
            self.validated.lock().unwrap().push(plan.objective.clone());
            Ok(())
        }
    }

    /// A dedup port that remembers claimed keys, for the repeated-observation
    /// test.
    #[derive(Default)]
    struct StatefulDedup {
        seen: Mutex<std::collections::HashSet<String>>,
    }

    #[async_trait]
    impl DedupPort for StatefulDedup {
        async fn claim(&self, key: &str) -> Result<bool, DecisionError> {
            Ok(self.seen.lock().unwrap().insert(key.to_string()))
        }

        async fn release(&self, key: &str) -> Result<(), DecisionError> {
            self.seen.lock().unwrap().remove(key);
            Ok(())
        }
    }

    /// A port that fails execution from the first call.
    #[derive(Default)]
    struct FailingActionPort;

    #[async_trait]
    impl ActionPort for FailingActionPort {
        async fn execute(&self, _action: &Action) -> Result<Value, DecisionError> {
            Err(DecisionError::Unavailable("execution failed".into()))
        }
    }

    fn evidence() -> ContextBuilder {
        let mut builder = ContextBuilder::new();
        builder.evidence("host:a", "service.nginx.state", serde_json::json!("failed"));
        builder.evidence("host:a", "unit", serde_json::json!("nginx.service"));
        builder
    }

    /// Builds a state carrying only `unit` evidence with the given values.
    fn unit_state(units: &[&str]) -> Value {
        let mut builder = ContextBuilder::new();
        for unit in units {
            builder.evidence("host:a", "unit", serde_json::json!(unit));
        }
        builder.into_state()
    }

    #[test]
    fn unit_from_evidence_returns_the_single_usable_unit() {
        assert_eq!(
            unit_from_evidence(&unit_state(&["nginx.service"])).as_deref(),
            Some("nginx.service")
        );
    }

    #[test]
    fn unit_from_evidence_treats_blank_whitespace_and_redacted_as_no_unit() {
        assert_eq!(unit_from_evidence(&unit_state(&[""])), None);
        assert_eq!(unit_from_evidence(&unit_state(&["   "])), None);
        assert_eq!(unit_from_evidence(&unit_state(&[REDACTED])), None);
    }

    #[test]
    fn unit_from_evidence_fails_closed_on_multiple_distinct_units() {
        assert_eq!(
            unit_from_evidence(&unit_state(&["nginx.service", "postgres.service"])),
            None
        );
    }

    #[tokio::test]
    async fn a_trimmed_unit_evidence_fails_closed_without_an_action() {
        // The `unit` evidence is the oldest entry, so once the context exceeds
        // the cap it is trimmed by `into_state`. A restart then has no target
        // and the loop proposes nothing (fail closed), rather than a wrong or
        // silently absent action.
        let provider = FakeDecisionProvider::new(answers(0.9, 0.9));
        let ports = RecordingPorts::default();

        let mut context = ContextBuilder::new();
        context.evidence("host:a", "unit", serde_json::json!("nginx.service"));
        for i in 0..MAX_CONTEXT_ENTRIES {
            context.evidence("host:a", format!("metric.{i}"), serde_json::json!(i));
        }

        let plan = run_host_health(&provider, context, 0.7, &ports, &ports, &ports, &ports)
            .await
            .expect("loop runs");

        assert!(
            plan.is_none(),
            "a trimmed unit evidence names no action (fail closed)"
        );
        assert!(ports.executed.lock().unwrap().is_empty());
        assert!(ports.arguments.lock().unwrap().is_empty());
    }

    fn answers(choice_conf: f64, noul: f64) -> BTreeMap<String, DecisionAnswer> {
        BTreeMap::from([
            ("degraded".to_string(), DecisionAnswer::Noul { noul }),
            (
                "remediation".to_string(),
                DecisionAnswer::Choice {
                    choice: "restart".into(),
                    confidence: choice_conf,
                    probabilities: BTreeMap::from([
                        ("restart".to_string(), choice_conf),
                        ("noop".to_string(), 1.0 - choice_conf),
                    ]),
                },
            ),
        ])
    }

    #[tokio::test]
    async fn happy_path_proposes_executes_and_records() {
        let provider = FakeDecisionProvider::new(answers(0.9, 0.9));
        let ports = RecordingPorts::default();

        let plan = run_host_health(&provider, evidence(), 0.7, &ports, &ports, &ports, &ports)
            .await
            .expect("loop runs");

        assert!(plan.is_some(), "a plan clears the threshold");
        assert_eq!(ports.executed.lock().unwrap().len(), 1);
        assert_eq!(
            ports.executed.lock().unwrap()[0],
            CapabilityId::HOST_SERVICE_RESTART
        );
        {
            let arguments = ports.arguments.lock().unwrap();
            assert_eq!(arguments.len(), 1);
            assert_eq!(
                arguments[0]["unit"], "nginx.service",
                "the restart arguments carry the unit"
            );
        }
        {
            let recorded = ports.recorded.lock().unwrap();
            assert_eq!(recorded.len(), 1);
            assert_eq!(recorded[0].2.model_id, crate::decision::provenance::UNKNOWN);
            assert_eq!(recorded[0].2.context_hash.len(), 16);
        }
        assert_eq!(
            ports.validated.lock().unwrap().len(),
            1,
            "validation runs after the loop"
        );
    }

    #[tokio::test]
    async fn low_confidence_is_fail_closed() {
        let provider = FakeDecisionProvider::new(answers(0.4, 0.4));
        let ports = RecordingPorts::default();

        let plan = run_host_health(&provider, evidence(), 0.7, &ports, &ports, &ports, &ports)
            .await
            .expect("loop runs");

        assert!(plan.is_none(), "below threshold proposes nothing");
        assert!(ports.executed.lock().unwrap().is_empty());
        assert!(ports.recorded.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_restart_without_unit_evidence_proposes_no_action() {
        // A restart decision with no `unit` evidence cannot name a unit, so the
        // loop fails closed and proposes nothing (no malformed call is emitted).
        let provider = FakeDecisionProvider::new(answers(0.9, 0.9));
        let ports = RecordingPorts::default();

        let mut no_unit = ContextBuilder::new();
        no_unit.evidence("host:a", "service.nginx.state", serde_json::json!("failed"));

        let plan = run_host_health(&provider, no_unit, 0.7, &ports, &ports, &ports, &ports)
            .await
            .expect("loop runs");

        assert!(plan.is_none(), "no unit evidence means no action");
        assert!(ports.executed.lock().unwrap().is_empty());
        assert!(ports.arguments.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn malformed_response_is_rejected() {
        // A kind mismatch (choice where the skill asks noul) must be rejected.
        let provider = FakeDecisionProvider::new(BTreeMap::from([
            (
                "degraded".to_string(),
                DecisionAnswer::Choice {
                    choice: "restart".into(),
                    confidence: 0.9,
                    probabilities: BTreeMap::new(),
                },
            ),
            (
                "remediation".to_string(),
                DecisionAnswer::Choice {
                    choice: "restart".into(),
                    confidence: 0.9,
                    probabilities: BTreeMap::new(),
                },
            ),
        ]));
        let ports = RecordingPorts::default();

        let result =
            run_host_health(&provider, evidence(), 0.7, &ports, &ports, &ports, &ports).await;
        assert!(matches!(result, Err(DecisionError::Validation(_))));
        assert!(ports.executed.lock().unwrap().is_empty());
    }

    struct UnavailableProvider;

    #[async_trait]
    impl DecisionProvider for UnavailableProvider {
        async fn decide(
            &self,
            _request: DecisionRequest,
        ) -> Result<DecisionResponse, DecisionError> {
            Err(DecisionError::Unavailable("engine down".into()))
        }
    }

    #[tokio::test]
    async fn unavailable_engine_fails_closed() {
        let ports = RecordingPorts::default();

        let plan = run_host_health(
            &UnavailableProvider,
            evidence(),
            0.7,
            &ports,
            &ports,
            &ports,
            &ports,
        )
        .await
        .expect("loop runs");

        assert!(plan.is_none(), "an unavailable engine proposes nothing");
        assert!(ports.executed.lock().unwrap().is_empty());
        assert!(ports.recorded.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn failed_execution_is_recorded_before_it_propagates() {
        let provider = FakeDecisionProvider::new(answers(0.9, 0.9));
        let recorder = RecordingPorts::default();
        let failing = FailingActionPort;

        let result = run_host_health(
            &provider,
            evidence(),
            0.7,
            &failing,
            &recorder,
            &recorder,
            &recorder,
        )
        .await;
        assert!(result.is_err());
        let recorded = recorder.recorded.lock().unwrap();
        assert_eq!(recorded.len(), 1, "a failed execution is still recorded");
        assert!(!recorded[0].1, "recorded as not-executed");
    }

    #[tokio::test]
    async fn a_repeated_observation_spawns_one_plan_and_the_repeat_is_a_noop() {
        let provider = FakeDecisionProvider::new(answers(0.9, 0.9));
        let ports = RecordingPorts::default();
        let dedup = StatefulDedup::default();

        let first = run_host_health(&provider, evidence(), 0.7, &ports, &ports, &dedup, &ports)
            .await
            .expect("loop runs");
        assert!(first.is_some(), "the first observation plans");

        let second = run_host_health(&provider, evidence(), 0.7, &ports, &ports, &dedup, &ports)
            .await
            .expect("loop runs");
        assert!(second.is_none(), "the repeat spawns no plan");

        assert_eq!(
            ports.executed.lock().unwrap().len(),
            1,
            "exactly one execution"
        );
        assert_eq!(
            ports.deduped.lock().unwrap().len(),
            1,
            "the repeat is recorded as a no-op"
        );
    }

    #[tokio::test]
    async fn a_transient_no_plan_does_not_suppress_a_later_identical_observation() {
        let low = FakeDecisionProvider::new(answers(0.4, 0.4));
        let high = FakeDecisionProvider::new(answers(0.9, 0.9));
        let ports = RecordingPorts::default();
        let dedup = StatefulDedup::default();

        let first = run_host_health(&low, evidence(), 0.7, &ports, &ports, &dedup, &ports)
            .await
            .expect("loop runs");
        assert!(first.is_none(), "low confidence proposes nothing");

        let second = run_host_health(&high, evidence(), 0.7, &ports, &ports, &dedup, &ports)
            .await
            .expect("loop runs");
        assert!(
            second.is_some(),
            "a later identical observation plans after a transient no-plan"
        );
    }

    #[test]
    fn observation_dedup_key_is_stable_across_evidence_order() {
        let mut first = ContextBuilder::new();
        first.evidence("host:a", "service.nginx.state", serde_json::json!("failed"));
        first.evidence("host:a", "unit", serde_json::json!("nginx.service"));

        let mut second = ContextBuilder::new();
        second.evidence("host:a", "unit", serde_json::json!("nginx.service"));
        second.evidence("host:a", "service.nginx.state", serde_json::json!("failed"));

        assert_eq!(
            observation_dedup_key(&first),
            observation_dedup_key(&second),
            "the dedup key must not depend on evidence insertion order"
        );
    }
}
