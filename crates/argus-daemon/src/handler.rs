//! Request dispatch: authorization and operation routing.

use argus_cloud::transport::websocket::WssTransport;
use argus_domain::Principal;
use argus_ipc::{ErrorCode, Operation, Request, Response};
use chrono::Utc;
use serde_json::Value;

use crate::cloud::EnrollmentError;
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
    };

    Response::ok(request.correlation_id, result)
}

/// Reports what this installation knows about its cloud relationship.
async fn cloud_status(daemon: &Daemon, request: Request) -> Response {
    let tracker = daemon.tracker();
    let tracker = tracker.lock().await;
    let queue = daemon.report_queue();
    let queue = queue.lock().await;

    let status = crate::cloud::cloud_status(
        &daemon.config().cloud,
        daemon.repository().as_ref(),
        &tracker,
        &queue,
    )
    .await;

    Response::ok(request.correlation_id, to_value(status))
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
        Ok(()) => Response::ok(
            request.correlation_id,
            serde_json::json!({ "state": "not_configured" }),
        ),
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
        Ok(report) => Response::ok(request.correlation_id, to_value(report)),
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
