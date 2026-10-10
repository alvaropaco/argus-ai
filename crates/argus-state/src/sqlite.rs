//! SQLite-backed repository (interim default backend).
//!
//! See ADR-011: LanceDB is the intended operational store; SQLite is used as
//! a lean interim backend while the LanceDB Rust SDK is validated. Both are
//! behind the [`DomainRepository`](crate::DomainRepository) trait.

use std::path::Path;
use std::sync::Mutex;

use argus_domain::{
    ActionEventFilter, ActionEventRecord, AppliedConfigurationState, AutonomyState,
    BrainTraceRecord, CapabilityPublication, CloudCommand, CloudConnection, CloudEnrollment,
    DEFAULT_ACTION_EVENT_LIMIT, DomainEvent, EnvironmentId, Execution, ExecutionApproval,
    ExecutionDecision, HealthStatus, Hypothesis, ManagedConfiguration, Observation, Plan,
    ReportBuffer, TokenUsageRecord,
};
use argus_memory::{Episode, Fact, ProcedureRecord};
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
CREATE TABLE IF NOT EXISTS reasoning_hypotheses (
    id   TEXT PRIMARY KEY,
    data TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS reasoning_plans (
    id   TEXT PRIMARY KEY,
    data TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS reasoning_executions (
    id   TEXT PRIMARY KEY,
    data TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS memory_episodes (
    id   TEXT PRIMARY KEY,
    data TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS memory_facts (
    id   TEXT PRIMARY KEY,
    data TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS memory_procedures (
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
CREATE TABLE IF NOT EXISTS ledger_actions (
    id          TEXT PRIMARY KEY,
    kind        TEXT NOT NULL,
    status      TEXT NOT NULL,
    occurred_at TEXT NOT NULL,
    data        TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS ledger_actions_time ON ledger_actions (occurred_at);
CREATE INDEX IF NOT EXISTS ledger_actions_kind ON ledger_actions (kind);
CREATE TABLE IF NOT EXISTS ledger_traces (
    id          TEXT PRIMARY KEY,
    cycle_id    TEXT,
    occurred_at TEXT NOT NULL,
    data        TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS ledger_traces_cycle ON ledger_traces (cycle_id);
CREATE TABLE IF NOT EXISTS ledger_usage (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    occurred_at TEXT NOT NULL,
    data        TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS ledger_usage_time ON ledger_usage (occurred_at);
CREATE TABLE IF NOT EXISTS autonomy_state (
    id   TEXT PRIMARY KEY,
    data TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS runbooks (
    -- Keyed by the runbook's stable name: a re-delivery supersedes (spec 009 FR-001).
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

    /// Like [`Self::list_json`], but returns `(rowid-key, data)` pairs ordered by
    /// insertion, so reasoning history reads back in the order it was recorded.
    /// Like [`Self::list_json_ordered`] but for non-Uuid keys (e.g. the
    /// memory facts' composite "subject#attribute" key).
    fn list_json_ordered_raw(&self, table: &str) -> Result<Vec<(String, String)>, RepositoryError> {
        self.with_conn(|conn| {
            let mut stmt =
                conn.prepare(&format!("SELECT id, data FROM {table} ORDER BY rowid ASC"))?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
    }

    fn list_json_ordered(&self, table: &str) -> Result<Vec<(Uuid, String)>, RepositoryError> {
        self.with_conn(|conn| {
            let mut stmt =
                conn.prepare(&format!("SELECT id, data FROM {table} ORDER BY rowid ASC"))?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (id, data) = row?;
                let id = Uuid::parse_str(&id)
                    .map_err(|e| RepositoryError::Corrupt(format!("invalid id: {e}")))?;
                out.push((id, data));
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

    /// RFC3339 UTC timestamps order lexicographically, so the text column
    /// sorts correctly without a schema change.
    fn occurred_at(at: &chrono::DateTime<chrono::Utc>) -> String {
        at.to_rfc3339()
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

    async fn list_observations(&self) -> Result<Vec<Observation>, RepositoryError> {
        self.list_json("observations")?
            .iter()
            .map(|data| Self::decode(data))
            .collect()
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

    async fn put_hypothesis(
        &self,
        id: Uuid,
        hypothesis: &Hypothesis,
    ) -> Result<(), RepositoryError> {
        let data = Self::encode(hypothesis)?;
        self.put_json("reasoning_hypotheses", &id.to_string(), &data)
    }

    async fn list_hypotheses(&self) -> Result<Vec<(Uuid, Hypothesis)>, RepositoryError> {
        self.list_json_ordered("reasoning_hypotheses")?
            .into_iter()
            .map(|(id, data)| Self::decode(&data).map(|h| (id, h)))
            .collect()
    }

    async fn put_plan(&self, id: Uuid, plan: &Plan) -> Result<(), RepositoryError> {
        let data = Self::encode(plan)?;
        self.put_json("reasoning_plans", &id.to_string(), &data)
    }

    async fn list_plans(&self) -> Result<Vec<(Uuid, Plan)>, RepositoryError> {
        self.list_json_ordered("reasoning_plans")?
            .into_iter()
            .map(|(id, data)| Self::decode(&data).map(|p| (id, p)))
            .collect()
    }

    async fn put_episode(&self, id: Uuid, episode: &Episode) -> Result<(), RepositoryError> {
        let data = Self::encode(episode)?;
        self.put_json("memory_episodes", &id.to_string(), &data)
    }

    async fn list_episodes(&self) -> Result<Vec<(Uuid, Episode)>, RepositoryError> {
        self.list_json_ordered("memory_episodes")?
            .into_iter()
            .map(|(id, data)| Self::decode(&data).map(|e| (id, e)))
            .collect()
    }

    async fn put_fact(&self, fact: &Fact) -> Result<(), RepositoryError> {
        // The composite key mirrors SemanticMemory's supersede semantics:
        // re-recording a (subject, attribute) replaces the prior value.
        let key = format!("{}#{}", fact.subject.as_str(), fact.attribute);
        let data = Self::encode(fact)?;
        self.put_json("memory_facts", &key, &data)
    }

    async fn list_facts(&self) -> Result<Vec<Fact>, RepositoryError> {
        // Facts key on "subject#attribute", not a Uuid, so they list through
        // the raw-key variant.
        Ok(self
            .list_json_ordered_raw("memory_facts")?
            .into_iter()
            .map(|(_, data)| Self::decode(&data))
            .collect::<Result<Vec<_>, _>>()?)
    }

    async fn put_procedure(
        &self,
        id: Uuid,
        procedure: &ProcedureRecord,
    ) -> Result<(), RepositoryError> {
        let data = Self::encode(procedure)?;
        self.put_json("memory_procedures", &id.to_string(), &data)
    }

    async fn list_procedures(&self) -> Result<Vec<(Uuid, ProcedureRecord)>, RepositoryError> {
        self.list_json_ordered("memory_procedures")?
            .into_iter()
            .map(|(id, data)| Self::decode(&data).map(|p| (id, p)))
            .collect()
    }

    async fn put_execution(&self, id: Uuid, execution: &Execution) -> Result<(), RepositoryError> {
        let data = Self::encode(execution)?;
        self.put_json("reasoning_executions", &id.to_string(), &data)
    }

    async fn list_executions(&self) -> Result<Vec<(Uuid, Execution)>, RepositoryError> {
        self.list_json_ordered("reasoning_executions")?
            .into_iter()
            .map(|(id, data)| Self::decode(&data).map(|e| (id, e)))
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

    async fn list_execution_decisions(&self) -> Result<Vec<ExecutionDecision>, RepositoryError> {
        self.list_json("cloud_decisions")?
            .iter()
            .map(|data| Self::decode(data))
            .collect()
    }

    async fn put_execution_approval(
        &self,
        approval: &ExecutionApproval,
    ) -> Result<(), RepositoryError> {
        self.put_json(
            "cloud_approvals",
            &approval.token.to_string(),
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

    async fn put_action_event(&self, event: &ActionEventRecord) -> Result<(), RepositoryError> {
        let data = Self::encode(event)?;
        self.with_conn(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO ledger_actions (id, kind, status, occurred_at, data) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    event.event_id.to_string(),
                    event.kind,
                    event.outcome,
                    Self::occurred_at(&event.occurred_at),
                    data
                ],
            )?;
            Ok(())
        })
    }

    async fn list_action_events(
        &self,
        filter: &ActionEventFilter,
    ) -> Result<Vec<ActionEventRecord>, RepositoryError> {
        let limit = if filter.limit == 0 {
            DEFAULT_ACTION_EVENT_LIMIT
        } else {
            filter.limit
        };
        let mut sql = String::from("SELECT data FROM ledger_actions");
        let mut clauses: Vec<String> = Vec::new();
        if let Some(since) = filter.since {
            clauses.push(format!("occurred_at >= '{}'", Self::occurred_at(&since)));
        }
        if let Some(kind) = &filter.kind {
            clauses.push(format!("kind = '{}'", kind.replace('\'', "''")));
        }
        if let Some(status) = &filter.status {
            clauses.push(format!("status = '{}'", status.replace('\'', "''")));
        }
        if !clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&clauses.join(" AND "));
        }
        sql.push_str(" ORDER BY occurred_at DESC, rowid DESC LIMIT ");
        sql.push_str(&limit.to_string());
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })?
        .iter()
        .map(|data| Self::decode(data))
        .collect()
    }

    async fn put_brain_trace(&self, trace: &BrainTraceRecord) -> Result<(), RepositoryError> {
        let data = Self::encode(trace)?;
        self.with_conn(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO ledger_traces (id, cycle_id, occurred_at, data) \
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    trace.trace_id.to_string(),
                    trace.cycle_id.map(|id| id.to_string()),
                    Self::occurred_at(&trace.occurred_at),
                    data
                ],
            )?;
            Ok(())
        })
    }

    async fn list_brain_traces(
        &self,
        cycle_id: Option<Uuid>,
        limit: usize,
    ) -> Result<Vec<BrainTraceRecord>, RepositoryError> {
        let limit = if limit == 0 {
            DEFAULT_ACTION_EVENT_LIMIT
        } else {
            limit
        };
        let sql = match cycle_id {
            Some(id) => format!(
                "SELECT data FROM ledger_traces WHERE cycle_id = '{}' \
                 ORDER BY occurred_at DESC, rowid DESC LIMIT {limit}",
                id
            ),
            None => format!(
                "SELECT data FROM ledger_traces ORDER BY occurred_at DESC, rowid DESC LIMIT {limit}"
            ),
        };
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })?
        .iter()
        .map(|data| Self::decode(data))
        .collect()
    }

    async fn put_token_usage(&self, usage: &TokenUsageRecord) -> Result<(), RepositoryError> {
        let data = Self::encode(usage)?;
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO ledger_usage (occurred_at, data) VALUES (?1, ?2)",
                rusqlite::params![Self::occurred_at(&usage.occurred_at), data],
            )?;
            Ok(())
        })
    }

    async fn list_token_usage(
        &self,
        limit: usize,
    ) -> Result<Vec<TokenUsageRecord>, RepositoryError> {
        let limit = if limit == 0 {
            DEFAULT_ACTION_EVENT_LIMIT
        } else {
            limit
        };
        let sql = format!(
            "SELECT data FROM ledger_usage ORDER BY occurred_at DESC, id DESC LIMIT {limit}"
        );
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })?
        .iter()
        .map(|data| Self::decode(data))
        .collect()
    }

    async fn retain_ledger(&self, days: i64) -> Result<(), RepositoryError> {
        let cutoff = Self::occurred_at(&(chrono::Utc::now() - chrono::Duration::days(days.max(0))));
        self.with_conn(|conn| {
            for table in ["ledger_actions", "ledger_traces", "ledger_usage"] {
                conn.execute(
                    &format!("DELETE FROM {table} WHERE occurred_at < ?1"),
                    rusqlite::params![cutoff],
                )?;
            }
            Ok(())
        })
    }

    async fn put_autonomy_state(&self, state: &AutonomyState) -> Result<(), RepositoryError> {
        // One row per environment: the record carries its environment key and
        // a re-put supersedes (spec 008 FR-001).
        self.put_json("autonomy_state", "autonomy", &Self::encode(state)?)
    }

    async fn get_autonomy_state(&self) -> Result<Option<AutonomyState>, RepositoryError> {
        self.get_json("autonomy_state", "autonomy")?
            .map(|data| Self::decode(&data))
            .transpose()
    }

    async fn put_runbook(
        &self,
        runbook: &argus_runbooks::DeliveredRunbook,
    ) -> Result<(), RepositoryError> {
        // Keyed by the runbook's stable name; a re-delivery replaces the row
        // (spec 009 FR-001 supersede, FR-002 persistence).
        self.put_json("runbooks", runbook.name(), &Self::encode(runbook)?)
    }

    async fn list_runbooks(
        &self,
    ) -> Result<Vec<argus_runbooks::DeliveredRunbook>, RepositoryError> {
        let mut out: Vec<argus_runbooks::DeliveredRunbook> = self
            .list_json("runbooks")?
            .iter()
            .map(|data| Self::decode(data))
            .collect::<Result<Vec<_>, _>>()?;
        out.sort_by(|a, b| a.name().cmp(b.name()));
        Ok(out)
    }
}

/// The wrapper row for `ledger_traces`, whose extracted columns live beside
/// the serialized record (the actions table keeps its columns flat instead,
/// because its filters are flat).
#[cfg(test)]
mod tests {
    use argus_domain::{
        ActionEventFilter, ActionEventRecord, AppliedConfigurationState, ApplyStatus,
        ApprovalState, BlastRadius, BrainTraceRecord, CapabilityDescriptor, CapabilityId,
        CapabilityPublication, CloudCommand, CloudConnection, CloudEnrollment, DecisionOutcome,
        DomainEvent, EnvironmentId, EventType, ExecutionApproval, ExecutionDecision, HealthStatus,
        ManagedConfiguration, RefusalReason, ReportBuffer, RiskClass, Severity, TokenUsageRecord,
    };
    use chrono::Utc;
    use semver::Version;

    use super::*;
    use crate::DomainRepository;

    fn action_event(outcome: &str, kind: &str, at: chrono::DateTime<Utc>) -> ActionEventRecord {
        ActionEventRecord {
            event_id: uuid::Uuid::new_v4(),
            correlation_id: uuid::Uuid::new_v4(),
            causation_id: None,
            cycle_id: None,
            plan_id: Some(uuid::Uuid::new_v4()),
            kind: kind.into(),
            target: Some("nginx.service".into()),
            args: serde_json::json!({ "unit": "nginx.service", "TOKEN": "raw-stays-local" }),
            verdict: "allow".into(),
            policy_id: Some("bootstrap.remediation".into()),
            outcome: outcome.into(),
            duration_ms: Some(10),
            validation: None,
            occurred_at: at,
        }
    }

    #[tokio::test]
    async fn the_action_ledger_appends_and_lists_newest_first() {
        let repo = SqliteRepository::open_in_memory().unwrap();
        assert!(
            repo.list_action_events(&ActionEventFilter::default())
                .await
                .unwrap()
                .is_empty()
        );

        let old = action_event(
            "ok",
            "host.service.restart",
            Utc::now() - chrono::Duration::hours(2),
        );
        let new = action_event("denied", "host.process.signal", Utc::now());
        repo.put_action_event(&old).await.unwrap();
        repo.put_action_event(&new).await.unwrap();

        let listed = repo
            .list_action_events(&ActionEventFilter::default())
            .await
            .unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].event_id, new.event_id, "newest first");
        assert_eq!(
            listed[0].args["TOKEN"], "raw-stays-local",
            "local fidelity keeps raw args"
        );
    }

    #[tokio::test]
    async fn the_action_ledger_never_rewrites_an_event_id() {
        let repo = SqliteRepository::open_in_memory().unwrap();
        let mut event = action_event("ok", "host.service.restart", Utc::now());
        repo.put_action_event(&event).await.unwrap();
        event.outcome = "failed".into();
        repo.put_action_event(&event).await.unwrap();

        let listed = repo
            .list_action_events(&ActionEventFilter::default())
            .await
            .unwrap();
        assert_eq!(listed.len(), 1, "append-only: the re-put is ignored");
        assert_eq!(listed[0].outcome, "ok", "the first write stands");
    }

    #[tokio::test]
    async fn action_ledger_filters_narrow_by_kind_status_and_time() {
        let repo = SqliteRepository::open_in_memory().unwrap();
        let denied = action_event("denied", "host.process.signal", Utc::now());
        let ok = action_event("ok", "host.service.restart", Utc::now());
        let ancient = action_event(
            "ok",
            "host.service.restart",
            Utc::now() - chrono::Duration::days(3),
        );
        for event in [&denied, &ok, &ancient] {
            repo.put_action_event(event).await.unwrap();
        }

        let by_status = repo
            .list_action_events(&ActionEventFilter {
                status: Some("denied".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(by_status.len(), 1);
        assert_eq!(by_status[0].event_id, denied.event_id);

        let by_kind = repo
            .list_action_events(&ActionEventFilter {
                kind: Some("host.service.restart".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(by_kind.len(), 2);

        let recent = repo
            .list_action_events(&ActionEventFilter {
                since: Some(Utc::now() - chrono::Duration::hours(1)),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(recent.len(), 2, "the ancient row is out of the window");
    }

    #[tokio::test]
    async fn traces_and_usage_round_trip_and_narrow_by_cycle() {
        let repo = SqliteRepository::open_in_memory().unwrap();
        let cycle = uuid::Uuid::new_v4();
        let trace = BrainTraceRecord {
            trace_id: uuid::Uuid::new_v4(),
            cycle_id: Some(cycle),
            plan_id: Some(uuid::Uuid::new_v4()),
            evidence: vec!["unit: nginx.service failed".into()],
            decision: Some("provider decision passed the gate".into()),
            objective: Some("restore nginx".into()),
            steps: vec!["host.service.restart".into()],
            outcome: Some("Completed".into()),
            occurred_at: Utc::now(),
        };
        repo.put_brain_trace(&trace).await.unwrap();
        let other = BrainTraceRecord {
            trace_id: uuid::Uuid::new_v4(),
            cycle_id: None,
            ..trace.clone()
        };
        repo.put_brain_trace(&other).await.unwrap();

        let for_cycle = repo.list_brain_traces(Some(cycle), 10).await.unwrap();
        assert_eq!(for_cycle.len(), 1);
        assert_eq!(for_cycle[0], trace);
        assert_eq!(repo.list_brain_traces(None, 10).await.unwrap().len(), 2);

        let usage = TokenUsageRecord {
            cycle_id: Some(cycle),
            model: Some("deepseek-chat".into()),
            prompt_tokens: Some(120),
            completion_tokens: None,
            total_tokens: None,
            duration_ms: Some(640),
            occurred_at: Utc::now(),
        };
        repo.put_token_usage(&usage).await.unwrap();
        let listed = repo.list_token_usage(10).await.unwrap();
        assert_eq!(listed, vec![usage]);
        assert!(
            listed[0].completion_tokens.is_none(),
            "a missing provider usage block stays unknown, never zero"
        );
    }

    #[tokio::test]
    async fn retaining_the_ledger_drops_only_old_rows() {
        let repo = SqliteRepository::open_in_memory().unwrap();
        let old = action_event(
            "ok",
            "host.service.restart",
            Utc::now() - chrono::Duration::days(40),
        );
        let new = action_event("ok", "host.service.restart", Utc::now());
        repo.put_action_event(&old).await.unwrap();
        repo.put_action_event(&new).await.unwrap();
        repo.put_token_usage(&TokenUsageRecord {
            cycle_id: None,
            model: None,
            prompt_tokens: None,
            completion_tokens: None,
            total_tokens: None,
            duration_ms: None,
            occurred_at: Utc::now() - chrono::Duration::days(40),
        })
        .await
        .unwrap();

        repo.retain_ledger(30).await.unwrap();

        let listed = repo
            .list_action_events(&ActionEventFilter::default())
            .await
            .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].event_id, new.event_id);
        assert!(repo.list_token_usage(10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn environment_round_trip() {
        let repo = SqliteRepository::open_in_memory().unwrap();
        assert_eq!(repo.get_environment().await.unwrap(), None);

        let id = EnvironmentId::new();
        repo.save_environment(&id).await.unwrap();
        assert_eq!(repo.get_environment().await.unwrap(), Some(id));
    }

    #[tokio::test]
    async fn autonomy_state_round_trips_as_a_single_superseding_row() {
        let repo = SqliteRepository::open_in_memory().unwrap();
        assert_eq!(repo.get_autonomy_state().await.unwrap(), None);

        let mut state = argus_domain::AutonomyState::fresh(EnvironmentId::new(), Utc::now());
        repo.put_autonomy_state(&state).await.unwrap();
        assert_eq!(
            repo.get_autonomy_state().await.unwrap(),
            Some(state.clone())
        );

        // The tick's mutations supersede: counters and budget windows ride
        // the serialized record, so a restart resumes mid-assimilation
        // (spec 008 FR-001, AC-007).
        state.phase = argus_domain::AssimilationPhase::Shadow;
        state.shadow_clean_cycles = 7;
        state.budget.low_risk_hour = Some((12_345, 4));
        repo.put_autonomy_state(&state).await.unwrap();

        let loaded = repo.get_autonomy_state().await.unwrap().unwrap();
        assert_eq!(loaded, state);
        assert_eq!(loaded.shadow_clean_cycles, 7);
        assert_eq!(loaded.budget.low_risk_hour, Some((12_345, 4)));
    }

    #[tokio::test]
    async fn delivered_runbooks_persist_by_name_and_supersede() {
        let repo = SqliteRepository::open_in_memory().unwrap();
        assert!(repo.list_runbooks().await.unwrap().is_empty());

        let delivered = delivered_runbook("learned-procedure", 3);
        repo.put_runbook(&delivered).await.unwrap();
        let listed = repo.list_runbooks().await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0], delivered);

        // A re-delivery of the same name supersedes (spec 009 FR-001); gate
        // progress rides the serialized candidate (FR-002).
        let mut superseding = delivered_runbook("learned-procedure", 4);
        superseding
            .runbook
            .record_gate(argus_runbooks::Gate::Evaluation)
            .unwrap();
        repo.put_runbook(&superseding).await.unwrap();

        // A different name is a separate row.
        repo.put_runbook(&delivered_runbook("other-procedure", 1))
            .await
            .unwrap();

        let listed = repo.list_runbooks().await.unwrap();
        assert_eq!(listed.len(), 2, "one row per name");
        let learned = listed
            .iter()
            .find(|r| r.name() == "learned-procedure")
            .unwrap();
        assert_eq!(learned.version_number, 4);
        assert_eq!(
            learned.runbook.gates(),
            [argus_runbooks::Gate::Evaluation],
            "the superseding row carries the newer gate progress"
        );
        // Deterministic order.
        assert_eq!(listed[0].name(), "learned-procedure");
        assert_eq!(listed[1].name(), "other-procedure");
    }

    fn delivered_runbook(name: &str, version_number: i64) -> argus_runbooks::DeliveredRunbook {
        let runbook = argus_runbooks::Runbook::candidate(
            uuid::Uuid::new_v4(),
            name,
            argus_runbooks::RunbookTrigger::Symptom("restart-loop".into()),
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
        );
        argus_runbooks::DeliveredRunbook {
            runbook,
            configuration_id: uuid::Uuid::new_v4(),
            version_id: uuid::Uuid::new_v4(),
            version_number,
            delivered_at: Utc::now(),
        }
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
    async fn list_observations_returns_recorded_observations() {
        let repo = SqliteRepository::open_in_memory().unwrap();
        assert!(repo.list_observations().await.unwrap().is_empty());

        let subject = argus_domain::ResourceId::new("host", "abc").unwrap();
        let observation = argus_domain::Observation::new(
            uuid::Uuid::new_v4(),
            "argusd",
            subject,
            "service.active",
            argus_domain::ObservedValue::Bool(true),
            1.0,
            argus_domain::Provenance::new("systemd", "is_active", Utc::now()),
            Utc::now(),
        )
        .unwrap();
        repo.put_observation(&observation).await.unwrap();

        assert_eq!(repo.list_observations().await.unwrap(), vec![observation]);
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
            "",
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
        assert!(loaded.authorizes(command_id, "", Utc::now()));
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

    #[tokio::test]
    async fn reasoning_artifacts_round_trip_in_insertion_order() {
        let repo = SqliteRepository::open_in_memory().unwrap();
        assert!(repo.list_plans().await.unwrap().is_empty());
        assert!(repo.list_hypotheses().await.unwrap().is_empty());
        assert!(repo.list_executions().await.unwrap().is_empty());

        let first_id = uuid::Uuid::new_v4();
        repo.put_plan(first_id, &sample_plan("restore nginx", 0.9))
            .await
            .unwrap();
        let second_id = uuid::Uuid::new_v4();
        repo.put_plan(second_id, &sample_plan("restore postgres", 0.7))
            .await
            .unwrap();

        let plans = repo.list_plans().await.unwrap();
        assert_eq!(plans.len(), 2, "both plans are stored");
        assert_eq!(plans[0].0, first_id);
        assert_eq!(plans[0].1.objective, "restore nginx");
        assert_eq!(plans[1].0, second_id);

        let hypothesis = argus_domain::Hypothesis {
            statement: "nginx stopped".into(),
            confidence: 0.9,
            supporting_evidence: vec![],
            status: argus_domain::HypothesisStatus::Confirmed,
        };
        repo.put_hypothesis(first_id, &hypothesis).await.unwrap();
        let hypotheses = repo.list_hypotheses().await.unwrap();
        assert_eq!(hypotheses, vec![(first_id, hypothesis)]);

        let execution = argus_domain::Execution {
            action: plan_steps_action(),
            status: argus_domain::ExecutionStatus::Completed,
            evidence: serde_json::json!({ "unit": "nginx.service" }),
        };
        repo.put_execution(first_id, &execution).await.unwrap();
        let executions = repo.list_executions().await.unwrap();
        assert_eq!(executions, vec![(first_id, execution)]);
    }

    #[tokio::test]
    async fn re_recording_a_correlation_id_replaces_its_reasoning_artifact() {
        let repo = SqliteRepository::open_in_memory().unwrap();
        let id = uuid::Uuid::new_v4();
        repo.put_plan(id, &sample_plan("first", 0.5)).await.unwrap();
        repo.put_plan(id, &sample_plan("second", 0.8))
            .await
            .unwrap();

        let plans = repo.list_plans().await.unwrap();
        assert_eq!(plans.len(), 1, "one row per correlation id");
        assert_eq!(plans[0].1.objective, "second");
    }

    fn sample_plan(objective: &str, confidence: f64) -> Plan {
        Plan {
            objective: objective.into(),
            steps: vec![argus_domain::PlanStep {
                action: plan_steps_action(),
                rollback: None,
            }],
            preconditions: vec![],
            expected_outcomes: vec![],
            blast_radius: BlastRadius::Host,
            confidence,
            status: argus_domain::PlanStatus::Proposed,
        }
    }

    fn plan_steps_action() -> argus_domain::Action {
        argus_domain::Action {
            capability: CapabilityId::new("host.service.restart").unwrap(),
            resource: None,
            arguments: serde_json::json!({ "unit": "nginx.service" }),
        }
    }
}
