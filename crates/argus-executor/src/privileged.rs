//! Privileged execution path for environment-changing capabilities.
//!
//! An operation only reaches this executor as an [`AuthorizedAction`], so the
//! structural gate is preserved: the cloud cannot reach the operating system
//! without a local `Allow`. What this module adds is reversal: when a capability
//! declares that its effect can be undone and the operation fails after taking
//! effect, the inverse operation is attempted, and the outcome is reported
//! distinctly rather than collapsed into a flat failure (ADR-0022 §7).

use std::sync::Arc;

use argus_domain::CapabilityId;
use chrono::Utc;
use serde_json::json;

use crate::action::{AuthorizedAction, ExecutionError, ExecutionResult, ReversalStatus};
use crate::executor::Executor;
use crate::service::{ServiceController, ServiceError};

/// A host-service operation, and its inverse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServiceOp {
    Restart,
    Stop,
    Start,
}

impl ServiceOp {
    /// The operation a capability names, if it names one this executor performs.
    fn of(capability: &CapabilityId) -> Option<Self> {
        match capability.as_str() {
            CapabilityId::HOST_SERVICE_RESTART => Some(Self::Restart),
            CapabilityId::HOST_SERVICE_STOP => Some(Self::Stop),
            CapabilityId::HOST_SERVICE_START => Some(Self::Start),
            _ => None,
        }
    }

    fn verb(self) -> &'static str {
        match self {
            Self::Restart => "restart",
            Self::Stop => "stop",
            Self::Start => "start",
        }
    }

    fn apply(self, services: &dyn ServiceController, unit: &str) -> Result<(), ServiceError> {
        match self {
            Self::Restart => services.restart(unit),
            Self::Stop => services.stop(unit),
            Self::Start => services.start(unit),
        }
    }

    /// The operation that undoes this one, when one exists.
    ///
    /// A stop is undone by a start and vice versa. A restart has no inverse: it
    /// neither creates nor removes a running unit, so a failure leaves the unit
    /// in a state a blind reversal would only disturb further.
    fn inverse(self) -> Option<Self> {
        match self {
            Self::Stop => Some(Self::Start),
            Self::Start => Some(Self::Stop),
            Self::Restart => None,
        }
    }
}

/// Executes environment-changing capabilities through a [`ServiceController`].
pub struct PrivilegedExecutor {
    services: Arc<dyn ServiceController>,
}

impl PrivilegedExecutor {
    pub fn new(services: Arc<dyn ServiceController>) -> Self {
        Self { services }
    }
}

impl Executor for PrivilegedExecutor {
    fn execute(&self, action: &AuthorizedAction) -> Result<ExecutionResult, ExecutionError> {
        let capability = action.capability();

        let Some(operation) = ServiceOp::of(capability) else {
            return Err(ExecutionError::Unsupported(capability.clone()));
        };

        let unit = action
            .request()
            .arguments
            .get("unit")
            .and_then(|value| value.as_str())
            .filter(|unit| !unit.trim().is_empty())
            .ok_or_else(|| {
                ExecutionError::Failed("the request does not name a service unit".to_string())
            })?
            .to_string();

        let started_at = Utc::now();

        match operation.apply(self.services.as_ref(), &unit) {
            Ok(()) => Ok(ExecutionResult {
                capability: capability.clone(),
                evidence: json!({
                    "unit": unit,
                    "operation": operation.verb(),
                    "reversal": ReversalStatus::NotAttempted,
                }),
                started_at,
                finished_at: Utc::now(),
            }),
            Err(error) => Err(ExecutionError::FailedAfterEffect {
                reversal: Self::attempt_reversal(operation, self.services.as_ref(), &unit),
                reason: error.to_string(),
            }),
        }
    }
}

impl PrivilegedExecutor {
    /// Attempts the inverse operation, or reports that none is possible.
    fn attempt_reversal(
        operation: ServiceOp,
        services: &dyn ServiceController,
        unit: &str,
    ) -> ReversalStatus {
        match operation.inverse() {
            None => ReversalStatus::NotPossible,
            Some(inverse) => match inverse.apply(services, unit) {
                Ok(()) => ReversalStatus::Reversed,
                Err(_) => ReversalStatus::ReversalFailed,
            },
        }
    }
}

impl PrivilegedExecutor {
    /// Whether this executor performs the named capability.
    pub fn handles(capability: &CapabilityId) -> bool {
        ServiceOp::of(capability).is_some()
    }
}

/// Routes an authorized action to the executor that performs it.
///
/// The published surface mixes read-only and environment-changing capabilities,
/// and every published capability must be executable through the same cloud path
/// with no shortcut (SC-016). Routing on the capability keeps both executors
/// behind the single [`AuthorizedAction`] gate.
pub struct CompositeExecutor {
    read_only: Arc<dyn Executor>,
    environment_changing: Arc<dyn Executor>,
}

impl CompositeExecutor {
    pub fn new(read_only: Arc<dyn Executor>, environment_changing: Arc<dyn Executor>) -> Self {
        Self {
            read_only,
            environment_changing,
        }
    }
}

impl Executor for CompositeExecutor {
    fn execute(&self, action: &AuthorizedAction) -> Result<ExecutionResult, ExecutionError> {
        if PrivilegedExecutor::handles(action.capability()) {
            self.environment_changing.execute(action)
        } else {
            self.read_only.execute(action)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use argus_domain::{
        CapabilityId, CapabilityRequest, PolicyDecision, Principal, RequestContext,
    };
    use chrono::Utc;
    use semver::Version;
    use serde_json::json;
    use uuid::Uuid;

    use super::*;

    /// A controller whose configured failure lets a test drive the reversal paths.
    #[derive(Default)]
    struct ScriptedController {
        fail: std::sync::Mutex<Vec<String>>,
        calls: std::sync::Mutex<Vec<String>>,
    }

    impl ScriptedController {
        fn failing(verbs: &[&str]) -> Self {
            Self {
                fail: std::sync::Mutex::new(verbs.iter().map(|v| v.to_string()).collect()),
                calls: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn record_and_check(&self, verb: &str) -> Result<(), ServiceError> {
            self.calls.lock().unwrap().push(verb.to_string());
            if self.fail.lock().unwrap().contains(&verb.to_string()) {
                return Err(ServiceError::Failed(format!("{verb} refused")));
            }
            Ok(())
        }

        fn recorded(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl ServiceController for ScriptedController {
        fn restart(&self, _unit: &str) -> Result<(), ServiceError> {
            self.record_and_check("restart")
        }
        fn stop(&self, _unit: &str) -> Result<(), ServiceError> {
            self.record_and_check("stop")
        }
        fn start(&self, _unit: &str) -> Result<(), ServiceError> {
            self.record_and_check("start")
        }
    }

    fn action_for(capability: &str, unit: &str) -> AuthorizedAction {
        let request = CapabilityRequest::new(
            CapabilityId::new(capability).unwrap(),
            Principal::new(Some(0), Some(0)),
            None,
            json!({ "unit": unit }),
            RequestContext::new(
                Uuid::new_v4(),
                Version::new(0, 1, 0),
                Principal::new(Some(0), Some(0)),
                Utc::now(),
            ),
        );
        AuthorizedAction::new(request, PolicyDecision::allow("test", "ok")).expect("allowed")
    }

    #[test]
    fn a_service_restart_executes_and_records_the_unit() {
        let controller = Arc::new(ScriptedController::default());
        let executor = PrivilegedExecutor::new(controller.clone());

        let result = executor
            .execute(&action_for(
                CapabilityId::HOST_SERVICE_RESTART,
                "nginx.service",
            ))
            .expect("restart succeeds");

        assert_eq!(result.evidence["unit"], "nginx.service");
        assert_eq!(result.evidence["reversal"], json!("not_attempted"));
        assert_eq!(controller.recorded(), vec!["restart"]);
    }

    #[test]
    fn an_unsupported_capability_never_reaches_the_controller() {
        let controller = Arc::new(ScriptedController::default());
        let executor = PrivilegedExecutor::new(controller.clone());

        let err = executor
            .execute(&action_for("host.process.signal", "nginx.service"))
            .unwrap_err();

        assert!(matches!(err, ExecutionError::Unsupported(_)));
        assert!(controller.recorded().is_empty());
    }

    #[test]
    fn a_request_without_a_unit_is_refused_before_touching_the_host() {
        let controller = Arc::new(ScriptedController::default());
        let executor = PrivilegedExecutor::new(controller.clone());

        let request = CapabilityRequest::new(
            CapabilityId::new(CapabilityId::HOST_SERVICE_RESTART).unwrap(),
            Principal::new(Some(0), Some(0)),
            None,
            json!({}),
            RequestContext::new(
                Uuid::new_v4(),
                Version::new(0, 1, 0),
                Principal::new(Some(0), Some(0)),
                Utc::now(),
            ),
        );
        let action =
            AuthorizedAction::new(request, PolicyDecision::allow("test", "ok")).expect("allowed");

        let err = executor.execute(&action).unwrap_err();
        assert!(matches!(err, ExecutionError::Failed(_)));
        assert!(controller.recorded().is_empty());
    }

    #[test]
    fn a_failed_stop_is_reversed_by_a_start() {
        let controller = Arc::new(ScriptedController::failing(&["stop"]));
        let executor = PrivilegedExecutor::new(controller.clone());

        let err = executor
            .execute(&action_for(
                CapabilityId::HOST_SERVICE_STOP,
                "nginx.service",
            ))
            .unwrap_err();

        assert!(
            matches!(
                err,
                ExecutionError::FailedAfterEffect {
                    reversal: ReversalStatus::Reversed,
                    ..
                }
            ),
            "a reversible failure must report that it was undone: {err:?}"
        );
        assert_eq!(
            controller.recorded(),
            vec!["stop", "start"],
            "the reversal must run after the failure"
        );
    }

    #[test]
    fn a_failed_start_is_reversed_by_a_stop() {
        let controller = Arc::new(ScriptedController::failing(&["start"]));
        let executor = PrivilegedExecutor::new(controller.clone());

        let err = executor
            .execute(&action_for(
                CapabilityId::HOST_SERVICE_START,
                "nginx.service",
            ))
            .unwrap_err();

        assert!(matches!(
            err,
            ExecutionError::FailedAfterEffect {
                reversal: ReversalStatus::Reversed,
                ..
            }
        ));
        assert_eq!(controller.recorded(), vec!["start", "stop"]);
    }

    #[test]
    fn a_failed_reversal_is_reported_as_such_and_not_as_success() {
        let controller = Arc::new(ScriptedController::failing(&["stop", "start"]));
        let executor = PrivilegedExecutor::new(controller.clone());

        let err = executor
            .execute(&action_for(
                CapabilityId::HOST_SERVICE_STOP,
                "nginx.service",
            ))
            .unwrap_err();

        assert!(
            matches!(
                err,
                ExecutionError::FailedAfterEffect {
                    reversal: ReversalStatus::ReversalFailed,
                    ..
                }
            ),
            "a reversal that itself failed must be distinguishable: {err:?}"
        );
        assert_eq!(controller.recorded(), vec!["stop", "start"]);
    }

    #[test]
    fn a_failed_restart_has_no_inverse_and_says_so() {
        let controller = Arc::new(ScriptedController::failing(&["restart"]));
        let executor = PrivilegedExecutor::new(controller.clone());

        let err = executor
            .execute(&action_for(
                CapabilityId::HOST_SERVICE_RESTART,
                "nginx.service",
            ))
            .unwrap_err();

        assert!(matches!(
            err,
            ExecutionError::FailedAfterEffect {
                reversal: ReversalStatus::NotPossible,
                ..
            }
        ));
        assert_eq!(
            controller.recorded(),
            vec!["restart"],
            "a restart must not be blindly re-run as a reversal"
        );
    }
}
