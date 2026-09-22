//! SQLite-backed repository (interim default backend).
//!
//! See ADR-011: LanceDB is the intended operational store; SQLite is used as
//! a lean interim backend while the LanceDB Rust SDK is validated. Both are
//! behind the [`DomainRepository`](crate::DomainRepository) trait.

use std::path::Path;
use std::sync::Mutex;

use argus_domain::{
    AppliedConfigurationState, CapabilityPublication, CloudCommand, CloudConnection,
    CloudEnrollment, DomainEvent, EnvironmentId, ExecutionApproval, ExecutionDecision,
    HealthStatus, ManagedConfiguration, Observation, ReportBuffer,
};
use async_trait::async_trait;
use rusqlite::Connection;
use uuid::Uuid;

use crate::repository::{DomainRepository, RepositoryError};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS environment (
    id   TEXT PRIMARY KEY,
    data TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS health (
    id   INTEGER PRIMARY KEY CHECK (id = 1),
    data TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS observations (
    id   TEXT PRIMARY KEY,
    data TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS audit_events (
    id   TEXT PRIMARY KEY,
    data TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS cloud_enrollment (
    id   TEXT PRIMARY KEY,
    data TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS cloud_connection (
    id   TEXT PRIMARY KEY,
    data TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS cloud_commands (
    id     TEXT PRIMARY KEY,
    status TEXT NOT NULL,
    data   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS cloud_commands_status ON cloud_commands (status);
CREATE TABLE IF NOT EXISTS cloud_decisions (
    id   TEXT PRIMARY KEY,
    data TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS cloud_approvals (
    id   TEXT PRIMARY KEY,
    data TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS cloud_publications (
    id   TEXT PRIMARY KEY,
    data TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS cloud_managed_configurations (
    id   TEXT PRIMARY KEY,
    data TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS cloud_applied_configurations (
    id   TEXT PRIMARY KEY,
    data TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS cloud_report_buffer (
    id   TEXT PRIMARY KEY,
    data TEXT NOT NULL
);
"#;

const SINGLETON: &str = "cloud";

/// Non-terminal command statuses, matching `CloudCommandStatus` serialisation.
const UNFINISHED_STATUSES: &str = "'received','executing'";

/// Every table holding cloud-derived state, cleared by `clear_cloud_state`.
const CLOUD_TABLES: &[&str] = &[
    "cloud_enrollment",
    "cloud_connection",
    "cloud_commands",
    "cloud_decisions",
    "cloud_approvals",
    "cloud_publications",
    "cloud_managed_configurations",
    "cloud_applied_configurations",
    "cloud_report_buffer",
];

/// A [`DomainRepository`] backed by SQLite.
pub struct SqliteRepository {
    conn: Mutex<Connection>,
}

impl From<rusqlite::Error> for RepositoryError {
    fn from(e: rusqlite::Error) -> Self {
        RepositoryError::Failed(e.to_string())
    }
}

impl SqliteRepository {
    /// Opens (or creates) a SQLite database at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, RepositoryError> {
        let conn =
            Connection::open(path).map_err(|e| RepositoryError::Unavailable(e.to_string()))?;
        let repo = Self {
            conn: Mutex::new(conn),
        };
        repo.init_schema()?;
        Ok(repo)
    }

    /// Opens an in-memory database (useful for tests).
    pub fn open_in_memory() -> Result<Self, RepositoryError> {
        let conn = Connection::open_in_memory()
            .map_err(|e| RepositoryError::Unavailable(e.to_string()))?;
        let repo = Self {
            conn: Mutex::new(conn),
        };
        repo.init_schema()?;
        Ok(repo)
    }

    fn init_schema(&self) -> Result<(), RepositoryError> {
        self.with_conn(|conn| {
            conn.execute_batch(SCHEMA)?;
            Ok(())
        })
    }

    /// Runs `f` with the underlying connection, dropping the lock before
    /// returning (so no guard is held across an `async` boundary).
    fn with_conn<T>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, RepositoryError>,
    ) -> Result<T, RepositoryError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| RepositoryError::Failed("lock poisoned".into()))?;
        f(&conn)
    }

    fn put_json(&self, table: &str, id: &str, data: &str) -> Result<(), RepositoryError> {
        self.with_conn(|conn| {
            conn.execute(
                &format!("INSERT OR REPLACE INTO {table} (id, data) VALUES (?1, ?2)"),
                rusqlite::params![id, data],
            )?;
            Ok(())
        })
    }

    fn get_json(&self, table: &str, id: &str) -> Result<Option<String>, RepositoryError> {
        self.with_conn(|conn| {
            let mut stmt =
                conn.prepare(&format!("SELECT data FROM {table} WHERE id = ?1 LIMIT 1"))?;
            let mut rows = stmt.query([id])?;
            let Some(row) = rows.next()? else {
                return Ok(None);
            };
            let data: String = row.get(0)?;
            Ok(Some(data))
        })
    }

    fn get_latest_json(&self, table: &str) -> Result<Option<String>, RepositoryError> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT data FROM {table} ORDER BY rowid DESC LIMIT 1"
            ))?;
            let mut rows = stmt.query([])?;
            let Some(row) = rows.next()? else {
                return Ok(None);
            };
            Ok(Some(row.get(0)?))
        })
    }

    fn list_json(&self, table: &str) -> Result<Vec<String>, RepositoryError> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(&format!("SELECT data FROM {table}"))?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
    }

    fn delete_all(&self, table: &str) -> Result<(), RepositoryError> {
        self.with_conn(|conn| {
            conn.execute(&format!("DELETE FROM {table}"), [])?;
            Ok(())
        })
    }

    fn put_command(&self, command: &CloudCommand) -> Result<(), RepositoryError> {
        let data =
            serde_json::to_string(command).map_err(|e| RepositoryError::Failed(e.to_string()))?;
        let status = serde_json::to_value(command.status)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_else(|| "unknown".to_string());
        self.with_conn(|conn| {
            conn.execute(
                "INSERT OR REPLACE INTO cloud_commands (id, status, data) VALUES (?1, ?2, ?3)",
                rusqlite::params![command.command_id.to_string(), status, data],
            )?;
            Ok(())
        })
    }

    fn list_unfinished_commands(&self) -> Result<Vec<String>, RepositoryError> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT data FROM cloud_commands WHERE status IN ({UNFINISHED_STATUSES})"
            ))?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
    }

    fn decode<T: serde::de::DeserializeOwned>(data: &str) -> Result<T, RepositoryError> {
        serde_json::from_str(data).map_err(|e| RepositoryError::Corrupt(e.to_string()))
    }

    fn encode<T: serde::Serialize>(value: &T) -> Result<String, RepositoryError> {
        serde_json::to_string(value).map_err(|e| RepositoryError::Failed(e.to_string()))
    }
}

#[async_trait]
impl DomainRepository for SqliteRepository {
    async fn save_environment(&self, id: &EnvironmentId) -> Result<(), RepositoryError> {
        let data = serde_json::to_string(id).map_err(|e| RepositoryError::Failed(e.to_string()))?;
        self.put_json("environment", "env", &data)
    }

    async fn get_environment(&self) -> Result<Option<EnvironmentId>, RepositoryError> {
        match self.get_json("environment", "env")? {
            None => Ok(None),
            Some(data) => serde_json::from_str(&data)
                .map(Some)
                .map_err(|e| RepositoryError::Corrupt(e.to_string())),
        }
    }

    async fn put_observation(&self, observation: &Observation) -> Result<(), RepositoryError> {
        let data = serde_json::to_string(observation)
            .map_err(|e| RepositoryError::Failed(e.to_string()))?;
        self.put_json("observations", &observation.id().to_string(), &data)
    }

    async fn put_audit_event(&self, event: &DomainEvent) -> Result<(), RepositoryError> {
        let data =
            serde_json::to_string(event).map_err(|e| RepositoryError::Failed(e.to_string()))?;
        self.put_json("audit_events", &event.id().to_string(), &data)
    }

    async fn list_audit_events(&self) -> Result<Vec<DomainEvent>, RepositoryError> {
        self.list_json("audit_events")?
            .iter()
            .map(|data| Self::decode(data))
            .collect()
    }

    async fn save_health(&self, health: &HealthStatus) -> Result<(), RepositoryError> {
        let data =
            serde_json::to_string(health).map_err(|e| RepositoryError::Failed(e.to_string()))?;
        self.put_json("health", "1", &data)
    }

    async fn get_health(&self) -> Result<Option<HealthStatus>, RepositoryError> {
        match self.get_json("health", "1")? {
            None => Ok(None),
            Some(data) => serde_json::from_str(&data)
                .map(Some)
                .map_err(|e| RepositoryError::Corrupt(e.to_string())),
        }
    }

    async fn save_cloud_enrollment(
        &self,
        enrollment: &CloudEnrollment,
    ) -> Result<(), RepositoryError> {
        self.put_json("cloud_enrollment", SINGLETON, &Self::encode(enrollment)?)
    }

    async fn get_cloud_enrollment(&self) -> Result<Option<CloudEnrollment>, RepositoryError> {
        self.get_json("cloud_enrollment", SINGLETON)?
            .map(|data| Self::decode(&data))
            .transpose()
    }

    async fn clear_cloud_state(&self) -> Result<(), RepositoryError> {
        for table in CLOUD_TABLES {
            self.delete_all(table)?;
        }
        Ok(())
    }

    async fn save_cloud_connection(
        &self,
        connection: &CloudConnection,
    ) -> Result<(), RepositoryError> {
        self.put_json("cloud_connection", SINGLETON, &Self::encode(connection)?)
    }

    async fn get_cloud_connection(&self) -> Result<Option<CloudConnection>, RepositoryError> {
        self.get_json("cloud_connection", SINGLETON)?
            .map(|data| Self::decode(&data))
            .transpose()
    }

    async fn put_cloud_command(&self, command: &CloudCommand) -> Result<(), RepositoryError> {
        self.put_command(command)
    }

    async fn get_cloud_command(
        &self,
        command_id: Uuid,
    ) -> Result<Option<CloudCommand>, RepositoryError> {
        self.get_json("cloud_commands", &command_id.to_string())?
            .map(|data| Self::decode(&data))
            .transpose()
    }

    async fn list_unfinished_cloud_commands(&self) -> Result<Vec<CloudCommand>, RepositoryError> {
        self.list_unfinished_commands()?
            .iter()
            .map(|data| Self::decode(data))
            .collect()
    }

    async fn retain_cloud_commands(&self, keep: usize) -> Result<(), RepositoryError> {
        self.with_conn(|conn| {
            conn.execute(
                "DELETE FROM cloud_commands WHERE id NOT IN \
                 (SELECT id FROM cloud_commands ORDER BY rowid DESC LIMIT ?1)",
                rusqlite::params![keep as i64],
            )?;
            Ok(())
        })
    }

    async fn put_execution_decision(
        &self,
        decision: &ExecutionDecision,
    ) -> Result<(), RepositoryError> {
        self.put_json(
            "cloud_decisions",
            &decision.decision_id.to_string(),
            &Self::encode(decision)?,
        )
    }

    async fn put_execution_approval(
        &self,
        approval: &ExecutionApproval,
    ) -> Result<(), RepositoryError> {
        self.put_json(
            "cloud_approvals",
            &approval.command_id.to_string(),
            &Self::encode(approval)?,
        )
    }

    async fn get_execution_approval(
        &self,
        command_id: Uuid,
    ) -> Result<Option<ExecutionApproval>, RepositoryError> {
        self.get_json("cloud_approvals", &command_id.to_string())?
            .map(|data| Self::decode(&data))
            .transpose()
    }

    async fn save_capability_publication(
        &self,
        publication: &CapabilityPublication,
    ) -> Result<(), RepositoryError> {
        self.put_json(
            "cloud_publications",
            &publication.publication_id.to_string(),
            &Self::encode(publication)?,
        )
    }

    async fn get_capability_publication(
        &self,
    ) -> Result<Option<CapabilityPublication>, RepositoryError> {
        self.get_latest_json("cloud_publications")?
            .map(|data| Self::decode(&data))
            .transpose()
    }

    async fn save_managed_configuration(
        &self,
        configuration: &ManagedConfiguration,
    ) -> Result<(), RepositoryError> {
        self.put_json(
            "cloud_managed_configurations",
            &configuration.configuration_id.to_string(),
            &Self::encode(configuration)?,
        )
    }

    async fn get_managed_configuration(
        &self,
        configuration_id: Uuid,
    ) -> Result<Option<ManagedConfiguration>, RepositoryError> {
        self.get_json(
            "cloud_managed_configurations",
            &configuration_id.to_string(),
        )?
        .map(|data| Self::decode(&data))
        .transpose()
    }

    async fn save_applied_configuration(
        &self,
        state: &AppliedConfigurationState,
    ) -> Result<(), RepositoryError> {
        self.put_json(
            "cloud_applied_configurations",
            &state.configuration_id.to_string(),
            &Self::encode(state)?,
        )
    }

    async fn list_applied_configurations(
        &self,
    ) -> Result<Vec<AppliedConfigurationState>, RepositoryError> {
        self.list_json("cloud_applied_configurations")?
            .iter()
            .map(|data| Self::decode(data))
            .collect()
    }

    async fn save_report_buffer(&self, buffer: &ReportBuffer) -> Result<(), RepositoryError> {
        self.put_json("cloud_report_buffer", SINGLETON, &Self::encode(buffer)?)
    }

    async fn get_report_buffer(&self) -> Result<Option<ReportBuffer>, RepositoryError> {
        self.get_json("cloud_report_buffer", SINGLETON)?
            .map(|data| Self::decode(&data))
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use argus_domain::{
        AppliedConfigurationState, ApplyStatus, ApprovalState, BlastRadius, CapabilityDescriptor,
        CapabilityId, CapabilityPublication, CloudCommand, CloudConnection, CloudEnrollment,
        DecisionOutcome, DomainEvent, EnvironmentId, EventType, ExecutionApproval,
        ExecutionDecision, HealthStatus, ManagedConfiguration, RefusalReason, ReportBuffer,
        RiskClass, Severity,
    };
    use chrono::Utc;
    use semver::Version;

    use super::*;
    use crate::DomainRepository;

    #[tokio::test]
    async fn environment_round_trip() {
        let repo = SqliteRepository::open_in_memory().unwrap();
        assert_eq!(repo.get_environment().await.unwrap(), None);

        let id = EnvironmentId::new();
        repo.save_environment(&id).await.unwrap();
        assert_eq!(repo.get_environment().await.unwrap(), Some(id));
    }

    #[tokio::test]
    async fn health_round_trip() {
        let repo = SqliteRepository::open_in_memory().unwrap();
        assert_eq!(repo.get_health().await.unwrap(), None);

        let health = HealthStatus::degraded("test", Utc::now());
        repo.save_health(&health).await.unwrap();
        assert_eq!(repo.get_health().await.unwrap(), Some(health));
    }

    #[tokio::test]
    async fn observations_and_events_are_appended() {
        let repo = SqliteRepository::open_in_memory().unwrap();

        let subject = argus_domain::ResourceId::new("host", "abc").unwrap();
        let observation = argus_domain::Observation::new(
            uuid::Uuid::new_v4(),
            "argusd",
            subject,
            "cpu.usage",
            argus_domain::ObservedValue::Number(0.5),
            0.9,
            argus_domain::Provenance::new("procfs", "read", Utc::now()),
            Utc::now(),
        )
        .unwrap();
        repo.put_observation(&observation).await.unwrap();

        let event = DomainEvent::new(
            uuid::Uuid::new_v4(),
            EventType::new("argus.started").unwrap(),
            Utc::now(),
            "argusd",
            "argusd",
            Severity::Info,
            None,
            None,
            serde_json::json!({}),
        );
        repo.put_audit_event(&event).await.unwrap();
    }

    #[tokio::test]
    async fn health_is_a_singleton() {
        let repo = SqliteRepository::open_in_memory().unwrap();
        let first = HealthStatus::ready(Utc::now());
        let second = HealthStatus::degraded("later", Utc::now());

        repo.save_health(&first).await.unwrap();
        repo.save_health(&second).await.unwrap();

        // Only the latest health is retained.
        assert_eq!(repo.get_health().await.unwrap(), Some(second));
    }

    fn enrollment() -> CloudEnrollment {
        CloudEnrollment::new(
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            "web-01",
            Utc::now(),
        )
    }

    fn command(status: CloudCommandStatusExpected) -> CloudCommand {
        let mut command = CloudCommand::received(
            uuid::Uuid::new_v4(),
            EnvironmentId::new(),
            CapabilityId::new("host.service.restart").unwrap(),
            serde_json::json!({"unit": "nginx"}),
            None,
            Utc::now(),
        );
        if status == CloudCommandStatusExpected::Terminal {
            command.refuse(RefusalReason::PolicyDenied, Utc::now());
        }
        command
    }

    #[derive(PartialEq)]
    enum CloudCommandStatusExpected {
        InFlight,
        Terminal,
    }

    #[tokio::test]
    async fn cloud_enrollment_round_trips() {
        let repo = SqliteRepository::open_in_memory().unwrap();
        assert_eq!(repo.get_cloud_enrollment().await.unwrap(), None);

        let saved = enrollment();
        repo.save_cloud_enrollment(&saved).await.unwrap();
        assert_eq!(repo.get_cloud_enrollment().await.unwrap(), Some(saved));
    }

    #[tokio::test]
    async fn cloud_connection_round_trips_with_the_last_exchange() {
        let repo = SqliteRepository::open_in_memory().unwrap();
        let now = Utc::now();
        let mut connection = CloudConnection::connected("1.0.0");
        connection.record_success(now);

        repo.save_cloud_connection(&connection).await.unwrap();
        let loaded = repo.get_cloud_connection().await.unwrap().unwrap();
        assert_eq!(loaded.last_exchange_at, Some(now));
        assert_eq!(loaded.negotiated_protocol_version, "1.0.0");
        assert_eq!(loaded.backoff_attempt, 0);
    }

    #[tokio::test]
    async fn cloud_command_round_trips() {
        let repo = SqliteRepository::open_in_memory().unwrap();
        let saved = command(CloudCommandStatusExpected::Terminal);
        repo.put_cloud_command(&saved).await.unwrap();
        assert_eq!(
            repo.get_cloud_command(saved.command_id).await.unwrap(),
            Some(saved)
        );
    }

    #[tokio::test]
    async fn unfinished_commands_exclude_terminal_statuses() {
        let repo = SqliteRepository::open_in_memory().unwrap();
        let in_flight = command(CloudCommandStatusExpected::InFlight);
        let terminal = command(CloudCommandStatusExpected::Terminal);
        repo.put_cloud_command(&in_flight).await.unwrap();
        repo.put_cloud_command(&terminal).await.unwrap();

        let unfinished = repo.list_unfinished_cloud_commands().await.unwrap();
        assert_eq!(
            unfinished.len(),
            1,
            "only the interrupted command is returned"
        );
        assert_eq!(unfinished[0].command_id, in_flight.command_id);
    }

    #[tokio::test]
    async fn retaining_commands_keeps_the_newest() {
        let repo = SqliteRepository::open_in_memory().unwrap();
        let mut ids = Vec::new();
        for _ in 0..5 {
            let c = command(CloudCommandStatusExpected::Terminal);
            ids.push(c.command_id);
            repo.put_cloud_command(&c).await.unwrap();
        }

        repo.retain_cloud_commands(2).await.unwrap();
        let mut remaining = Vec::new();
        for id in &ids {
            if repo.get_cloud_command(*id).await.unwrap().is_some() {
                remaining.push(*id);
            }
        }
        assert_eq!(remaining.len(), 2);
        assert_eq!(remaining, vec![ids[3], ids[4]], "the two newest survive");
    }

    #[tokio::test]
    async fn decision_and_approval_round_trip() {
        let repo = SqliteRepository::open_in_memory().unwrap();
        let command_id = uuid::Uuid::new_v4();

        let decision = ExecutionDecision {
            decision_id: uuid::Uuid::new_v4(),
            command_id,
            outcome: DecisionOutcome::RequireApproval,
            risk_class: RiskClass::HighRisk,
            blast_radius: BlastRadius::Host,
            policy_id: "bootstrap.read-only".into(),
            reason: "high risk requires approval".into(),
            decided_at: Utc::now(),
        };
        repo.put_execution_decision(&decision).await.unwrap();

        let approval = ExecutionApproval::grant(
            command_id,
            "uid=1000",
            Utc::now(),
            Utc::now() + chrono::Duration::minutes(10),
        );
        repo.put_execution_approval(&approval).await.unwrap();

        let loaded = repo
            .get_execution_approval(command_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded, approval);
        assert_eq!(loaded.state, ApprovalState::Granted);
        assert!(loaded.authorizes(command_id, Utc::now()));
    }

    #[tokio::test]
    async fn approval_lookup_is_keyed_by_command() {
        let repo = SqliteRepository::open_in_memory().unwrap();
        let never_approved = uuid::Uuid::new_v4();
        assert_eq!(
            repo.get_execution_approval(never_approved).await.unwrap(),
            None
        );
    }

    fn descriptor() -> CapabilityDescriptor {
        CapabilityDescriptor::new(
            CapabilityId::new("host.status.read").unwrap(),
            "argusd",
            "status.get",
            RiskClass::Read,
            Version::new(0, 1, 0),
            serde_json::json!({}),
            serde_json::json!({}),
            argus_domain::Reversibility::None,
        )
    }

    #[tokio::test]
    async fn the_latest_publication_is_returned() {
        let repo = SqliteRepository::open_in_memory().unwrap();
        let first = CapabilityPublication::new("0.1.0", vec![descriptor()], Utc::now());
        repo.save_capability_publication(&first).await.unwrap();
        let second = CapabilityPublication::new("0.2.0", vec![descriptor()], Utc::now());
        repo.save_capability_publication(&second).await.unwrap();

        let current = repo.get_capability_publication().await.unwrap().unwrap();
        assert_eq!(current.version, "0.2.0");
    }

    #[tokio::test]
    async fn publication_detects_a_noop_republish() {
        let capabilities = vec![descriptor()];
        let publication = CapabilityPublication::new("0.1.0", capabilities.clone(), Utc::now());
        assert!(publication.is_noop_for(&capabilities));
        assert!(!publication.is_noop_for(&[]));
    }

    #[tokio::test]
    async fn managed_and_applied_configuration_round_trip() {
        let repo = SqliteRepository::open_in_memory().unwrap();
        let configuration_id = uuid::Uuid::new_v4();

        let managed = ManagedConfiguration {
            configuration_id,
            kind: "agent_team".into(),
            current_version_id: None,
            current_version_number: 0,
            content_hash: None,
            applied_at: None,
            apply_status: ApplyStatus::None,
            apply_reason: None,
        };
        repo.save_managed_configuration(&managed).await.unwrap();
        assert_eq!(
            repo.get_managed_configuration(configuration_id)
                .await
                .unwrap(),
            Some(managed)
        );

        let applied = AppliedConfigurationState {
            configuration_id,
            applied_version_id: None,
            applied_version_number: 0,
            content_hash: None,
            updated_at: Utc::now(),
        };
        repo.save_applied_configuration(&applied).await.unwrap();
        let listed = repo.list_applied_configurations().await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].applied_version_id, None, "absence is explicit");
    }

    #[tokio::test]
    async fn report_buffer_round_trips() {
        let repo = SqliteRepository::open_in_memory().unwrap();
        let mut buffer = ReportBuffer::with_capacity(4);
        buffer.record_enqueue();
        buffer.record_enqueue();
        repo.save_report_buffer(&buffer).await.unwrap();

        let loaded = repo.get_report_buffer().await.unwrap().unwrap();
        assert_eq!(loaded.capacity, 4);
        assert_eq!(loaded.count, 2);
    }

    #[tokio::test]
    async fn clear_cloud_state_removes_every_cloud_record() {
        let repo = SqliteRepository::open_in_memory().unwrap();
        let now = Utc::now();

        repo.save_cloud_enrollment(&enrollment()).await.unwrap();
        repo.save_cloud_connection(&CloudConnection::connected("1.0.0"))
            .await
            .unwrap();
        repo.put_cloud_command(&command(CloudCommandStatusExpected::InFlight))
            .await
            .unwrap();
        repo.save_capability_publication(&CapabilityPublication::new("0.1.0", vec![], now))
            .await
            .unwrap();
        repo.save_report_buffer(&ReportBuffer::with_capacity(4))
            .await
            .unwrap();

        repo.clear_cloud_state().await.unwrap();

        assert_eq!(repo.get_cloud_enrollment().await.unwrap(), None);
        assert_eq!(repo.get_cloud_connection().await.unwrap(), None);
        assert!(
            repo.list_unfinished_cloud_commands()
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(repo.get_capability_publication().await.unwrap(), None);
        assert_eq!(repo.get_report_buffer().await.unwrap(), None);
        assert!(repo.list_applied_configurations().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn clearing_cloud_state_leaves_local_operational_state_intact() {
        let repo = SqliteRepository::open_in_memory().unwrap();
        let environment = EnvironmentId::new();
        let health = HealthStatus::ready(Utc::now());
        repo.save_environment(&environment).await.unwrap();
        repo.save_health(&health).await.unwrap();
        repo.save_cloud_enrollment(&enrollment()).await.unwrap();

        repo.clear_cloud_state().await.unwrap();

        assert_eq!(repo.get_environment().await.unwrap(), Some(environment));
        assert_eq!(repo.get_health().await.unwrap(), Some(health));
    }

    #[tokio::test]
    async fn a_backend_without_cloud_state_fails_explicitly_rather_than_silently() {
        let repo = crate::InMemoryRepository::new();
        let err = repo.get_cloud_enrollment().await.unwrap_err();
        let text = err.to_string();
        assert!(text.contains("cloud state"), "{text}");
        assert!(text.contains("get_cloud_enrollment"), "{text}");
    }
}
