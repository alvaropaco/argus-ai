//! Spec-004 FR-001: the memory layers (episodic episodes, semantic facts,
//! procedural records) persist through `DomainRepository` in both backends,
//! listing in insertion order, with the documented supersede semantics.

use argus_memory::{Episode, EpisodeOutcome, Fact, ProcedureRecord, ProcedureStatus};
use argus_state::{DomainRepository, InMemoryRepository, SqliteRepository};
use chrono::{TimeZone, Utc};
use uuid::Uuid;

fn ts() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 5, 12, 0, 0).unwrap()
}

fn episode(symptom: &str) -> Episode {
    Episode {
        id: Uuid::new_v4(),
        subject: argus_domain::ResourceId::new("service", "api").unwrap(),
        symptom: symptom.to_string(),
        resource_class: "service".to_string(),
        root_cause: Some("leak".into()),
        remediation: Some("restart".into()),
        outcome: EpisodeOutcome::Resolved,
        change_proximity: Some(chrono::Duration::minutes(30)),
        started_at: ts(),
        resolved_at: Some(ts()),
    }
}

fn fact(attribute: &str, value: &str) -> Fact {
    Fact {
        subject: argus_domain::ResourceId::new("host", "local").unwrap(),
        attribute: attribute.to_string(),
        value: value.to_string(),
        source: "test".to_string(),
        learned_at: ts(),
    }
}

fn procedure(name: &str) -> ProcedureRecord {
    let mut p = ProcedureRecord::new(
        Uuid::new_v4(),
        "restart-loop",
        name,
        ProcedureStatus::Candidate,
        ts(),
    );
    p.record_outcome(true, ts()).unwrap();
    p.record_outcome(false, ts()).unwrap();
    p
}

async fn exercise_memory_round_trip(repo: &dyn DomainRepository) {
    // Episodes: idempotent on id; a re-put replaces and moves to the end
    // of insertion order (the same supersede rule as facts, and what
    // INSERT OR REPLACE's fresh rowid yields in SQLite).
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    repo.put_episode(first, &episode("restart-loop"))
        .await
        .unwrap();
    repo.put_episode(second, &episode("oom-killed"))
        .await
        .unwrap();
    repo.put_episode(first, &episode("disk-pressure"))
        .await
        .unwrap();
    let episodes = repo.list_episodes().await.unwrap();
    assert_eq!(episodes.len(), 2);
    assert_eq!(episodes[0].0, second);
    assert_eq!(episodes[0].1.symptom, "oom-killed");
    assert_eq!(episodes[1].0, first);
    assert_eq!(episodes[1].1.symptom, "disk-pressure");

    // Facts: supersede on (subject, attribute).
    repo.put_fact(&fact("kernel.release", "6.8.0"))
        .await
        .unwrap();
    repo.put_fact(&fact("cpu.cores", "8")).await.unwrap();
    repo.put_fact(&fact("kernel.release", "6.9.1"))
        .await
        .unwrap();
    let facts = repo.list_facts().await.unwrap();
    assert_eq!(facts.len(), 2);
    let kernel = facts
        .iter()
        .find(|f| f.attribute == "kernel.release")
        .unwrap();
    assert_eq!(kernel.value, "6.9.1");

    // Procedures: idempotent on id.
    let pid = Uuid::new_v4();
    repo.put_procedure(pid, &procedure("alpha")).await.unwrap();
    repo.put_procedure(Uuid::new_v4(), &procedure("beta"))
        .await
        .unwrap();
    let mut updated = procedure("alpha");
    updated.status = ProcedureStatus::Approved;
    repo.put_procedure(pid, &updated).await.unwrap();
    let procedures = repo.list_procedures().await.unwrap();
    assert_eq!(procedures.len(), 2);
    let alpha = procedures.iter().find(|(id, _)| *id == pid).unwrap();
    assert_eq!(alpha.1.status, ProcedureStatus::Approved);
    assert_eq!(alpha.1.success_rate(), Some(0.5));
}

#[tokio::test]
async fn in_memory_backend_round_trips_the_memory_layers() {
    let repo = InMemoryRepository::new();
    exercise_memory_round_trip(&repo).await;
}

#[tokio::test]
async fn sqlite_backend_round_trips_the_memory_layers() {
    let dir = tempfile::tempdir().unwrap();
    let repo = SqliteRepository::open(dir.path().join("memory.db")).unwrap();
    exercise_memory_round_trip(&repo).await;

    // And it is durable across connections.
    let reopened = SqliteRepository::open(dir.path().join("memory.db")).unwrap();
    assert_eq!(reopened.list_episodes().await.unwrap().len(), 2);
    assert_eq!(reopened.list_facts().await.unwrap().len(), 2);
    assert_eq!(reopened.list_procedures().await.unwrap().len(), 2);
}
