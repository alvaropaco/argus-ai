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

    let names: Vec<String> = daemon
        .runbooks()
        .list()
        .into_iter()
        .map(|rb| rb.name.clone())
        .collect();
    assert_eq!(names, ["restart-failed"]);
    assert_eq!(
        daemon
            .runbooks()
            .by_name("restart-failed")
            .unwrap()
            .status(),
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

// --- Spec 010: procedure plans — runbook attribution by construction ---

/// A provider that counts consultations, so "the provider was not consulted"
/// is observed, not inferred.
struct CountingProvider {
    inner: Arc<FakeDecisionProvider>,
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

impl CountingProvider {
    fn new(inner: Arc<FakeDecisionProvider>) -> (Self, Arc<std::sync::atomic::AtomicUsize>) {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        (
            Self {
                inner,
                calls: Arc::clone(&calls),
            },
            calls,
        )
    }
}

#[async_trait::async_trait]
impl argus_ai_core::decision::provider::DecisionProvider for CountingProvider {
    async fn decide(
        &self,
        request: argus_ai_core::decision::types::DecisionRequest,
    ) -> Result<
        argus_ai_core::decision::types::DecisionResponse,
        argus_ai_core::decision::error::DecisionError,
    > {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.decide(request).await
    }
}

/// A directory-loaded runbook at the full ladder: the loader replays the
/// declared gates, so it loads Promoted — the only status that drives
/// procedures (ADR-0036 §3).
fn promoted_runbook_dir(name: &str, allowed_actions: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("procedure.toml"),
        format!(
            r#"
name = "{name}"
trigger = {{ symptom = "host-health" }}
allowed_actions = [{allowed_actions}]
rollback = [{allowed_actions}]
gates = ["evaluation", "simulation", "validation", "policy", "approval", "promotion"]

[[validation]]
description = "unit active again"
attribute = "unit.active_state"
comparison = {{ equal = "active" }}
"#
        ),
    )
    .unwrap();
    dir
}

#[tokio::test]
async fn a_promoted_runbook_drives_an_attributed_procedure_plan_without_a_provider() {
    let dir = promoted_runbook_dir("restart-failed", "\"host.service.restart\"");
    let daemon = {
        let mut config = test_config("procedure-noprovider");
        config.brain.runbooks_dir = Some(dir.path().to_string_lossy().into_owned());
        Daemon::init(config).await.unwrap()
    };
    assert_eq!(
        daemon
            .runbooks()
            .by_name("restart-failed")
            .expect("promoted runbook loaded")
            .status(),
        argus_runbooks::RunbookStatus::Promoted
    );

    let record = brain::cycle_with_evidence(
        &daemon,
        &BrainConfig::default(),
        vec![("argus-test.service".into(), "failed failed".into())],
        true,
    )
    .await;

    // AC-001: the attributed procedure plan, no provider consulted (there is
    // none — a procedure plan is not AI output at all).
    assert_eq!(
        record.plan_objective.as_deref(),
        Some("procedure: restart-failed")
    );
    assert_eq!(record.runbook.as_deref(), Some("restart-failed"));
    assert_eq!(record.steps, ["host.service.restart"]);
    assert!(!record.provider_available);
    assert!(record.decision.is_some());

    // The persisted plan carries the attribution (FR-002) — `plan.list` and
    // the sentinel read it from here.
    let plans = daemon.repository().list_plans().await.unwrap();
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].1.runbook.as_deref(), Some("restart-failed"));

    // The episode records the runbook's name as its remediation signature
    // (FR-004) — the linkage the Evaluation gate accepts.
    let episodes = daemon.repository().list_episodes().await.unwrap();
    assert_eq!(episodes.len(), 1);
    assert_eq!(
        episodes[0].1.remediation.as_deref(),
        Some("procedure: restart-failed")
    );
}

#[tokio::test]
async fn a_procedure_plan_never_consults_a_configured_provider() {
    let dir = promoted_runbook_dir("restart-failed", "\"host.service.restart\"");
    let mut config = test_config("procedure-counting");
    config.brain.runbooks_dir = Some(dir.path().to_string_lossy().into_owned());
    let daemon = Daemon::init(config).await.unwrap();
    let (provider, calls) = CountingProvider::new(restarting_provider());
    daemon.set_provider(Some(Arc::new(provider)));

    let record = brain::cycle_with_evidence(
        &daemon,
        &BrainConfig::default(),
        vec![("argus-test.service".into(), "failed failed".into())],
        true,
    )
    .await;

    assert_eq!(
        record.plan_objective.as_deref(),
        Some("procedure: restart-failed")
    );
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "a procedure plan exists precisely to be deterministic"
    );
}

#[tokio::test]
async fn without_a_matching_promoted_runbook_the_provider_path_is_unchanged() {
    // No runbooks at all.
    let daemon = Daemon::init(test_config("provider-empty-library"))
        .await
        .unwrap();
    let (provider, calls) = CountingProvider::new(restarting_provider());
    daemon.set_provider(Some(Arc::new(provider)));

    let record = brain::cycle_with_evidence(
        &daemon,
        &BrainConfig::default(),
        vec![("argus-test.service".into(), "failed failed".into())],
        true,
    )
    .await;

    assert_eq!(
        record.plan_objective.as_deref(),
        Some("restore the host service")
    );
    assert!(
        record.runbook.is_none(),
        "a provider plan carries no attribution (AC-002)"
    );
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the provider reasoned"
    );

    // A candidate (gateless) runbook at the trigger never drives a procedure:
    // promotion is what makes a runbook load-bearing.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("candidate.toml"),
        r#"
name = "candidate-procedure"
trigger = { symptom = "host-health" }
allowed_actions = ["host.service.restart"]
rollback = ["host.service.restart"]
"#,
    )
    .unwrap();
    let mut config = test_config("provider-candidate-only");
    config.brain.runbooks_dir = Some(dir.path().to_string_lossy().into_owned());
    let daemon = Daemon::init(config).await.unwrap();
    let (provider, calls) = CountingProvider::new(restarting_provider());
    daemon.set_provider(Some(Arc::new(provider)));

    let record = brain::cycle_with_evidence(
        &daemon,
        &BrainConfig::default(),
        vec![("argus-test.service".into(), "failed failed".into())],
        true,
    )
    .await;

    assert_eq!(
        record.plan_objective.as_deref(),
        Some("restore the host service")
    );
    assert!(record.runbook.is_none());
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_promoted_runbook_that_cannot_express_the_situation_falls_through() {
    // The pod action's schema wants `name`; the situation's subject is a
    // failed systemd unit. Zero valid steps → the provider path, honestly.
    let dir = promoted_runbook_dir("pod-procedure", "\"k8s.pod.restart\"");
    let mut config = test_config("procedure-zero-steps");
    config.brain.runbooks_dir = Some(dir.path().to_string_lossy().into_owned());
    let daemon = Daemon::init(config).await.unwrap();
    let (provider, calls) = CountingProvider::new(restarting_provider());
    daemon.set_provider(Some(Arc::new(provider)));

    let record = brain::cycle_with_evidence(
        &daemon,
        &BrainConfig::default(),
        vec![("argus-test.service".into(), "failed failed".into())],
        true,
    )
    .await;

    assert_eq!(
        record.plan_objective.as_deref(),
        Some("restore the host service"),
        "the construction fall-through logged and the provider reasoned"
    );
    assert!(record.runbook.is_none());
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_paused_procedure_holds_the_dedup_key_until_its_outcome_lands() {
    // An approval-requiring capability pauses the procedure plan; the paused
    // situation dedups, the grant resumes the SAME attributed plan, and the
    // terminal outcome flows to the runbook and releases the situation
    // (AC-004, AC-006). The unit does not exist on any host, so the resumed
    // run is ATTEMPTED and fails (the report carries the failed execution) —
    // an attempted failure is real history; a refusal with zero executions
    // would record nothing at all.
    let dir = promoted_runbook_dir("restart-failed", "\"host.service.restart\"");
    let mut config = test_config("procedure-pause");
    config.brain.runbooks_dir = Some(dir.path().to_string_lossy().into_owned());
    let daemon = Daemon::init(config).await.unwrap();
    // Well-formed and nonexistent everywhere: the restart is attempted and
    // fails, never an already-desired no-op and never a policy refusal.
    let unit = "argus-procedure-nonexistent.service";

    let first = brain::cycle_with_evidence(
        &daemon,
        &BrainConfig::default(),
        vec![(unit.into(), "failed failed".into())],
        true,
    )
    .await;
    let outcome = first.outcome.expect("the plan ran through the boundary");
    assert!(
        outcome.starts_with("awaiting approval"),
        "the approval-requiring procedure pauses: {outcome}"
    );
    assert!(first.runbook.as_deref() == Some("restart-failed"));
    assert_eq!(
        daemon
            .runbooks()
            .by_name("restart-failed")
            .unwrap()
            .attempts(),
        0,
        "a pause invents no outcome"
    );

    // The claimed dedup key suppresses duplicates while a human decides.
    let second = brain::cycle_with_evidence(
        &daemon,
        &BrainConfig::default(),
        vec![(unit.into(), "failed failed".into())],
        true,
    )
    .await;
    assert!(
        second.plan_objective.is_none(),
        "no duplicate procedure while the first is pending"
    );

    // The grant resumes the same attributed plan: attempted, failed.
    let pending = daemon
        .list_pending_approvals()
        .pop()
        .expect("the paused procedure");
    assert_eq!(pending.plan.runbook.as_deref(), Some("restart-failed"));
    assert!(
        pending.dedup_key.is_some(),
        "the attributed plan carries its dedup key for the resume"
    );
    let granted = daemon.grant_approval(pending.token, "uid=0").unwrap();
    let events = argus_events::LocalEventBus::new(16);
    let resumed = daemon.resume_remediation(&granted, &events).await;
    let report = match resumed {
        argus_daemon::control::ResumeOutcome::Finished(report) => report,
        other => panic!("the granted plan ran to a terminal status: {other:?}"),
    };
    assert!(
        !report.executions.is_empty(),
        "the failure was attempted, not refused: an execution was recorded"
    );

    let runbook = daemon.runbooks().by_name("restart-failed").unwrap();
    assert_eq!(
        runbook.attempts(),
        1,
        "the resumed ATTEMPT's outcome flowed to its runbook (AC-006)"
    );
    assert_eq!(
        runbook.historical_success_rate(),
        Some(0.0),
        "an attempted-and-failed run is unsuccessful history (AC-004)"
    );
    assert_eq!(runbook.gates().len(), 6, "gates do not move on a failure");

    // The released dedup key: the same evidence proposes the same attributed
    // procedure again instead of being suppressed forever.
    let third = brain::cycle_with_evidence(
        &daemon,
        &BrainConfig::default(),
        vec![(unit.into(), "failed failed".into())],
        true,
    )
    .await;
    assert_eq!(
        third.plan_objective.as_deref(),
        Some("procedure: restart-failed"),
        "the released situation reasons again (AC-004)"
    );
}

#[tokio::test]
async fn the_sentinel_report_carries_the_procedure_attribution() {
    let dir = promoted_runbook_dir("restart-failed", "\"host.service.restart\"");
    let mut config = test_config("procedure-sentinel");
    config.brain.runbooks_dir = Some(dir.path().to_string_lossy().into_owned());
    let daemon = Daemon::init(config).await.unwrap();

    brain::cycle_with_evidence(
        &daemon,
        &BrainConfig::default(),
        vec![("argus-test.service".into(), "failed failed".into())],
        true,
    )
    .await;
    // The spawn loop is what records the shared cycle state; stand in for it
    // so the last_cycle serialization is exercised directly.
    daemon
        .brain_state()
        .record(argus_daemon::brain_state::BrainCycleRecord {
            cycle_id: Some(uuid::Uuid::new_v4()),
            evidence: vec!["argus-test.service: failed failed".into()],
            provider_available: false,
            decision: Some("procedure plan from promoted runbook 'restart-failed'".into()),
            plan: Some("procedure: restart-failed".into()),
            runbook: Some("restart-failed".into()),
            outcome: Some("Denied".into()),
            at: chrono::Utc::now(),
        });

    let report = argus_daemon::cloud::SentinelSource::snapshot(&daemon)
        .await
        .unwrap();
    assert_eq!(report.plans.len(), 1);
    assert_eq!(
        report.plans[0].get("runbook").and_then(|v| v.as_str()),
        Some("restart-failed"),
        "the plan entry names its runbook (FR-002)"
    );
    let last_cycle = report.last_cycle.expect("a recorded cycle");
    assert_eq!(
        last_cycle.get("runbook").and_then(|v| v.as_str()),
        Some("restart-failed")
    );

    // A provider plan carries no attribution: its entry has no runbook key.
    let plain = Daemon::init(test_config("sentinel-provider-plan"))
        .await
        .unwrap();
    plain.set_provider(Some(restarting_provider()));
    brain::cycle_with_evidence(
        &plain,
        &BrainConfig::default(),
        vec![("argus-test.service".into(), "failed failed".into())],
        true,
    )
    .await;
    let report = argus_daemon::cloud::SentinelSource::snapshot(&plain)
        .await
        .unwrap();
    assert_eq!(report.plans.len(), 1);
    assert!(
        report.plans[0].get("runbook").is_none(),
        "provider plans stay unattributed on the wire"
    );
}
