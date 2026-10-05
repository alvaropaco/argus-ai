//! Typed remediation capabilities (spec 003 M3, CAP-14 host, FR-017).
//!
//! Three controller families back the remediation capabilities:
//! [`ProcessController`] (`host.process.signal`), [`ContainerController`]
//! (`container.restart`), and [`CgroupController`] (`host.cgroup.freeze` /
//! `host.cgroup.thaw`, the resource-control adjustment). Each is a port so
//! tests run against deterministic mocks and the kernel-facing implementations
//! stay thin (Principle 9, ADR-0032).
//!
//! Every effect crosses the same boundary as the bootstrap executors: only an
//! [`AuthorizedAction`] reaches [`RemediationExecutor`], and guardrails run at
//! execution time on the freshly re-resolved target, immediately before the
//! effect (ADR-0028 §1-§2).

use std::sync::Arc;

use argus_domain::CapabilityId;
use chrono::Utc;
use serde_json::json;

use crate::action::{AuthorizedAction, ExecutionError, ExecutionResult};
use crate::executor::Executor;
use crate::guardrail::GuardrailRegistry;

/// The signals a remediation plan may deliver. The set is closed: anything
/// outside it is refused before it becomes a syscall argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessSignal {
    /// Graceful termination.
    Term,
    /// Unconditional termination; irreversible, so a plan step carrying it
    /// declares no rollback.
    Kill,
    /// Suspend the process (the cgroup-freezer's per-process counterpart).
    Stop,
    /// Resume a suspended process; the declared rollback of `Stop`.
    Cont,
}

impl ProcessSignal {
    /// Parses the wire value carried in an action's `signal` argument.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "term" => Some(Self::Term),
            "kill" => Some(Self::Kill),
            "stop" => Some(Self::Stop),
            "cont" => Some(Self::Cont),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Term => "term",
            Self::Kill => "kill",
            Self::Stop => "stop",
            Self::Cont => "cont",
        }
    }
}

/// Errors from the remediation controllers.
#[derive(Debug, thiserror::Error)]
pub enum RemediationError {
    #[error("remediation target is unavailable: {0}")]
    Unavailable(String),

    #[error("remediation operation failed: {0}")]
    Failed(String),
}

/// Delivers signals to processes backing `host.process.signal`.
pub trait ProcessController: Send + Sync {
    fn signal(&self, pid: i32, signal: ProcessSignal) -> Result<(), RemediationError>;

    /// Whether the process exists, read from live state.
    fn exists(&self, pid: i32) -> Result<bool, RemediationError>;
}

/// A `kill(2)`-backed controller through the safe `nix` wrappers (the workspace
/// forbids `unsafe`, so the syscall boundary lives in the audited dependency).
/// Typed signal constants, never shell strings.
#[derive(Debug, Default)]
pub struct UnixProcessController;

impl UnixProcessController {
    pub fn new() -> Self {
        Self
    }

    fn nix_signal(signal: ProcessSignal) -> nix::sys::signal::Signal {
        match signal {
            ProcessSignal::Term => nix::sys::signal::Signal::SIGTERM,
            ProcessSignal::Kill => nix::sys::signal::Signal::SIGKILL,
            ProcessSignal::Stop => nix::sys::signal::Signal::SIGSTOP,
            ProcessSignal::Cont => nix::sys::signal::Signal::SIGCONT,
        }
    }
}

impl ProcessController for UnixProcessController {
    fn signal(&self, pid: i32, signal: ProcessSignal) -> Result<(), RemediationError> {
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(pid),
            Some(Self::nix_signal(signal)),
        )
        .map_err(|e| {
            RemediationError::Failed(format!("kill({pid}, {}) failed: {e}", signal.name()))
        })
    }

    fn exists(&self, pid: i32) -> Result<bool, RemediationError> {
        // `None` probes existence without delivering a signal.
        match nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None) {
            Ok(()) => Ok(true),
            Err(nix::errno::Errno::ESRCH) => Ok(false),
            Err(nix::errno::Errno::EPERM) => Ok(true),
            Err(e) => Err(RemediationError::Failed(format!(
                "probing pid {pid} failed: {e}"
            ))),
        }
    }
}

/// Restarts containers backing `container.restart`.
pub trait ContainerController: Send + Sync {
    fn restart(&self, id: &str) -> Result<(), RemediationError>;

    /// Whether the container is currently running, read from live state.
    fn is_running(&self, id: &str) -> Result<bool, RemediationError>;
}

/// A Docker-socket-backed controller over the blocking adapter client.
#[derive(Debug, Clone)]
pub struct DockerContainerController {
    client: argus_container::SyncDockerClient,
}

impl DockerContainerController {
    pub fn new() -> Self {
        Self {
            client: argus_container::SyncDockerClient::new(),
        }
    }

    pub fn with_socket(socket_path: impl Into<String>) -> Self {
        Self {
            client: argus_container::SyncDockerClient::with_socket(socket_path),
        }
    }
}

impl Default for DockerContainerController {
    fn default() -> Self {
        Self::new()
    }
}

impl ContainerController for DockerContainerController {
    fn restart(&self, id: &str) -> Result<(), RemediationError> {
        self.client
            .restart_container(id)
            .map_err(|e| RemediationError::Failed(e.to_string()))
    }

    fn is_running(&self, id: &str) -> Result<bool, RemediationError> {
        self.client.container_running(id).map_err(|e| match e {
            argus_container::ContainerError::Unavailable(_) => {
                RemediationError::Unavailable(e.to_string())
            }
            _ => RemediationError::Failed(e.to_string()),
        })
    }
}

/// Freezes and thaws cgroup v2 subtrees backing the resource-control
/// capabilities. The freezer is the pressure-remediation primitive: it pauses
/// a non-critical consumer without killing it, and a thaw undoes it exactly.
pub trait CgroupController: Send + Sync {
    fn set_freeze(&self, path: &str, frozen: bool) -> Result<(), RemediationError>;

    /// Whether the subtree is currently frozen, read from live state.
    fn is_frozen(&self, path: &str) -> Result<bool, RemediationError>;
}

/// A cgroup v2 unified-hierarchy controller writing `cgroup.freeze` under a
/// configured mount root (kernel-native: a single file write, no CLI).
#[derive(Debug, Clone)]
pub struct CgroupV2Controller {
    root: String,
}

impl CgroupV2Controller {
    pub fn with_root(root: impl Into<String>) -> Self {
        Self { root: root.into() }
    }

    /// The freeze file for `path`, refusing traversal before it becomes a
    /// filesystem path.
    fn freeze_file(&self, path: &str) -> Result<String, RemediationError> {
        validate_cgroup_path(path)?;
        Ok(format!("{}/cgroup.freeze", self.trim_root(path)))
    }

    fn trim_root(&self, path: &str) -> String {
        let root = self.root.trim_end_matches('/');
        let path = path.trim_start_matches('/');
        if path.is_empty() {
            root.to_string()
        } else {
            format!("{root}/{path}")
        }
    }

    fn write_freeze(&self, path: &str, frozen: bool) -> Result<(), RemediationError> {
        std::fs::write(self.freeze_file(path)?, if frozen { "1" } else { "0" })
            .map_err(|e| RemediationError::Failed(format!("cgroup.freeze write failed: {e}")))
    }
}

impl Default for CgroupV2Controller {
    fn default() -> Self {
        Self::with_root("/sys/fs/cgroup")
    }
}

impl CgroupController for CgroupV2Controller {
    fn set_freeze(&self, path: &str, frozen: bool) -> Result<(), RemediationError> {
        self.write_freeze(path, frozen)
    }

    fn is_frozen(&self, path: &str) -> Result<bool, RemediationError> {
        let raw = std::fs::read_to_string(self.freeze_file(path)?)
            .map_err(|e| RemediationError::Failed(format!("cgroup.freeze read failed: {e}")))?;
        Ok(raw.trim() == "1")
    }
}

/// A cgroup path is relative to the mount root, non-empty, and free of
/// traversal: no leading `/`, no `..` component, no empty components.
pub fn validate_cgroup_path(path: &str) -> Result<(), RemediationError> {
    if path.is_empty() {
        return Err(RemediationError::Failed("cgroup path is empty".to_string()));
    }
    if path.starts_with('/') {
        return Err(RemediationError::Failed(
            "cgroup path must be relative to the mount root".to_string(),
        ));
    }
    if path
        .split('/')
        .any(|component| component.is_empty() || component == "..")
    {
        return Err(RemediationError::Failed(format!(
            "cgroup path '{path}' contains an empty or '..' component"
        )));
    }
    Ok(())
}

/// Executes the typed remediation capabilities through their controllers.
///
/// Routing is by capability family, and every family's guardrails run here, at
/// the execution boundary, before the effect (ADR-0028 §1-§2).
pub struct RemediationExecutor {
    processes: Arc<dyn ProcessController>,
    containers: Arc<dyn ContainerController>,
    cgroups: Arc<dyn CgroupController>,
    guardrails: GuardrailRegistry,
}

impl RemediationExecutor {
    pub fn new(
        processes: Arc<dyn ProcessController>,
        containers: Arc<dyn ContainerController>,
        cgroups: Arc<dyn CgroupController>,
        guardrails: GuardrailRegistry,
    ) -> Self {
        Self {
            processes,
            containers,
            cgroups,
            guardrails,
        }
    }

    /// Whether this executor performs the named capability.
    pub fn handles(capability: &CapabilityId) -> bool {
        matches!(
            capability.as_str(),
            CapabilityId::HOST_PROCESS_SIGNAL
                | CapabilityId::CONTAINER_RESTART
                | CapabilityId::HOST_CGROUP_FREEZE
                | CapabilityId::HOST_CGROUP_THAW
        )
    }

    fn signal_pid(&self, action: &AuthorizedAction) -> Result<ExecutionResult, ExecutionError> {
        let capability = action.capability();
        let arguments = &action.request().arguments;
        let pid = arguments
            .get("pid")
            .and_then(serde_json::Value::as_i64)
            .and_then(|pid| i32::try_from(pid).ok())
            .ok_or_else(|| {
                ExecutionError::Failed(
                    "the request does not name a valid numeric 'pid'".to_string(),
                )
            })?;
        let signal = arguments
            .get("signal")
            .and_then(serde_json::Value::as_str)
            .and_then(ProcessSignal::parse)
            .ok_or_else(|| {
                ExecutionError::Failed(
                    "the request does not name a signal in {term,kill,stop,cont}".to_string(),
                )
            })?;

        self.guardrails.check(capability, &pid.to_string())?;

        let started_at = Utc::now();
        self.processes
            .signal(pid, signal)
            .map_err(|e| ExecutionError::Failed(e.to_string()))?;
        Ok(ExecutionResult {
            capability: capability.clone(),
            evidence: json!({ "pid": pid, "signal": signal.name() }),
            started_at,
            finished_at: Utc::now(),
        })
    }

    fn restart_container(
        &self,
        action: &AuthorizedAction,
    ) -> Result<ExecutionResult, ExecutionError> {
        let capability = action.capability();
        let id = action
            .request()
            .arguments
            .get("container")
            .and_then(serde_json::Value::as_str)
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| {
                ExecutionError::Failed("the request does not name a 'container'".to_string())
            })?
            .to_string();

        self.guardrails.check(capability, &id)?;

        let started_at = Utc::now();
        self.containers
            .restart(&id)
            .map_err(|e| ExecutionError::Failed(e.to_string()))?;
        Ok(ExecutionResult {
            capability: capability.clone(),
            evidence: json!({ "container": id, "operation": "restart" }),
            started_at,
            finished_at: Utc::now(),
        })
    }

    fn set_cgroup_freeze(
        &self,
        action: &AuthorizedAction,
        frozen: bool,
    ) -> Result<ExecutionResult, ExecutionError> {
        let capability = action.capability();
        let path = action
            .request()
            .arguments
            .get("path")
            .and_then(serde_json::Value::as_str)
            .filter(|path| !path.trim().is_empty())
            .ok_or_else(|| {
                ExecutionError::Failed("the request does not name a cgroup 'path'".to_string())
            })?
            .to_string();

        self.guardrails.check(capability, &path)?;

        let started_at = Utc::now();
        self.cgroups
            .set_freeze(&path, frozen)
            .map_err(|e| ExecutionError::Failed(e.to_string()))?;
        Ok(ExecutionResult {
            capability: capability.clone(),
            evidence: json!({ "path": path, "frozen": frozen }),
            started_at,
            finished_at: Utc::now(),
        })
    }
}

impl Executor for RemediationExecutor {
    fn execute(&self, action: &AuthorizedAction) -> Result<ExecutionResult, ExecutionError> {
        let capability = action.capability();
        match capability.as_str() {
            CapabilityId::HOST_PROCESS_SIGNAL => self.signal_pid(action),
            CapabilityId::CONTAINER_RESTART => self.restart_container(action),
            CapabilityId::HOST_CGROUP_FREEZE => self.set_cgroup_freeze(action, true),
            CapabilityId::HOST_CGROUP_THAW => self.set_cgroup_freeze(action, false),
            _ => Err(ExecutionError::Unsupported(capability.clone())),
        }
    }
}

/// Guardrails wired for the remediation capabilities (the daemon registers
/// these when it builds the executor).
///
/// - `host.process.signal`: the pid must be above 1 and must never be this
///   process — argusd does not signal itself or init.
/// - `container.restart`: the id is re-validated by the adapter, and any
///   configured protected container is refused.
/// - `host.cgroup.freeze`/`thaw`: the path must be traversal-free, and any
///   configured protected path is refused.
pub fn remediation_guardrails(
    protected_containers: &[String],
    protected_cgroups: &[String],
) -> GuardrailRegistry {
    let mut registry = GuardrailRegistry::new();

    let signal = CapabilityId::new(CapabilityId::HOST_PROCESS_SIGNAL)
        .expect("remediation capability ids are valid");
    registry.register(signal.clone(), Arc::new(NeverSignalSelf));
    registry.register(signal, Arc::new(PidAboveOne));

    let restart = CapabilityId::new(CapabilityId::CONTAINER_RESTART)
        .expect("remediation capability ids are valid");
    registry.register(
        restart,
        Arc::new(NeverTarget::new(protected_containers.iter().cloned())),
    );

    for capability in [
        CapabilityId::HOST_CGROUP_FREEZE,
        CapabilityId::HOST_CGROUP_THAW,
    ] {
        let id = CapabilityId::new(capability).expect("remediation capability ids are valid");
        registry.register(id.clone(), Arc::new(ValidCgroupPath));
        registry.register(
            id,
            Arc::new(NeverTarget::new(protected_cgroups.iter().cloned())),
        );
    }
    registry
}

/// Refuses any target on a protected denylist: the opposite shape of
/// [`crate::guardrail::AllowedTargets`], for families where everything is
/// permitted except a small protected set.
#[derive(Debug, Default)]
pub struct NeverTarget {
    protected: std::collections::BTreeSet<String>,
}

impl NeverTarget {
    pub fn new(protected: impl IntoIterator<Item = String>) -> Self {
        Self {
            protected: protected.into_iter().collect(),
        }
    }
}

impl crate::guardrail::Guardrail for NeverTarget {
    fn check(&self, target: &str) -> Result<(), String> {
        if self.protected.contains(target) {
            return Err(format!("'{target}' is a protected remediation target"));
        }
        Ok(())
    }
}

/// Refuses the executor's own pid: remediation never signals argusd.
#[derive(Debug, Default)]
pub struct NeverSignalSelf;

impl crate::guardrail::Guardrail for NeverSignalSelf {
    fn check(&self, target: &str) -> Result<(), String> {
        let pid: i32 = target
            .parse()
            .map_err(|_| format!("target '{target}' is not a pid"))?;
        if pid == std::process::id() as i32 {
            return Err("refusing to signal this daemon's own process".to_string());
        }
        Ok(())
    }
}

/// Refuses pids at or below 1: init and invalid pids are never targets.
#[derive(Debug, Default)]
pub struct PidAboveOne;

impl crate::guardrail::Guardrail for PidAboveOne {
    fn check(&self, target: &str) -> Result<(), String> {
        let pid: i32 = target
            .parse()
            .map_err(|_| format!("target '{target}' is not a pid"))?;
        crate::guardrail::pid_above_one(pid.into())
    }
}

/// Refuses cgroup paths that are not safe relative subtrees.
#[derive(Debug, Default)]
pub struct ValidCgroupPath;

impl crate::guardrail::Guardrail for ValidCgroupPath {
    fn check(&self, target: &str) -> Result<(), String> {
        validate_cgroup_path(target).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use argus_domain::{CapabilityRequest, PolicyDecision, Principal, RequestContext};
    use chrono::Utc;
    use semver::Version;
    use serde_json::json;
    use std::sync::Mutex;
    use uuid::Uuid;

    /// A controller whose calls a test can assert on.
    #[derive(Default)]
    struct MockProcesses {
        calls: Mutex<Vec<(i32, String)>>,
    }

    impl ProcessController for MockProcesses {
        fn signal(&self, pid: i32, signal: ProcessSignal) -> Result<(), RemediationError> {
            self.calls
                .lock()
                .unwrap()
                .push((pid, signal.name().to_string()));
            Ok(())
        }

        fn exists(&self, _pid: i32) -> Result<bool, RemediationError> {
            Ok(true)
        }
    }

    #[derive(Default)]
    struct MockContainers;

    impl ContainerController for MockContainers {
        fn restart(&self, id: &str) -> Result<(), RemediationError> {
            let _ = id;
            Ok(())
        }

        fn is_running(&self, _id: &str) -> Result<bool, RemediationError> {
            Ok(true)
        }
    }

    #[derive(Default)]
    struct MockCgroups;

    impl CgroupController for MockCgroups {
        fn set_freeze(&self, _path: &str, _frozen: bool) -> Result<(), RemediationError> {
            Ok(())
        }

        fn is_frozen(&self, _path: &str) -> Result<bool, RemediationError> {
            Ok(false)
        }
    }

    fn action_for(capability: &str, arguments: serde_json::Value) -> AuthorizedAction {
        let request = CapabilityRequest::new(
            CapabilityId::new(capability).unwrap(),
            Principal::new(Some(0), Some(0)),
            None,
            arguments,
            RequestContext::new(
                Uuid::new_v4(),
                Version::new(0, 1, 0),
                Principal::new(Some(0), Some(0)),
                Utc::now(),
            ),
        );
        AuthorizedAction::new(request, PolicyDecision::allow("test", "ok")).expect("allowed")
    }

    fn executor(protected_containers: &[&str], protected_cgroups: &[&str]) -> RemediationExecutor {
        let guardrails = remediation_guardrails(
            &protected_containers
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
            &protected_cgroups
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
        );
        RemediationExecutor::new(
            Arc::new(MockProcesses::default()),
            Arc::new(MockContainers),
            Arc::new(MockCgroups),
            guardrails,
        )
    }

    #[test]
    fn the_executor_handles_exactly_the_remediation_capabilities() {
        for capability in [
            CapabilityId::HOST_PROCESS_SIGNAL,
            CapabilityId::CONTAINER_RESTART,
            CapabilityId::HOST_CGROUP_FREEZE,
            CapabilityId::HOST_CGROUP_THAW,
        ] {
            assert!(RemediationExecutor::handles(
                &CapabilityId::new(capability).unwrap()
            ));
        }
        assert!(!RemediationExecutor::handles(
            &CapabilityId::new(CapabilityId::HOST_SERVICE_RESTART).unwrap()
        ));
    }

    #[test]
    fn a_signal_reaches_its_controller_with_typed_arguments() {
        let processes = MockProcesses::default();
        let executor = RemediationExecutor::new(
            Arc::new(processes),
            Arc::new(MockContainers),
            Arc::new(MockCgroups),
            remediation_guardrails(&[], &[]),
        );
        let result = executor
            .execute(&action_for(
                CapabilityId::HOST_PROCESS_SIGNAL,
                json!({ "pid": 4242, "signal": "term" }),
            ))
            .expect("signal executes");
        assert_eq!(result.evidence["pid"], 4242);
        assert_eq!(result.evidence["signal"], "term");
    }

    #[test]
    fn an_unknown_signal_or_missing_pid_is_refused_before_any_syscall() {
        let executor = executor(&[], &[]);
        let err = executor
            .execute(&action_for(
                CapabilityId::HOST_PROCESS_SIGNAL,
                json!({ "pid": 4242, "signal": "sig9" }),
            ))
            .unwrap_err();
        assert!(matches!(err, ExecutionError::Failed(_)), "{err:?}");

        let err = executor
            .execute(&action_for(
                CapabilityId::HOST_PROCESS_SIGNAL,
                json!({ "signal": "term" }),
            ))
            .unwrap_err();
        assert!(matches!(err, ExecutionError::Failed(_)), "{err:?}");
    }

    #[test]
    fn init_and_this_process_are_never_signal_targets() {
        let executor = executor(&[], &[]);
        for pid in [1, std::process::id() as i32] {
            let err = executor
                .execute(&action_for(
                    CapabilityId::HOST_PROCESS_SIGNAL,
                    json!({ "pid": pid, "signal": "term" }),
                ))
                .unwrap_err();
            assert!(
                matches!(err, ExecutionError::GuardrailViolation(_)),
                "pid {pid} must be refused by a guardrail: {err:?}"
            );
        }
    }

    #[test]
    fn a_container_restart_reaches_its_controller() {
        let executor = executor(&[], &[]);
        let result = executor
            .execute(&action_for(
                CapabilityId::CONTAINER_RESTART,
                json!({ "container": "abc123" }),
            ))
            .expect("restart executes");
        assert_eq!(result.evidence["container"], "abc123");
    }

    #[test]
    fn a_protected_container_is_refused_before_the_socket() {
        let executor = executor(&["argus-own"], &[]);
        let err = executor
            .execute(&action_for(
                CapabilityId::CONTAINER_RESTART,
                json!({ "container": "argus-own" }),
            ))
            .unwrap_err();
        assert!(
            matches!(err, ExecutionError::GuardrailViolation(_)),
            "{err:?}"
        );
    }

    #[test]
    fn a_freeze_and_a_thaw_reach_the_cgroup_controller() {
        let executor = executor(&[], &[]);
        executor
            .execute(&action_for(
                CapabilityId::HOST_CGROUP_FREEZE,
                json!({ "path": "workload.batch/app" }),
            ))
            .expect("freeze executes");
        executor
            .execute(&action_for(
                CapabilityId::HOST_CGROUP_THAW,
                json!({ "path": "workload.batch/app" }),
            ))
            .expect("thaw executes");
    }

    #[test]
    fn a_cgroup_path_with_traversal_is_refused() {
        let executor = executor(&[], &[]);
        for path in ["/abs/path", "a/../b", "a//b", ""] {
            let err = executor
                .execute(&action_for(
                    CapabilityId::HOST_CGROUP_FREEZE,
                    json!({ "path": path }),
                ))
                .unwrap_err();
            assert!(
                matches!(
                    err,
                    ExecutionError::GuardrailViolation(_) | ExecutionError::Failed(_)
                ),
                "path '{path}' must be refused: {err:?}"
            );
        }
    }

    #[test]
    fn a_protected_cgroup_is_refused_even_though_the_path_is_valid() {
        let executor = executor(&[], &["system.slice/argusd"]);
        let err = executor
            .execute(&action_for(
                CapabilityId::HOST_CGROUP_FREEZE,
                json!({ "path": "system.slice/argusd" }),
            ))
            .unwrap_err();
        assert!(
            matches!(err, ExecutionError::GuardrailViolation(_)),
            "{err:?}"
        );
    }

    #[test]
    fn a_service_capability_is_unsupported_here() {
        let executor = executor(&[], &[]);
        let err = executor
            .execute(&action_for(
                CapabilityId::HOST_SERVICE_RESTART,
                json!({ "unit": "nginx.service" }),
            ))
            .unwrap_err();
        assert!(matches!(err, ExecutionError::Unsupported(_)), "{err:?}");
    }

    #[test]
    fn cgroup_v2_paths_are_validated_as_relative_subtrees() {
        assert!(validate_cgroup_path("system.slice/nginx.service").is_ok());
        assert!(validate_cgroup_path("workload.batch").is_ok());
        assert!(validate_cgroup_path("").is_err());
        assert!(validate_cgroup_path("/absolute").is_err());
        assert!(validate_cgroup_path("..").is_err());
        assert!(validate_cgroup_path("a/..").is_err());
        assert!(validate_cgroup_path("a//b").is_err());
    }

    #[test]
    fn the_unix_process_controller_reports_missing_processes() {
        let controller = UnixProcessController::new();
        // Probing with signal 0: a pid that cannot exist reports ESRCH.
        assert!(!controller.exists(i32::MAX).unwrap());
    }
}
