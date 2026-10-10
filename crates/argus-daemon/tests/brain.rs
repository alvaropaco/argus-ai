//! Spec-005 brain tests: the running observe → reason → act loop with the
//! deterministic fake provider (no host, no cluster, no live API), the
//! observe-only defaults, provider wiring, and runbook loading.

use std::collections::BTreeMap;
use std::sync::Arc;

use argus_ai_core::decision::adapters::FakeDecisionProvider;
use argus_ai_core::decision::types::DecisionAnswer;
use argus_daemon::config::{BrainConfig, DaemonConfig};
use argus_daemon::{Daemon, brain};
use argus_domain::AutonomyMode;

fn test_config(name: &str) -> DaemonConfig {
    DaemonConfig {
        socket_path: std::env::temp_dir()
            .join(format!("argus-brain-{name}-{}.sock", std::process::id()))
            .to_string_lossy()
            .into_owned(),
        state_path: std::env::temp_dir()
            .join(format!("argus-brain-{name}-{}.db", std::process::id()))
            .to_string_lossy()
            .into_owned(),
        ..DaemonConfig::default()
    }
}

fn restarting_provider() -> Arc<FakeDecisionProvider> {
    Arc::new(FakeDecisionProvider::new(BTreeMap::from([
        ("degraded".to_string(), DecisionAnswer::Noul { noul: 0.95 }),
        (
            "remediation".to_string(),
            DecisionAnswer::Choice {
                choice: "restart".into(),
                confidence: 0.9,
                probabilities: BTreeMap::from([
                    ("restart".to_string(), 0.9),
                    ("noop".to_string(), 0.1),
                ]),
            },
        ),
    ])))
}

#[tokio::test]
async fn a_cycle_without_a_provider_observes_only() {
    let daemon = Daemon::init(test_config("noprovider")).await.unwrap();
    let record = brain::cycle_with_evidence(
        &daemon,
        &BrainConfig::default(),
        vec![("argus-test.service".into(), "failed failed".into())],
        true,
    )
    .await;

    assert_eq!(record.evidence, ["argus-test.service: failed failed"]);
    assert!(!record.provider_available);
    assert!(record.decision.is_none());
    assert!(record.plan_objective.is_none());
    // Nothing was remembered: no plan ran.
    assert!(
        daemon
            .repository()
            .list_episodes()
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn at_l0_the_brain_proposes_but_executes_nothing() {
    let daemon = Daemon::init(test_config("l0")).await.unwrap();
    daemon.set_provider(Some(restarting_provider()));

    let record = brain::cycle_with_evidence(
        &daemon,
        &BrainConfig::default(), // autonomy = l0_observe
        vec![("argus-test.service".into(), "failed failed".into())],
        true,
    )
    .await;

    assert!(record.provider_available);
    assert!(record.decision.is_some());
    assert!(record.plan_objective.is_some());
    // L0: the plan ran through the boundary and nothing executed — the
    // outcome is the observed pause/deny shape, never a restart.
    let outcome = record.outcome.expect("the plan ran through the boundary");
    assert!(
        outcome.starts_with("Denied") || outcome.starts_with("awaiting"),
        "L0 executes nothing: {outcome}"
    );
}

#[tokio::test]
async fn a_cycle_records_episode_and_procedure_history() {
    let daemon = Daemon::init(test_config("memory")).await.unwrap();
    daemon.set_provider(Some(restarting_provider()));

    let _ = brain::cycle_with_evidence(
        &daemon,
        &BrainConfig::default(),
        vec![("argus-test.service".into(), "failed failed".into())],
        true,
    )
    .await;

    let episodes = daemon.repository().list_episodes().await.unwrap();
    assert_eq!(episodes.len(), 1, "one episode per acting cycle");
    assert_eq!(episodes[0].1.symptom, "host-health");
    assert_eq!(
        episodes[0].1.outcome,
        argus_memory::EpisodeOutcome::Unresolved,
        "L0 never resolves anything"
    );

    let procedures = daemon.repository().list_procedures().await.unwrap();
    assert_eq!(procedures.len(), 1);
    assert_eq!(procedures[0].1.attempts(), 1);

    // A second cycle over DIFFERENT evidence (a second failing unit — the
    // first cycle's key stays claimed while its plan awaits approval)
    // aggregates into the same procedure record.
    let second = brain::cycle_with_evidence(
        &daemon,
        &BrainConfig::default(),
        vec![("argus-test2.service".into(), "failed failed".into())],
        true,
    )
    .await;
    assert!(
        second.plan_objective.is_some(),
        "new evidence reasons again"
    );
    let procedures = daemon.repository().list_procedures().await.unwrap();
    assert_eq!(procedures.len(), 1);
    assert_eq!(procedures[0].1.attempts(), 2, "history aggregates");
    let episodes = daemon.repository().list_episodes().await.unwrap();
    assert_eq!(episodes.len(), 2, "one episode per acting cycle");
}

#[tokio::test]
async fn a_low_confidence_decision_proposes_nothing() {
    let daemon = Daemon::init(test_config("lowconf")).await.unwrap();
    daemon.set_provider(Some(Arc::new(FakeDecisionProvider::new(BTreeMap::from([
        ("degraded".to_string(), DecisionAnswer::Noul { noul: 0.6 }),
        (
            "remediation".to_string(),
            DecisionAnswer::Choice {
                choice: "restart".into(),
                confidence: 0.2,
                probabilities: BTreeMap::from([
                    ("restart".to_string(), 0.2),
                    ("noop".to_string(), 0.8),
                ]),
            },
        ),
    ])))));

    let record = brain::cycle_with_evidence(
        &daemon,
        &BrainConfig::default(),
        vec![("argus-test.service".into(), "failed failed".into())],
        true,
    )
    .await;

    // The gate (threshold 0.3) refused the low-confidence remediation.
    assert!(record.plan_objective.is_none());
    assert!(record.outcome.is_none());
}

#[tokio::test]
async fn the_provider_factory_maps_configured_providers() {
    // deepseek without a credential → None (observe-only, never a crash).
    let mut config = test_config("factory");
    config.model.provider = "deepseek".into();
    config.model.model = "deepseek-chat".into();
    let secrets = argus_daemon::cloud::CloudSecretStore::default_location();
    assert!(brain::build_provider(&config, &secrets).is_none());

    // Unknown provider → None.
    config.model.provider = "telepathy".into();
    assert!(brain::build_provider(&config, &secrets).is_none());

    // laya → the sidecar adapter, no credential needed.
    config.model.provider = "laya".into();
    config.model.base_url = Some("http://127.0.0.1:8000".into());
    assert!(brain::build_provider(&config, &secrets).is_some());

    // none/empty → None by design.
    config.model.provider = String::new();
    assert!(brain::build_provider(&config, &secrets).is_none());
}

#[tokio::test]
async fn runbooks_load_at_startup_from_the_configured_directory() {
    let dir = std::env::temp_dir().join(format!("argus-runbooks-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("restart-failed.toml"),
        r#"
name = "restart-failed"
trigger = { symptom = "host-health" }
required_evidence = ["service"]
allowed_actions = ["host.service.restart"]
rollback = ["host.service.restart"]

[[validation]]
description = "unit active again"
attribute = "unit.active_state"
comparison = { equal = "active" }
"#,
    )
    .unwrap();

    let mut config = test_config("runbooks");
    config.brain.runbooks_dir = Some(dir.to_string_lossy().into_owned());
    let daemon = Daemon::init(config).await.unwrap();

    let names: Vec<&str> = daemon
        .runbooks()
        .list()
        .into_iter()
        .map(|rb| rb.name.as_str())
        .collect();
    assert_eq!(names, ["restart-failed"]);
    assert_eq!(
        daemon.runbooks().list()[0].status(),
        argus_runbooks::RunbookStatus::Candidate
    );
    let _ = AutonomyMode::L0Observe;
    let _ = std::fs::remove_dir_all(&dir);
}

/// IPC end-to-end: `brain.diagnose` without a provider is NotReady (AC-003).
#[tokio::test]
async fn brain_diagnose_reports_not_ready_without_a_provider() {
    let config = test_config("ipc");
    let daemon = Arc::new(Daemon::init(config).await.unwrap());
    let response = argus_daemon::handler::handle(
        daemon.as_ref(),
        argus_domain::Principal::new(Some(0), Some(0)),
        argus_ipc::Request::new(
            argus_ipc::Operation::BrainDiagnose,
            uuid::Uuid::new_v4(),
            serde_json::json!({}),
        ),
    )
    .await;
    assert!(!response.ok);
    assert_eq!(
        response.error.expect("error body").code,
        argus_ipc::ErrorCode::NotReady
    );
}
