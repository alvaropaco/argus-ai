//! Spec 014 at the daemon boundary, with a FAKE [`McpRuntime`] (the trait
//! seam): deterministic tools, no subprocess — but the real registry, policy,
//! approval, budget, ledger, and executor machinery around them. The
//! transport itself is proven by argus-mcp's stdio subprocess test.

use std::sync::{Arc, Mutex};

use argus_daemon::config::{DaemonConfig, McpConfig, McpServerConfig, McpTransportConfig};
use argus_daemon::control::{ResumeOutcome, RunOutcome};
use argus_daemon::mcp::McpWiring;
use argus_daemon::runtime::Daemon;
use argus_domain::{
    Action, AutonomyMode, BlastRadius, CapabilityDescriptor, CapabilityId, DomainError, McpRuntime,
    Plan, PlanStatus, PlanStep, Reversibility, RiskClass,
};
use argus_events::LocalEventBus;
use argus_executor::{McpToolController, RemediationError};
use serde_json::{Value, json};
use uuid::Uuid;

fn temp_state(tag: &str) -> String {
    let dir = std::env::temp_dir().join(format!("argus-mcp-test-{tag}-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir.join("state.db").display().to_string()
}

fn config() -> DaemonConfig {
    DaemonConfig {
        state_path: temp_state("state"),
        ..DaemonConfig::default()
    }
}

/// The fake seam: a deterministic descriptor table, no I/O.
struct FakeMcpRuntime {
    descriptors: Vec<CapabilityDescriptor>,
}

impl McpRuntime for FakeMcpRuntime {
    fn register(&self, _descriptor: CapabilityDescriptor) -> Result<(), DomainError> {
        Ok(())
    }

    fn capabilities(&self) -> Vec<CapabilityId> {
        self.descriptors.iter().map(|d| d.id().clone()).collect()
    }

    fn descriptors(&self) -> Vec<CapabilityDescriptor> {
        self.descriptors.clone()
    }

    fn unregister_server(&self, server: &str) -> Vec<CapabilityId> {
        let prefix = format!("mcp.{server}.");
        let removed: Vec<CapabilityId> = self
            .capabilities()
            .into_iter()
            .filter(|id| id.as_str().starts_with(&prefix))
            .collect();
        removed
    }
}

/// A deterministic tool port: evidence names the capability; `fail` plays the
/// disconnected-server case.
struct FakeTools {
    fail: bool,
    calls: Mutex<Vec<String>>,
}

impl FakeTools {
    fn new(fail: bool) -> Self {
        Self {
            fail,
            calls: Mutex::new(Vec::new()),
        }
    }
}

impl McpToolController for FakeTools {
    fn call(
        &self,
        capability: &CapabilityId,
        _arguments: &Value,
    ) -> Result<Value, RemediationError> {
        self.calls
            .lock()
            .expect("calls lock")
            .push(capability.as_str().to_string());
        if self.fail {
            return Err(RemediationError::Unavailable(
                "mcp server 'fake' is not connected".to_string(),
            ));
        }
        Ok(json!({ "output": format!("fake:{}", capability.as_str()) }))
    }
}

/// One fake external tool behind `id`, with the spec-014 descriptor shape:
/// the tool's schema, blast `Host`, reversibility `None`.
fn descriptor(id: &str, risk: RiskClass, approval: bool) -> CapabilityDescriptor {
    CapabilityDescriptor::new(
        CapabilityId::new(id).expect("valid mcp id"),
        "mcp:fake",
        id,
        risk,
        semver::Version::new(0, 1, 0),
        json!({
            "type": "object",
            "required": ["message"],
            "properties": { "message": { "type": "string" } },
        }),
        json!({}),
        Reversibility::None,
    )
    .with_blast_radius(BlastRadius::Host)
    .requiring_approval_if(approval)
}

fn wiring(descriptors: Vec<CapabilityDescriptor>, fail: bool) -> Arc<McpWiring> {
    McpWiring::fake(
        Arc::new(FakeMcpRuntime { descriptors }),
        Arc::new(FakeTools::new(fail)),
    )
}

/// A one-step procedure plan for an external tool; the optional rollback is
/// what makes the step `reversible` to the escalation decision.
fn plan(step: &str, rollback: Option<&str>) -> Plan {
    Plan {
        objective: format!("exercise {step}"),
        steps: vec![PlanStep {
            action: Action {
                capability: CapabilityId::new(step).expect("valid capability"),
                resource: None,
                arguments: json!({ "message": "hi" }),
            },
            rollback: rollback.map(|capability| Action {
                capability: CapabilityId::new(capability).expect("valid rollback capability"),
                resource: None,
                arguments: json!({ "message": "undo" }),
            }),
        }],
        preconditions: vec![],
        expected_outcomes: vec![],
        blast_radius: BlastRadius::Host,
        confidence: 0.9,
        status: PlanStatus::Proposed,
        runbook: None,
    }
}

/// AC-001's local half: the discovered tools register beside the bootstrap
/// set with the descriptor shape the spec fixes.
#[tokio::test]
async fn mcp_tools_register_beside_the_bootstrap_set() {
    let daemon = Daemon::init_with_mcp(
        config(),
        wiring(
            vec![descriptor("mcp.fake.echo", RiskClass::HighRisk, true)],
            false,
        ),
    )
    .await
    .unwrap();

    let ids: Vec<String> = daemon
        .capability_descriptors()
        .iter()
        .map(|d| d.id().as_str().to_string())
        .collect();
    assert!(ids.contains(&"mcp.fake.echo".to_string()));
    assert!(
        ids.contains(&"host.status.read".to_string()),
        "the bootstrap set is untouched"
    );

    let registry = daemon.registry_snapshot();
    let echo = registry
        .get(&CapabilityId::new("mcp.fake.echo").unwrap())
        .expect("registered");
    assert_eq!(echo.risk_class(), RiskClass::HighRisk);
    assert!(echo.requires_approval());
    assert_eq!(echo.effective_blast_radius(), BlastRadius::Host);
    assert_eq!(echo.reversibility(), Reversibility::None);
    assert_eq!(
        echo.input_schema(),
        &json!({
            "type": "object",
            "required": ["message"],
            "properties": { "message": { "type": "string" } },
        })
    );
}

/// AC-002's default path: an unknown-provenance tool pauses at EVERY level
/// for an operator grant, and the granted call crosses the ordinary boundary
/// — the tool result lands as execution evidence.
#[tokio::test]
async fn a_high_risk_mcp_step_pauses_then_executes_on_a_grant() {
    let daemon = Daemon::init_with_mcp(
        config(),
        wiring(
            vec![descriptor("mcp.fake.echo", RiskClass::HighRisk, true)],
            false,
        ),
    )
    .await
    .unwrap();
    let events = LocalEventBus::new(16);

    let outcome = daemon
        .run_remediation(
            &plan("mcp.fake.echo", None),
            AutonomyMode::L4Autonomous,
            &events,
        )
        .await;
    let RunOutcome::Pending(pending) = outcome else {
        panic!("a HighRisk external tool must pause at every level, never auto-execute");
    };

    let granted = daemon.grant_approval(pending.token, "operator").unwrap();
    let outcome = daemon.resume_remediation(&granted, &events).await;
    let ResumeOutcome::Finished(report) = outcome else {
        panic!("a resumed plan is authorized and cannot pause");
    };
    assert_eq!(report.status, PlanStatus::Completed, "{report:?}");
    assert_eq!(report.executions.len(), 1);
    assert_eq!(
        report.executions[0].evidence["output"], "fake:mcp.fake.echo",
        "the tool's result lands as bounded execution evidence"
    );
}

/// AC-002's override semantics: the operator's `risk = "controlled"` puts the
/// tool under the autonomy matrix — at L4 it executes within budget, with no
/// pause at all, when the procedure declares its rollback.
#[tokio::test]
async fn an_overridden_controlled_tool_auto_executes_at_l4_within_budget() {
    let daemon = Daemon::init_with_mcp(
        config(),
        wiring(
            vec![
                descriptor("mcp.fake.echo", RiskClass::Controlled, false),
                descriptor("mcp.fake.undo", RiskClass::Controlled, false),
            ],
            false,
        ),
    )
    .await
    .unwrap();
    let events = LocalEventBus::new(16);

    let outcome = daemon
        .run_remediation(
            &plan("mcp.fake.echo", Some("mcp.fake.undo")),
            AutonomyMode::L4Autonomous,
            &events,
        )
        .await;
    let RunOutcome::Finished(report) = outcome else {
        panic!("an overridden controlled tool is matrix-governed at L4, not approval-gated");
    };
    assert_eq!(report.status, PlanStatus::Completed, "{report:?}");
    assert!(report.requires_approval.is_empty());
    assert_eq!(
        report.executions[0].evidence["output"],
        "fake:mcp.fake.echo"
    );

    // AC-005: the call is audited in the ledger like any capability — one
    // ActionEvent for `mcp.fake.echo`, allowed and completed, with the step's
    // arguments riding along (the budget reserve ran through the ordinary
    // gate above it).
    let events = daemon
        .repository()
        .list_action_events(&Default::default())
        .await
        .unwrap();
    let echo_events: Vec<_> = events
        .iter()
        .filter(|event| event.kind == "mcp.fake.echo")
        .collect();
    assert_eq!(
        echo_events.len(),
        1,
        "exactly one audited action for the tool call: {events:?}"
    );
    assert_eq!(echo_events[0].verdict, "allow");
    assert_eq!(echo_events[0].outcome, "ok");
    assert_eq!(echo_events[0].args["message"], "hi");
}

/// AC-003's boundary half: a down server is a typed error — the step fails,
/// the plan fails, and the daemon stays healthy (its tools stay registered,
/// visible failure).
#[tokio::test]
async fn a_down_server_fails_the_step_typed_and_the_daemon_stays_healthy() {
    let daemon = Daemon::init_with_mcp(
        config(),
        wiring(
            vec![descriptor("mcp.fake.echo", RiskClass::HighRisk, true)],
            true,
        ),
    )
    .await
    .unwrap();
    let events = LocalEventBus::new(16);

    let outcome = daemon
        .run_remediation(
            &plan("mcp.fake.echo", None),
            AutonomyMode::L0Observe,
            &events,
        )
        .await;
    let RunOutcome::Pending(pending) = outcome else {
        panic!("the approval gate runs before the executor");
    };
    let granted = daemon.grant_approval(pending.token, "operator").unwrap();
    let outcome = daemon.resume_remediation(&granted, &events).await;
    let ResumeOutcome::Finished(report) = outcome else {
        panic!("a resumed plan is authorized and cannot pause");
    };
    assert_eq!(report.status, PlanStatus::Failed);
    let execution = &report.executions[0];
    assert_eq!(execution.status, argus_domain::ExecutionStatus::Failed);
    let message = execution.evidence["error"].as_str().unwrap_or_default();
    assert!(
        message.contains("not connected"),
        "the typed failure rides the evidence: {message}"
    );

    // The daemon is healthy and the tool is still offered — visible failure,
    // never a silent disappearance (spec 014 FR-004 default).
    assert_eq!(
        daemon.health(),
        argus_domain::HealthStatus::ready(chrono::Utc::now())
    );
    assert!(
        daemon
            .capability_descriptors()
            .iter()
            .any(|d| d.id().as_str() == "mcp.fake.echo")
    );
}

/// AC-006: zero `[mcp]` servers — no connects, no discovered tools, and
/// byte-identical capabilities to the pre-spec surface. (Since spec 015 the
/// wiring itself stays available so a delivered `mcp` document can reconcile
/// a topology in live, without a restart or an executor rebuild; the
/// observable guarantee is the capability surface, which is what this
/// asserts.)
#[tokio::test]
async fn zero_mcp_servers_is_byte_identical_to_today() {
    let daemon = Daemon::init(config()).await.unwrap();
    assert!(
        daemon.mcp_servers().is_empty(),
        "no server is desired when nothing is configured"
    );

    let descriptors = daemon.capability_descriptors();
    assert_eq!(
        descriptors.len(),
        25,
        "exactly the bootstrap set: {:#?}",
        descriptors
            .iter()
            .map(|d| d.id().as_str())
            .collect::<Vec<_>>()
    );
    assert!(
        descriptors
            .iter()
            .all(|d| !d.id().as_str().starts_with("mcp.")),
        "no mcp capability exists when nothing is configured"
    );
}

/// AC-004 over the real chain: `enabled = false` in config — no wiring, no
/// runtime, no registered tools — and a call refused as unsupported.
#[tokio::test]
async fn a_disabled_server_contributes_nothing() {
    let mut config = config();
    config.mcp = McpConfig {
        servers: vec![McpServerConfig {
            name: "fake".to_string(),
            transport: McpTransportConfig::Stdio {
                command: "never-spawned".to_string(),
                args: vec![],
            },
            enabled: false,
            timeout_seconds: 30,
            tools: None,
            overrides: vec![],
        }],
    };
    let daemon = Daemon::init(config).await.unwrap();
    assert!(
        !daemon
            .capability_descriptors()
            .iter()
            .any(|d| d.id().as_str().starts_with("mcp.")),
        "a disabled server's tools are unregistered"
    );
    // A call is refused as unknown — deny-by-default at the registry.
    let request = argus_domain::CapabilityRequest::new(
        CapabilityId::new("mcp.fake.echo").unwrap(),
        argus_domain::Principal::new(Some(0), Some(0)),
        None,
        json!({ "message": "hi" }),
        argus_domain::RequestContext::new(
            Uuid::new_v4(),
            semver::Version::new(0, 1, 0),
            argus_domain::Principal::new(Some(0), Some(0)),
            chrono::Utc::now(),
        ),
    );
    let error = daemon.authorize_and_execute(request).unwrap_err();
    assert!(
        matches!(error, argus_daemon::DispatchError::Denied(_)),
        "an unregistered mcp call is refused: {error:?}"
    );
}
