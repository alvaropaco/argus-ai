//! In-memory runtime state for the bootstrap daemon.

use std::sync::Arc;
use std::time::Instant;

use argus_domain::{
    AuthorizationRequest, BlastRadius, CapabilityId, CapabilityRequest, EnvironmentId,
    HealthStatus, PolicyOutcome, RiskClass,
};
use argus_executor::{BootstrapExecutor, CapabilityProvider, ExecutionError, Executor};
use argus_policy::{BootstrapPolicyEvaluator, PolicyEvaluator};
use argus_state::{DomainRepository, RepositoryError, SqliteRepository};
use chrono::Utc;
use serde_json::Value;

use crate::config::DaemonConfig;

/// The running daemon and its bootstrap state.
pub struct Daemon {
    started_at: Instant,
    environment_id: EnvironmentId,
    config: DaemonConfig,
    repository: Arc<dyn DomainRepository>,
    policy: BootstrapPolicyEvaluator,
    executor: BootstrapExecutor,
}

/// Errors from the capability dispatch boundary.
#[derive(Debug, thiserror::Error)]
pub enum DispatchError {
    #[error("capability denied by policy: {0:?}")]
    Denied(PolicyOutcome),

    #[error("execution failed: {0}")]
    Execution(#[from] ExecutionError),
}

struct DaemonProvider {
    config: DaemonConfig,
}

impl CapabilityProvider for DaemonProvider {
    fn health(&self) -> HealthStatus {
        HealthStatus::ready(Utc::now())
    }

    fn config(&self) -> Value {
        serde_json::to_value(&self.config).unwrap_or(Value::Null)
    }

    fn plugins(&self) -> Value {
        serde_json::json!([])
    }

    fn host_status(&self) -> Value {
        serde_json::json!({ "status": "unavailable" })
    }

    fn list_capabilities(&self) -> Vec<CapabilityId> {
        bootstrap_capabilities()
    }
}

impl Daemon {
    /// Initializes the daemon, opening the state store and persisting/loading
    /// environment identity and an initial health record.
    pub async fn init(config: DaemonConfig) -> Result<Self, RepositoryError> {
        let repository: Arc<dyn DomainRepository> =
            Arc::new(SqliteRepository::open(&config.state_path)?);

        let environment_id = match repository.get_environment().await? {
            Some(id) => id,
            None => {
                let id = EnvironmentId::new();
                repository.save_environment(&id).await?;
                id
            }
        };
        repository
            .save_health(&HealthStatus::ready(Utc::now()))
            .await?;

        let provider = Arc::new(DaemonProvider {
            config: config.clone(),
        });

        Ok(Self {
            started_at: Instant::now(),
            environment_id,
            config,
            repository,
            policy: BootstrapPolicyEvaluator::new(),
            executor: BootstrapExecutor::new(provider),
        })
    }

    pub fn config(&self) -> &DaemonConfig {
        &self.config
    }

    pub fn environment_id(&self) -> EnvironmentId {
        self.environment_id
    }

    pub fn repository(&self) -> &Arc<dyn DomainRepository> {
        &self.repository
    }

    pub fn health(&self) -> HealthStatus {
        HealthStatus::ready(Utc::now())
    }

    pub fn status(&self) -> Value {
        serde_json::json!({
            "health": self.health(),
            "environment_id": self.environment_id.as_uuid().to_string(),
            "uptime_seconds": self.started_at.elapsed().as_secs(),
            "plugin_count": 0,
        })
    }

    pub fn plugins(&self) -> Value {
        serde_json::json!([])
    }

    pub fn capabilities(&self) -> Value {
        serde_json::json!(
            bootstrap_capabilities()
                .iter()
                .map(|c| c.as_str())
                .collect::<Vec<_>>()
        )
    }

    /// Routes a capability request through the policy and executor boundaries.
    ///
    /// A capability that is not allowed by policy is denied; an allowed
    /// capability is executed and its evidence is returned.
    pub fn authorize_and_execute(
        &self,
        request: CapabilityRequest,
    ) -> Result<Value, DispatchError> {
        let authz = AuthorizationRequest::new(request, RiskClass::Read, BlastRadius::None);
        let decision = self.policy.evaluate(&authz);

        if !decision.is_allowed() {
            return Err(DispatchError::Denied(decision.outcome));
        }

        let action = argus_executor::AuthorizedAction::new(authz.capability_request, decision)?;
        let result = self.executor.execute(&action)?;
        Ok(result.evidence)
    }
}

fn bootstrap_capabilities() -> Vec<CapabilityId> {
    [
        CapabilityId::HOST_STATUS_READ,
        CapabilityId::ARGUS_HEALTH_READ,
        CapabilityId::ARGUS_CONFIG_READ,
        CapabilityId::ARGUS_PLUGINS_LIST,
    ]
    .into_iter()
    .map(|c| CapabilityId::new(c).expect("bootstrap capability ids are valid"))
    .collect()
}
