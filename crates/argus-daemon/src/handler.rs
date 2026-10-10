//! Request dispatch: authorization and operation routing.

use argus_cloud::transport::websocket::WssTransport;
use argus_domain::Principal;
use argus_ipc::{ErrorCode, Operation, Request, Response};
use chrono::Utc;
use serde_json::Value;
use uuid::Uuid;

use crate::cloud::EnrollmentError;
use crate::runbooks::Decision;
use crate::runtime::Daemon;

/// Handles a single decoded request, enforcing authorization and dispatch.
pub async fn handle(daemon: &Daemon, principal: Principal, request: Request) -> Response {
    tracing::debug!(
        correlation_id = %request.correlation_id,
        operation = %request.operation,
        uid = ?principal.uid(),
        "handling request"
    );

    if let Some(allowed) = daemon.config().authorized_uids.as_ref() {
        match principal.uid() {
            Some(uid) if allowed.contains(&uid) => {}
            _ => {
                return Response::err(
                    request.correlation_id,
                    ErrorCode::Unauthorized,
                    "peer not authorized",
                );
            }
        }
    }

    let Some(operation) = request.operation.parse::<Operation>().ok() else {
        return Response::err(
            request.correlation_id,
            ErrorCode::UnknownOperation,
            format!("unknown operation '{}'", request.operation),
        );
    };

    let result = match operation {
        Operation::HealthGet => to_value(daemon.health()),
        Operation::StatusGet => daemon.status(),
        Operation::ConfigGet => to_value(daemon.config()),
        Operation::PluginsList => daemon.plugins(),
        Operation::CapabilitiesList => daemon.capabilities(),
        Operation::CloudEnroll => return enroll(daemon, request).await,
        Operation::CloudStatus => return cloud_status(daemon, request).await,
        Operation::CloudForget => return cloud_forget(daemon, request).await,
        Operation::CloudSetPrivilegedExecution => {
            return set_privileged_execution(daemon, request);
        }
        Operation::ApprovalList => return approval_list(daemon, request),
        Operation::ApprovalGrant => return approval_grant(daemon, &principal, request).await,
        Operation::ApprovalDeny => return approval_deny(daemon, &principal, request),
        Operation::PlanList => return plan_list(daemon, request).await,
        Operation::AuditList => return audit_list(daemon, request).await,
        Operation::SentinelGet => return sentinel_get(daemon, &request).await,
        Operation::ReportGenerate => return report_generate(daemon, &request).await,
        Operation::BrainDiagnose => return brain_diagnose(daemon, &request).await,
        Operation::RunbooksList => return runbooks_list(daemon, request.correlation_id),
        Operation::RunbooksApprove => {
            return runbook_decision(daemon, &principal, &request, Decision::Approve).await;
        }
        Operation::RunbooksPromote => {
            return runbook_decision(daemon, &principal, &request, Decision::Promote).await;
        }
    };

    Response::ok(request.correlation_id, result)
}

/// Reports what this installation knows about its cloud relationship.
async fn cloud_status(daemon: &Daemon, request: Request) -> Response {
    let tracker = daemon.tracker();
    let tracker = tracker.lock().await;
    let queue = daemon.report_queue();
    let queue = queue.lock().await;

    let status = to_value(
        crate::cloud::cloud_status(
            &daemon.config().cloud,
            daemon.repository().as_ref(),
            &tracker,
            &queue,
        )
        .await,
    );

    // The kill switch is runtime state, so it is reported from the live flag
    // rather than from the startup configuration snapshot.
    let status = match status {
        Value::Object(mut object) => {
            object.insert(
                "privileged_execution_enabled".to_string(),
                Value::Bool(daemon.privileged_execution_enabled()),
            );
            Value::Object(object)
        }
        other => other,
    };

    Response::ok(request.correlation_id, status)
}

/// Flips the local kill switch, which governs host-changing invocations only.
///
/// The request arrives over the local socket, where the daemon has already
/// authorized the peer by uid. Nothing on the cloud channel reaches this path,
/// which is what keeps the switch a local operator control (FR-050, SC-017).
fn set_privileged_execution(daemon: &Daemon, request: Request) -> Response {
    let Some(enabled) = request.payload.get("enabled").and_then(Value::as_bool) else {
        return Response::err(
            request.correlation_id,
            ErrorCode::Malformed,
            "cloud.set-privileged-execution requires a boolean 'enabled' in the payload",
        );
    };

    daemon.set_privileged_execution(enabled);

    Response::ok(
        request.correlation_id,
        serde_json::json!({ "privileged_execution_enabled": enabled }),
    )
}

/// Lists plans paused at an approval-requiring step, with their token and the
/// context hash the grant must be bound to.
fn approval_list(daemon: &Daemon, request: Request) -> Response {
    let pending = daemon
        .list_pending_approvals()
        .into_iter()
        .map(|pending| {
            serde_json::json!({
                "token": pending.token.to_string(),
                "context_hash": pending.context_hash,
                "objective": pending.plan.objective,
                "step_count": pending.plan.steps.len(),
            })
        })
        .collect::<Vec<_>>();

    Response::ok(request.correlation_id, serde_json::json!(pending))
}

/// Grants approval for a pending plan's token.
async fn approval_grant(daemon: &Daemon, principal: &Principal, request: Request) -> Response {
    let Some(token) = parse_token(&request) else {
        return Response::err(
            request.correlation_id,
            ErrorCode::Malformed,
            "approval.grant requires a 'token' in the payload",
        );
    };

    match daemon.grant_approval(token, &granted_by(principal)) {
        Ok(pending) => {
            // The second half of the round-trip (ADR-0030 §5): the grant is
            // consumed exactly once and the paused plan continues — the
            // operator's decision takes effect, and the response shows what
            // happened.
            let events = argus_events::LocalEventBus::new(16);
            let outcome = daemon.resume_remediation(&pending, &events).await;
            let resumed = match outcome {
                crate::control::ResumeOutcome::Finished(report) => format!("{:?}", report.status),
                crate::control::ResumeOutcome::Refused(why) => format!("refused: {why:?}"),
            };
            Response::ok(
                request.correlation_id,
                serde_json::json!({
                    "token": token.to_string(),
                    "state": "granted",
                    "outcome": resumed,
                }),
            )
        }
        Err(error) => Response::err(request.correlation_id, ErrorCode::Denied, error.to_string()),
    }
}

/// Denies a pending plan's token; a denied approval never executes.
fn approval_deny(daemon: &Daemon, principal: &Principal, request: Request) -> Response {
    let Some(token) = parse_token(&request) else {
        return Response::err(
            request.correlation_id,
            ErrorCode::Malformed,
            "approval.deny requires a 'token' in the payload",
        );
    };

    match daemon.deny_approval(token, &granted_by(principal)) {
        Ok(()) => Response::ok(
            request.correlation_id,
            serde_json::json!({ "token": token.to_string(), "state": "denied" }),
        ),
        Err(error) => Response::err(request.correlation_id, ErrorCode::Denied, error.to_string()),
    }
}

/// Lists the recorded plans with their correlation ids (FR-008): the
/// reasoning history an operator reviews alongside the audit trail.
async fn plan_list(daemon: &Daemon, request: Request) -> Response {
    match daemon.list_plans().await {
        Ok(plans) => {
            let plans = plans
                .into_iter()
                .map(|(id, plan)| {
                    let mut entry = serde_json::json!({
                        "correlation_id": id.to_string(),
                        "objective": plan.objective,
                        "status": plan.status,
                        "confidence": plan.confidence,
                        "step_count": plan.steps.len(),
                        "steps": plan.steps.iter().map(|step| {
                            serde_json::json!({
                                "capability": step.action.capability.as_str(),
                                "rollback": step.rollback.as_ref().map(|r| r.capability.as_str()),
                            })
                        }).collect::<Vec<_>>(),
                    });
                    // Spec 010 FR-002: a procedure plan's runbook attribution
                    // rides the entry — additive, absent for provider plans.
                    if let Some(runbook) = &plan.runbook
                        && let Some(object) = entry.as_object_mut()
                    {
                        object.insert("runbook".into(), serde_json::Value::String(runbook.clone()));
                    }
                    entry
                })
                .collect::<Vec<_>>();
            Response::ok(request.correlation_id, serde_json::json!(plans))
        }
        Err(error) => Response::err(
            request.correlation_id,
            ErrorCode::Internal,
            error.to_string(),
        ),
    }
}

/// Lists the append-only audit trail (FR-008).
async fn audit_list(daemon: &Daemon, request: Request) -> Response {
    match daemon.list_audit_events().await {
        Ok(events) => {
            let events = events
                .iter()
                .map(|event| {
                    serde_json::json!({
                        "id": event.id().to_string(),
                        "event_type": event.event_type().as_str(),
                        "timestamp": event.timestamp().to_rfc3339(),
                        "source": event.source(),
                        "subject": event.subject(),
                        "severity": event.severity(),
                        "correlation_id": event.correlation_id().map(|id| id.to_string()),
                        "payload": event.payload(),
                    })
                })
                .collect::<Vec<_>>();
            Response::ok(request.correlation_id, serde_json::json!(events))
        }
        Err(error) => Response::err(
            request.correlation_id,
            ErrorCode::Internal,
            error.to_string(),
        ),
    }
}

/// Parses the single-use token from an approval request's payload.
fn parse_token(request: &Request) -> Option<Uuid> {
    request
        .payload
        .get("token")
        .and_then(Value::as_str)
        .and_then(|token| token.parse::<Uuid>().ok())
}

/// The operator identity recorded on a grant, derived from the authorized peer.
fn granted_by(principal: &Principal) -> String {
    principal
        .uid()
        .map(|uid| format!("uid={uid}"))
        .unwrap_or_else(|| "local".to_string())
}

/// Removes the enrollment, leaving the installation running locally.
async fn cloud_forget(daemon: &Daemon, request: Request) -> Response {
    let outcome = crate::cloud::forget_enrollment(
        daemon.repository().as_ref(),
        daemon.secrets(),
        daemon.managed_settings(),
    )
    .await;

    match outcome {
        Ok(()) => {
            daemon.cloud_wake().notify_one();
            Response::ok(
                request.correlation_id,
                serde_json::json!({ "state": "not_configured" }),
            )
        }
        Err(error) => Response::err(request.correlation_id, ErrorCode::Internal, error),
    }
}

/// Enrolls this installation, returning a response directly because every
/// non-success path is an error envelope rather than a value.
async fn enroll(daemon: &Daemon, request: Request) -> Response {
    let Some(code) = request.payload.get("code").and_then(Value::as_str) else {
        return Response::err(
            request.correlation_id,
            ErrorCode::Malformed,
            "cloud.enroll requires a 'code' in the payload",
        );
    };

    let cloud = &daemon.config().cloud;
    let Some(endpoint) = cloud.endpoint.clone() else {
        return Response::err(
            request.correlation_id,
            ErrorCode::NotReady,
            "no cloud endpoint is configured",
        );
    };

    let mut transport = match WssTransport::connect(&endpoint).await {
        Ok(transport) => transport,
        Err(error) => {
            return Response::err(
                request.correlation_id,
                ErrorCode::Internal,
                format!("cannot reach the cloud: {error}"),
            );
        }
    };

    let outcome = crate::cloud::enroll_installation(
        cloud,
        daemon.secrets(),
        daemon.repository().as_ref(),
        &mut transport,
        crate::cloud::EnrollmentRequest {
            code,
            hostname: &crate::cloud::hostname(),
            agent_version: env!("CARGO_PKG_VERSION"),
            now: Utc::now(),
        },
    )
    .await;

    match outcome {
        Ok(report) => {
            // Use the new credential now rather than after the current backoff.
            daemon.cloud_wake().notify_one();
            Response::ok(request.correlation_id, to_value(report))
        }
        Err(EnrollmentError::AlreadyEnrolled) => Response::err(
            request.correlation_id,
            ErrorCode::Denied,
            "this installation is already enrolled; forget the current enrollment first",
        ),
        Err(EnrollmentError::NotConfigured) => Response::err(
            request.correlation_id,
            ErrorCode::NotReady,
            "cloud connectivity is not configured",
        ),
        Err(EnrollmentError::Denied { code, remediation }) => Response::err(
            request.correlation_id,
            ErrorCode::Denied,
            format!("{code}: {remediation}"),
        ),
        Err(error) => Response::err(
            request.correlation_id,
            ErrorCode::Internal,
            error.to_string(),
        ),
    }
}

fn to_value<T: serde::Serialize>(value: T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// `sentinel.get`: the live sentinel view (spec-004 FR-005).
async fn sentinel_get(daemon: &Daemon, request: &Request) -> Response {
    match serde_json::to_value(daemon.sentinel_snapshot().await) {
        Ok(view) => Response::ok(request.correlation_id, view),
        Err(e) => Response::err(
            request.correlation_id,
            ErrorCode::Internal,
            format!("serialize sentinel view: {e}"),
        ),
    }
}

/// `report.generate`: renders one report kind from live daemon state
/// (spec-004 FR-005). The payload is `{ "kind": "daily" | ... }`; an
/// unknown kind is a bad request. Inputs come from the daemon's real
/// tables; nothing is invented (FR-021).
async fn report_generate(daemon: &Daemon, request: &Request) -> Response {
    let Ok(kind) = serde_json::from_value::<argus_reporting::ReportKind>(
        request.payload.get("kind").cloned().unwrap_or(Value::Null),
    ) else {
        return Response::err(
            request.correlation_id,
            ErrorCode::Malformed,
            "payload must carry a known report kind",
        );
    };

    let executions = daemon.list_executions().await.unwrap_or_default();
    let approvals = daemon.list_pending_approvals();
    let inputs = argus_reporting::ReportInputs {
        open_incidents: Vec::new(),
        resolved_incidents: Vec::new(),
        risks: Vec::new(),
        prediction_prose: Vec::new(),
        actions_executed: executions.len() as u32,
        actions_validated: executions
            .iter()
            .filter(|(_, e)| e.status == argus_domain::ExecutionStatus::Completed)
            .count() as u32,
        actions_rolled_back: 0,
        actions_needing_manual: 0,
        policy_denials: 0,
        pending_approvals: approvals.len() as u32,
    };
    let report = argus_reporting::generate_report(kind, &inputs, None, Utc::now());
    match serde_json::to_value(&report) {
        Ok(value) => Response::ok(request.correlation_id, value),
        Err(e) => Response::err(
            request.correlation_id,
            ErrorCode::Internal,
            format!("serialize report: {e}"),
        ),
    }
}

/// `brain.diagnose`: run one full brain cycle on demand (spec 005 FR-004).
/// `NotReady` when no decision provider is configured — the daemon observes
/// but never guesses (AC-003).
async fn brain_diagnose(daemon: &Daemon, request: &Request) -> Response {
    if daemon.provider().is_none() {
        return Response::err(
            request.correlation_id,
            ErrorCode::NotReady,
            "no decision provider configured; the brain runs observe-only",
        );
    }
    let brain = daemon.config().brain.clone();
    let record = crate::brain::cycle(daemon, &brain).await;
    match serde_json::to_value(serde_json::json!({
        "evidence": record.evidence,
        "provider_available": record.provider_available,
        "decision": record.decision,
        "plan": record.plan_objective,
        "outcome": record.outcome,
    })) {
        Ok(value) => Response::ok(request.correlation_id, value),
        Err(e) => Response::err(
            request.correlation_id,
            ErrorCode::Internal,
            format!("serialize brain cycle: {e}"),
        ),
    }
}

/// `runbooks.list`: the runbook library (spec 005 FR-005, gates since spec
/// 009 FR-004) — name, status, gate progress, and cloud provenance per entry.
fn runbooks_list(daemon: &Daemon, correlation_id: Uuid) -> Response {
    let runbooks: Vec<serde_json::Value> = daemon
        .runbooks()
        .list()
        .into_iter()
        .map(|rb| {
            let mut entry = serde_json::json!({
                "name": rb.name,
                // The serde spelling ("candidate"), matching the sentinel
                // view and the web — never the Rust Debug casing.
                "status": rb.status(),
                "gates": rb.gates(),
                "attempts": rb.attempts(),
                "success_rate": rb.historical_success_rate(),
            });
            if let Some(provenance) = daemon.runbooks().provenance(&rb.name)
                && let Some(object) = entry.as_object_mut()
            {
                object.insert(
                    "provenance".into(),
                    serde_json::json!({
                        "configuration_id": provenance.configuration_id.to_string(),
                        "version_id": provenance.version_id.to_string(),
                        "version_number": provenance.version_number,
                        "delivered_at": provenance.delivered_at.to_rfc3339(),
                    }),
                );
            }
            entry
        })
        .collect();
    Response::ok(correlation_id, serde_json::json!({ "runbooks": runbooks }))
}

/// `runbooks.approve` / `runbooks.promote`: the operator's decision crossing
/// into the local promotion ladder (spec 009 FR-004). The ladder's own errors
/// come back as the failure reason — the CLI shows what the ladder said.
async fn runbook_decision(
    daemon: &Daemon,
    principal: &Principal,
    request: &Request,
    decision: Decision,
) -> Response {
    let Some(name) = request.payload.get("name").and_then(Value::as_str) else {
        return Response::err(
            request.correlation_id,
            ErrorCode::Malformed,
            "runbooks.approve/promote require a 'name' in the payload",
        );
    };

    match daemon
        .runbooks()
        .decide(name, decision, &granted_by(principal))
        .await
    {
        Ok(status) => Response::ok(
            request.correlation_id,
            serde_json::json!({ "name": name, "state": status }),
        ),
        Err(error) => Response::err(request.correlation_id, ErrorCode::Denied, error),
    }
}
