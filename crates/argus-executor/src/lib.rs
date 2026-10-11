//! Typed, policy-authorized execution boundary.
//!
//! The only component allowed to cross the privileged execution boundary.
//! Actions can only be executed after carrying an `Allow` policy decision;
//! the bootstrap exposes read-only capabilities only (no arbitrary shell).

mod action;
mod bootstrap;
mod cluster;
mod executor;
mod guardrail;
mod privileged;
mod provider;
mod remediation;
mod service;

pub use action::{AuthorizedAction, ExecutionError, ExecutionResult, ReversalStatus};
pub use bootstrap::BootstrapExecutor;
pub use cluster::{
    ClusterController, KubernetesExecutor, MaxReplicas, ProtectedNamespaces,
    UnavailableClusterController, ValidK8sName, kubernetes_guardrails,
};
pub use executor::Executor;
pub use guardrail::{
    AllowedTargets, Guardrail, GuardrailRegistry, GuardrailViolation, NeverTargetArgusd,
    ValidServiceUnit, pid_above_one,
};
pub use privileged::{CompositeExecutor, PrivilegedExecutor};
pub use provider::CapabilityProvider;
pub use remediation::{
    CgroupController, CgroupV2Controller, ContainerController, DockerContainerController,
    DropCachesController, JournalController, KernelWriteController, MAX_KEEP_DAYS, MIN_KEEP_DAYS,
    NeverSignalSelf, NeverTarget, PidAboveOne, ProcessController, ProcessSignal, RemediationError,
    RemediationExecutor, UnixProcessController, ValidCgroupPath, remediation_guardrails,
    validate_cgroup_path,
};
pub use service::{
    MockServiceController, ServiceController, ServiceError, SystemdServiceController,
};
