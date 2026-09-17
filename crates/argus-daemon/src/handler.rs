//! Request dispatch: authorization and operation routing.

use argus_domain::Principal;
use argus_ipc::{ErrorCode, Operation, Request, Response};
use serde_json::Value;

use crate::runtime::Daemon;

/// Handles a single decoded request, enforcing authorization and dispatch.
pub fn handle(daemon: &Daemon, principal: Principal, request: Request) -> Response {
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
    };

    Response::ok(request.correlation_id, result)
}

fn to_value<T: serde::Serialize>(value: T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}
