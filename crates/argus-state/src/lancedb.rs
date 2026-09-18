//! LanceDB-backed repository adapter.
//!
//! Entities are stored as JSON rows in a single LanceDB table (`argus_records`)
//! with a `(id, kind, data)` schema. This keeps the bootstrap adapter simple
//! while remaining behind the [`DomainRepository`](crate::DomainRepository)
//! trait so LanceDB never leaks into the domain.

use std::sync::Arc;

use argus_domain::{DomainEvent, EnvironmentId, HealthStatus, Observation};
use async_trait::async_trait;
use futures::TryStreamExt;
use lancedb::arrow::arrow_array::cast::AsArray;
use lancedb::arrow::arrow_array::{ArrayRef, RecordBatch, StringArray};
use lancedb::arrow::arrow_schema::{DataType, Field, Schema};

use crate::repository::{DomainRepository, RepositoryError};

const TABLE: &str = "argus_records";
const KIND_ENVIRONMENT: &str = "environment";
const KIND_HEALTH: &str = "health";
const KIND_OBSERVATION: &str = "observation";
const KIND_AUDIT: &str = "audit_event";

/// A [`DomainRepository`] persisted with LanceDB.
pub struct LanceDbRepository {
    db: lancedb::Connection,
}

impl LanceDbRepository {
    /// Connects to (or creates) a LanceDB database at `path`.
    pub async fn connect(path: &str) -> Result<Self, RepositoryError> {
        let db = lancedb::connect(path)
            .execute()
            .await
            .map_err(|e| RepositoryError::Unavailable(e.to_string()))?;
        Ok(Self { db })
    }

    async fn append(&self, id: &str, kind: &str, data: &str) -> Result<(), RepositoryError> {
        let batch = record_batch(id, kind, data);
        if self.table_exists().await? {
            let table = self.open_table().await?;
            table
                .add(batch)
                .execute()
                .await
                .map_err(|e| RepositoryError::Failed(e.to_string()))?;
        } else {
            self.db
                .create_table(TABLE, batch)
                .execute()
                .await
                .map_err(|e| RepositoryError::Failed(e.to_string()))?;
        }
        Ok(())
    }

    async fn table_exists(&self) -> Result<bool, RepositoryError> {
        let names = self
            .db
            .table_names()
            .execute()
            .await
            .map_err(|e| RepositoryError::Failed(e.to_string()))?;
        Ok(names.iter().any(|n| n == TABLE))
    }

    async fn open_table(&self) -> Result<lancedb::Table, RepositoryError> {
        self.db
            .open_table(TABLE)
            .execute()
            .await
            .map_err(|e| RepositoryError::Failed(e.to_string()))
    }

    /// Returns the JSON payloads for all rows of `kind`, in insertion order.
    async fn read_kind(&self, kind: &str) -> Result<Vec<String>, RepositoryError> {
        if !self.table_exists().await? {
            return Ok(Vec::new());
        }
        let table = self.open_table().await?;
        let stream = table
            .query()
            .execute()
            .await
            .map_err(|e| RepositoryError::Failed(e.to_string()))?;
        let batches: Vec<RecordBatch> = stream
            .try_collect()
            .await
            .map_err(|e| RepositoryError::Failed(e.to_string()))?;

        Ok(filter_data_by_kind(&batches, kind))
    }
}

#[async_trait]
impl DomainRepository for LanceDbRepository {
    async fn save_environment(&self, id: &EnvironmentId) -> Result<(), RepositoryError> {
        let data = serde_json::to_string(id).map_err(|e| RepositoryError::Failed(e.to_string()))?;
        self.append(KIND_ENVIRONMENT, KIND_ENVIRONMENT, &data).await
    }

    async fn get_environment(&self) -> Result<Option<EnvironmentId>, RepositoryError> {
        let rows = self.read_kind(KIND_ENVIRONMENT).await?;
        parse_last(&rows)
    }

    async fn put_observation(&self, observation: &Observation) -> Result<(), RepositoryError> {
        let data = serde_json::to_string(observation)
            .map_err(|e| RepositoryError::Failed(e.to_string()))?;
        self.append(&observation.id().to_string(), KIND_OBSERVATION, &data)
            .await
    }

    async fn put_audit_event(&self, event: &DomainEvent) -> Result<(), RepositoryError> {
        let data =
            serde_json::to_string(event).map_err(|e| RepositoryError::Failed(e.to_string()))?;
        self.append(&event.id().to_string(), KIND_AUDIT, &data)
            .await
    }

    async fn save_health(&self, health: &HealthStatus) -> Result<(), RepositoryError> {
        let data =
            serde_json::to_string(health).map_err(|e| RepositoryError::Failed(e.to_string()))?;
        self.append(KIND_HEALTH, KIND_HEALTH, &data).await
    }

    async fn get_health(&self) -> Result<Option<HealthStatus>, RepositoryError> {
        let rows = self.read_kind(KIND_HEALTH).await?;
        parse_last(&rows)
    }
}

fn record_batch(id: &str, kind: &str, data: &str) -> RecordBatch {
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("kind", DataType::Utf8, false),
        Field::new("data", DataType::Utf8, false),
    ]));

    let id_col: ArrayRef = Arc::new(StringArray::from(vec![id]));
    let kind_col: ArrayRef = Arc::new(StringArray::from(vec![kind]));
    let data_col: ArrayRef = Arc::new(StringArray::from(vec![data]));

    RecordBatch::try_new(schema, vec![id_col, kind_col, data_col]).expect("valid record batch")
}

fn filter_data_by_kind(batches: &[RecordBatch], kind: &str) -> Vec<String> {
    let mut out = Vec::new();
    for batch in batches {
        let Some(kind_col) = batch.column_by_name("kind") else {
            continue;
        };
        let Some(data_col) = batch.column_by_name("data") else {
            continue;
        };
        let kinds = kind_col.as_string::<i32>();
        let datas = data_col.as_string::<i32>();
        for i in 0..kinds.len() {
            if kinds.value(i) == kind {
                out.push(datas.value(i).to_string());
            }
        }
    }
    out
}

fn parse_last<T: serde::de::DeserializeOwned>(
    rows: &[String],
) -> Result<Option<T>, RepositoryError> {
    match rows.last() {
        None => Ok(None),
        Some(json) => serde_json::from_str(json)
            .map(Some)
            .map_err(|e| RepositoryError::Corrupt(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use argus_domain::{DomainEvent, EnvironmentId, EventType, HealthStatus, Severity};
    use chrono::Utc;

    use super::*;
    use crate::DomainRepository;

    fn temp_path(name: &str) -> String {
        std::env::temp_dir()
            .join(format!("argus-lancedb-{name}-{}", std::process::id()))
            .to_string_lossy()
            .into_owned()
    }

    #[tokio::test]
    async fn environment_round_trip() {
        let path = temp_path("env");
        let _ = std::fs::remove_dir_all(&path);

        let repo = LanceDbRepository::connect(&path).await.unwrap();
        assert_eq!(repo.get_environment().await.unwrap(), None);

        let id = EnvironmentId::new();
        repo.save_environment(&id).await.unwrap();
        assert_eq!(repo.get_environment().await.unwrap(), Some(id));

        let _ = std::fs::remove_dir_all(&path);
    }

    #[tokio::test]
    async fn health_round_trip() {
        let path = temp_path("health");
        let _ = std::fs::remove_dir_all(&path);

        let repo = LanceDbRepository::connect(&path).await.unwrap();
        let health = HealthStatus::degraded("test degradation", Utc::now());
        repo.save_health(&health).await.unwrap();
        assert_eq!(repo.get_health().await.unwrap(), Some(health));

        let _ = std::fs::remove_dir_all(&path);
    }

    #[tokio::test]
    async fn observations_and_events_append() {
        let path = temp_path("append");
        let _ = std::fs::remove_dir_all(&path);

        let repo = LanceDbRepository::connect(&path).await.unwrap();
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

        let _ = std::fs::remove_dir_all(&path);
    }
}
