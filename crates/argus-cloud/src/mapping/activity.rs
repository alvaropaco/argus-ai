//! Activity projection onto the cloud's activity vocabulary.
//!
//! The outcome vocabulary is derived, never guessed: an authorization decision
//! plus an optional execution status determines it, so a refusal can never be
//! reported as a success and an approval-required invocation can never be
//! reported as executed.

use chrono::{DateTime, Utc};

use argus_domain::{DecisionOutcome, Execution, ExecutionStatus, RiskClass};

use crate::protocol::messages::{
    ActivitiesReportPayload, ActivityOutcomeWire, ActivityRecordWire, MAX_ACTIVITIES_PER_REPORT,
};

use super::capability::risk_class;

/// The installation's outcome vocabulary for one attempted operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityOutcome {
    Succeeded,
    Failed,
    Denied,
    PendingApproval,
}

/// Derives the outcome from the authorization decision and, when it happened,
/// the execution status.
///
/// A `Permitted` decision with no execution recorded maps to `Failed`: the
/// installation cannot claim an operation succeeded when it has no record of it
/// running.
pub fn outcome_from(
    decision: DecisionOutcome,
    execution: Option<ExecutionStatus>,
) -> ActivityOutcome {
    match (decision, execution) {
        (DecisionOutcome::Denied, _) => ActivityOutcome::Denied,
        (DecisionOutcome::RequireApproval, _) => ActivityOutcome::PendingApproval,
        (DecisionOutcome::Permitted, Some(ExecutionStatus::Completed)) => {
            ActivityOutcome::Succeeded
        }
        (DecisionOutcome::Permitted, Some(ExecutionStatus::Failed)) => ActivityOutcome::Failed,
        (DecisionOutcome::Permitted, None) => ActivityOutcome::Failed,
    }
}

fn wire_outcome(outcome: ActivityOutcome) -> ActivityOutcomeWire {
    match outcome {
        ActivityOutcome::Succeeded => ActivityOutcomeWire::Succeeded,
        ActivityOutcome::Failed => ActivityOutcomeWire::Failed,
        ActivityOutcome::Denied => ActivityOutcomeWire::Denied,
        ActivityOutcome::PendingApproval => ActivityOutcomeWire::PendingApproval,
    }
}

/// Inputs the daemon supplies alongside the execution.
#[derive(Debug, Clone, Default)]
pub struct ActivityContext {
    pub intent_id: Option<String>,
    pub correlation_id: Option<String>,
    pub reason: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

/// Projects one execution onto the cloud's activity shape.
pub fn record(
    execution: &Execution,
    risk: RiskClass,
    outcome: ActivityOutcome,
    context: ActivityContext,
) -> ActivityRecordWire {
    let finished_at = context.finished_at;
    ActivityRecordWire {
        intent_id: context.intent_id,
        capability_id: execution.action.capability.as_str().to_string(),
        risk_class: risk_class(risk),
        resource: execution.action.resource.as_ref().map(ToString::to_string),
        outcome: wire_outcome(outcome),
        reason: context.reason,
        started_at: context.started_at.unwrap_or_else(Utc::now),
        finished_at,
        correlation_id: context.correlation_id,
    }
}

/// Splits activity records into report batches respecting the item bound.
pub fn batches(records: Vec<ActivityRecordWire>) -> Vec<ActivitiesReportPayload> {
    records
        .chunks(MAX_ACTIVITIES_PER_REPORT)
        .map(|chunk| ActivitiesReportPayload {
            activities: chunk.to_vec(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use argus_domain::{Action, CapabilityId, ResourceId};
    use chrono::TimeZone;

    fn ts() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 21, 12, 0, 0).unwrap()
    }

    fn execution(status: ExecutionStatus) -> Execution {
        Execution {
            action: Action {
                capability: CapabilityId::new("host.service.restart").unwrap(),
                resource: Some(ResourceId::new("host", "web-01").unwrap()),
                arguments: serde_json::json!({"unit": "nginx"}),
            },
            status,
            evidence: serde_json::json!({"exit": 0}),
        }
    }

    #[test]
    fn denied_never_reports_as_success() {
        assert_eq!(
            outcome_from(DecisionOutcome::Denied, Some(ExecutionStatus::Completed)),
            ActivityOutcome::Denied
        );
    }

    #[test]
    fn approval_required_reports_as_pending_approval() {
        assert_eq!(
            outcome_from(DecisionOutcome::RequireApproval, None),
            ActivityOutcome::PendingApproval
        );
    }

    #[test]
    fn permitted_and_completed_reports_as_succeeded() {
        assert_eq!(
            outcome_from(DecisionOutcome::Permitted, Some(ExecutionStatus::Completed)),
            ActivityOutcome::Succeeded
        );
    }

    #[test]
    fn permitted_and_failed_reports_as_failed() {
        assert_eq!(
            outcome_from(DecisionOutcome::Permitted, Some(ExecutionStatus::Failed)),
            ActivityOutcome::Failed
        );
    }

    #[test]
    fn permitted_without_a_record_cannot_claim_success() {
        assert_eq!(
            outcome_from(DecisionOutcome::Permitted, None),
            ActivityOutcome::Failed
        );
    }

    #[test]
    fn record_carries_capability_resource_and_risk() {
        let context = ActivityContext {
            intent_id: Some("intent-1".into()),
            correlation_id: Some("corr-1".into()),
            reason: None,
            started_at: Some(ts()),
            finished_at: Some(ts()),
        };
        let wire = record(
            &execution(ExecutionStatus::Completed),
            RiskClass::LowRisk,
            ActivityOutcome::Succeeded,
            context,
        );

        assert_eq!(wire.capability_id, "host.service.restart");
        assert_eq!(wire.resource.as_deref(), Some("host:web-01"));
        assert_eq!(
            wire.risk_class,
            crate::protocol::messages::RiskClassWire::LowRisk
        );
        assert_eq!(wire.outcome, ActivityOutcomeWire::Succeeded);
        assert_eq!(wire.correlation_id.as_deref(), Some("corr-1"));

        let value = serde_json::to_value(&wire).unwrap();
        assert_eq!(value["risk_class"], "LowRisk", "cloud casing, not low_risk");
        assert_eq!(value["outcome"], "succeeded");
    }

    #[test]
    fn batching_splits_at_the_item_bound_without_loss() {
        let records: Vec<ActivityRecordWire> = (0..(MAX_ACTIVITIES_PER_REPORT + 3))
            .map(|_| {
                record(
                    &execution(ExecutionStatus::Completed),
                    RiskClass::Read,
                    ActivityOutcome::Succeeded,
                    ActivityContext::default(),
                )
            })
            .collect();
        let batches = batches(records);

        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].activities.len(), MAX_ACTIVITIES_PER_REPORT);
        assert_eq!(batches[1].activities.len(), 3);
    }

    #[test]
    fn pending_approval_serialises_for_the_cloud() {
        let wire = record(
            &execution(ExecutionStatus::Failed),
            RiskClass::HighRisk,
            ActivityOutcome::PendingApproval,
            ActivityContext::default(),
        );
        assert_eq!(
            serde_json::to_value(&wire).unwrap()["outcome"],
            "pending_approval"
        );
    }
}
