//! SQLite-backed repository (interim default backend).
//!
//! See ADR-011: LanceDB is the intended operational store; SQLite is used as
//! a lean interim backend while the LanceDB Rust SDK is validated. Both are
//! behind the [`DomainRepository`](crate::DomainRepository) trait.

use std::path::Path;
use std::sync::Mutex;

use argus_domain::{DomainEvent, EnvironmentId, HealthStatus, Observation};
use async_trait::async_trait;
use rusqlite::Connection;

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
"#;

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
}

#[cfg(test)]
mod tests {
    use argus_domain::{DomainEvent, EnvironmentId, EventType, HealthStatus, Severity};
    use chrono::Utc;

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
}
