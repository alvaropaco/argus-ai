//! The first operational skill and the thin reasoning loop.
//!
//! One skill (host-service health triage) with typed `choice`/`noul` questions,
//! and a loop that turns evidence into a validated, policy-gated, recorded
//! action. The loop depends on ports ([`ActionPort`], [`RecordPort`]) so it is
//! deterministic and testable; the daemon wires policy+executor and evidence.

use std::collections::BTreeMap;

use argus_domain::{Action, CapabilityId, Plan};
use async_trait::async_trait;
use serde_json::Value;

use crate::decision::context::ContextBuilder;
use crate::decision::error::DecisionError;
use crate::decision::gateway::propose_plan;
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
fn remediation_action(response: &DecisionResponse) -> Option<Action> {
    match response.answers.get("remediation") {
        Some(DecisionAnswer::Choice { choice, .. }) if choice == "restart" => Some(Action {
            capability: CapabilityId::new(CapabilityId::HOST_SERVICE_RESTART)
                .expect("bootstrap capability id is valid"),
            resource: None,
            arguments: Value::Object(serde_json::Map::new()),
        }),
        _ => None,
    }
}

/// A port the loop uses to execute an action; the daemon wires policy+executor.
#[async_trait]
pub trait ActionPort: Send + Sync {
    async fn execute(&self, action: &Action) -> Result<Value, DecisionError>;
}

/// A port the loop uses to record its outcome; the daemon wires evidence+audit.
#[async_trait]
pub trait RecordPort: Send + Sync {
    async fn record(&self, plan: &Plan, executed: bool) -> Result<(), DecisionError>;
}

/// Runs one host-health step: decide, validate, gate on confidence, propose a
/// plan, execute through the port, and record the outcome.
///
/// Returns the proposed plan when it clears the threshold, or `None` when the
/// planner proposes nothing actionable (fail-closed — nothing executes). An
/// execution or recording failure is recorded before it propagates.
pub async fn run_host_health(
    provider: &dyn DecisionProvider,
    evidence: ContextBuilder,
    threshold: f64,
    actions: &dyn ActionPort,
    recorder: &dyn RecordPort,
) -> Result<Option<Plan>, DecisionError> {
    let request = host_health_request(evidence);
    let response = provider.decide(request.clone()).await?;
    crate::decision::validate::validate_response(&request, &response)?;

    let Some(action) = remediation_action(&response) else {
        return Ok(None);
    };
    let Some(plan) = propose_plan(
        &response,
        threshold,
        "restore the host service",
        vec![action],
    ) else {
        return Ok(None);
    };

    // Execute every action the plan carries, and always record the outcome —
    // including a failed execution — so the audit trail is never missing.
    for action in &plan.actions {
        if let Err(error) = actions.execute(action).await {
            let _ = recorder.record(&plan, false).await;
            return Err(error);
        }
    }
    recorder.record(&plan, true).await?;
    Ok(Some(plan))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use crate::decision::adapters::FakeDecisionProvider;

    #[derive(Default)]
    struct RecordingPorts {
        executed: Mutex<Vec<String>>,
        recorded: Mutex<Vec<(String, bool)>>,
    }

    #[async_trait]
    impl ActionPort for RecordingPorts {
        async fn execute(&self, action: &Action) -> Result<Value, DecisionError> {
            self.executed
                .lock()
                .unwrap()
                .push(action.capability.as_str().to_string());
            Ok(Value::Null)
        }
    }

    #[async_trait]
    impl RecordPort for RecordingPorts {
        async fn record(&self, plan: &Plan, executed: bool) -> Result<(), DecisionError> {
            self.recorded
                .lock()
                .unwrap()
                .push((plan.objective.clone(), executed));
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
        builder
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

        let plan = run_host_health(&provider, evidence(), 0.7, &ports, &ports)
            .await
            .expect("loop runs");

        assert!(plan.is_some(), "a plan clears the threshold");
        assert_eq!(ports.executed.lock().unwrap().len(), 1);
        assert_eq!(
            ports.executed.lock().unwrap()[0],
            CapabilityId::HOST_SERVICE_RESTART
        );
        assert_eq!(ports.recorded.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn low_confidence_is_fail_closed() {
        let provider = FakeDecisionProvider::new(answers(0.4, 0.4));
        let ports = RecordingPorts::default();

        let plan = run_host_health(&provider, evidence(), 0.7, &ports, &ports)
            .await
            .expect("loop runs");

        assert!(plan.is_none(), "below threshold proposes nothing");
        assert!(ports.executed.lock().unwrap().is_empty());
        assert!(ports.recorded.lock().unwrap().is_empty());
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

        let result = run_host_health(&provider, evidence(), 0.7, &ports, &ports).await;
        assert!(matches!(result, Err(DecisionError::Validation(_))));
        assert!(ports.executed.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn failed_execution_is_recorded_before_it_propagates() {
        let provider = FakeDecisionProvider::new(answers(0.9, 0.9));
        let recorder = RecordingPorts::default();
        let failing = FailingActionPort;

        let result = run_host_health(&provider, evidence(), 0.7, &failing, &recorder).await;
        assert!(result.is_err());
        let recorded = recorder.recorded.lock().unwrap();
        assert_eq!(recorded.len(), 1, "a failed execution is still recorded");
        assert!(!recorded[0].1, "recorded as not-executed");
    }
}
