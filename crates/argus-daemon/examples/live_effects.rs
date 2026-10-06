//! Live Linux effect harness (run as root on a Linux host).
//!
//! Drives the daemon's REAL remediation path — the same
//! governor → policy → autonomy → executor → validation loop the daemon
//! runs — against the live controllers (cgroup v2 file writes, the Docker
//! socket, `/proc` signals). No mocks. Intended for validating deployments
//! on a real host; the test suites cover the mocked behavior.
//!
//! Usage (as root):
//! ```text
//! argus-live sentinel
//! argus-live freeze <relative-cgroup> [thaw-too]   # e.g. system.slice/argus-live-test.service
//! argus-live restart-container <container-id-or-name>
//! argus-live signal <pid> <stop|cont|term>
//! argus-live service-restart <unit>
//! ```
//!
//! Effects are real: `freeze` writes `cgroup.freeze`, `restart-container`
//! POSTs to the Docker socket, `signal` calls kill(2). Point them at
//! disposable targets only (transient units, test containers).

use std::sync::Arc;

use argus_daemon::config::DaemonConfig;
use argus_daemon::runtime::Daemon;
use argus_daemon::sentinel::Environment;
use argus_domain::{Action, AutonomyMode, BlastRadius, CapabilityId, Plan, PlanStatus, PlanStep};
use argus_events::LocalEventBus;
use serde_json::json;

fn config() -> DaemonConfig {
    DaemonConfig {
        // A scratch state file: the harness must not disturb a running
        // daemon's database. Effects happen on the host, not in this file.
        state_path: "/tmp/argus-live-effects.db".to_string(),
        socket_path: "/tmp/argus-live-effects.sock".to_string(),
        ..DaemonConfig::default()
    }
}

fn one_step_plan(
    capability: &str,
    args: serde_json::Value,
    rollback: Option<(&str, serde_json::Value)>,
) -> Plan {
    Plan {
        objective: format!("live effect: {capability}"),
        steps: vec![PlanStep {
            action: Action {
                capability: CapabilityId::new(capability).unwrap(),
                resource: None,
                arguments: args,
            },
            rollback: rollback.map(|(cap, args)| Action {
                capability: CapabilityId::new(cap).unwrap(),
                resource: None,
                arguments: args,
            }),
        }],
        preconditions: vec![],
        expected_outcomes: vec![],
        blast_radius: BlastRadius::Host,
        confidence: 0.95,
        status: PlanStatus::Proposed,
    }
}

fn print_outcome(outcome: argus_daemon::control::RunOutcome) {
    match outcome {
        argus_daemon::control::RunOutcome::Finished(report) => {
            println!("status: {:?}", report.status);
            println!("executions: {}", report.executions.len());
            for e in &report.executions {
                println!("  - {:?} {}", e.status, e.action.capability.as_str());
                println!("    evidence: {}", e.evidence);
            }
            println!("denied: {:?}", report.denied);
            println!("requires_approval: {:?}", report.requires_approval);
        }
        argus_daemon::control::RunOutcome::Pending(p) => {
            println!("PENDING approval token {} (plan paused)", p.token);
        }
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(String::as_str).unwrap_or("help");
    let daemon = Daemon::init(config()).await.expect("daemon init");

    match cmd {
        "sentinel" => {
            let view = daemon.sentinel_snapshot().await;
            println!("{}", serde_json::to_string_pretty(&view).unwrap());
            let _ = Environment::Production;
        }
        "freeze" => {
            let path = args
                .get(2)
                .expect("usage: freeze <relative-cgroup> [thaw-too]")
                .clone();
            let thaw_too = args.get(3).is_some_and(|a| a == "thaw-too");
            // Classify the subject so the governor permits the adjustment —
            // the same shape the memory-pressure scenario uses (AC-013):
            // BestEffort subjects are legitimate autopilot targets.
            daemon
                .governor()
                .classify(&path, argus_policy::Criticality::BestEffort);
            let plan = one_step_plan(
                CapabilityId::HOST_CGROUP_FREEZE,
                json!({ "path": path }),
                Some((CapabilityId::HOST_CGROUP_THAW, json!({ "path": path }))),
            );
            print_outcome(
                daemon
                    .run_remediation(&plan, AutonomyMode::L4Autonomous, &LocalEventBus::new(64))
                    .await,
            );
            if thaw_too {
                let plan = one_step_plan(
                    CapabilityId::HOST_CGROUP_THAW,
                    json!({ "path": path }),
                    None,
                );
                print_outcome(
                    daemon
                        .run_remediation(&plan, AutonomyMode::L4Autonomous, &LocalEventBus::new(64))
                        .await,
                );
            }
        }
        "restart-container" => {
            let id = args
                .get(2)
                .expect("usage: restart-container <id> [approve]")
                .clone();
            let approve = args.get(3).is_some_and(|a| a == "approve");
            let plan = one_step_plan(
                CapabilityId::CONTAINER_RESTART,
                json!({ "container": id }),
                None,
            );
            let events = LocalEventBus::new(64);
            match daemon
                .run_remediation(&plan, AutonomyMode::L4Autonomous, &events)
                .await
            {
                argus_daemon::control::RunOutcome::Pending(pending) if approve => {
                    println!(
                        "paused for approval; granting token {} and resuming",
                        pending.token
                    );
                    daemon
                        .grant_approval(pending.token, "live-harness")
                        .expect("grant");
                    match daemon.resume_remediation(&pending, &events).await {
                        argus_daemon::control::ResumeOutcome::Finished(report) => {
                            print_outcome(argus_daemon::control::RunOutcome::Finished(report));
                        }
                        argus_daemon::control::ResumeOutcome::Refused(why) => {
                            println!("resume refused: {why:?}");
                        }
                    }
                }
                outcome => print_outcome(outcome),
            }
        }
        "signal" => {
            let pid: i64 = args
                .get(2)
                .expect("usage: signal <pid> <stop|cont|term> [approve]")
                .parse()
                .unwrap();
            let sig = args.get(3).cloned().unwrap_or_else(|| "stop".into());
            let approve = args.get(4).is_some_and(|a| a == "approve");
            let plan = one_step_plan(
                CapabilityId::HOST_PROCESS_SIGNAL,
                json!({ "pid": pid, "signal": sig }),
                Some((
                    CapabilityId::HOST_PROCESS_SIGNAL,
                    json!({ "pid": pid, "signal": "cont" }),
                )),
            );
            // host.process.signal is policy-gated: expect the approval
            // round-trip unless a grant exists.
            let events = LocalEventBus::new(64);
            match daemon
                .run_remediation(&plan, AutonomyMode::L4Autonomous, &events)
                .await
            {
                argus_daemon::control::RunOutcome::Pending(pending) if approve => {
                    println!("paused for approval; granting token {}", pending.token);
                    daemon
                        .grant_approval(pending.token, "live-harness")
                        .expect("grant");
                    match daemon.resume_remediation(&pending, &events).await {
                        argus_daemon::control::ResumeOutcome::Finished(report) => {
                            print_outcome(argus_daemon::control::RunOutcome::Finished(report));
                        }
                        argus_daemon::control::ResumeOutcome::Refused(why) => {
                            println!("resume refused: {why:?}");
                        }
                    }
                }
                outcome => print_outcome(outcome),
            }
        }
        "service-restart" => {
            let unit = args
                .get(2)
                .expect("usage: service-restart <unit> [approve]")
                .clone();
            let approve = args.get(3).is_some_and(|a| a == "approve");
            let plan = one_step_plan(
                CapabilityId::HOST_SERVICE_RESTART,
                json!({ "unit": unit }),
                None,
            );
            let events = LocalEventBus::new(64);
            match daemon
                .run_remediation(&plan, AutonomyMode::L4Autonomous, &events)
                .await
            {
                argus_daemon::control::RunOutcome::Pending(pending) if approve => {
                    println!("paused for approval; granting token {}", pending.token);
                    daemon
                        .grant_approval(pending.token, "live-harness")
                        .expect("grant");
                    match daemon.resume_remediation(&pending, &events).await {
                        argus_daemon::control::ResumeOutcome::Finished(report) => {
                            print_outcome(argus_daemon::control::RunOutcome::Finished(report));
                        }
                        argus_daemon::control::ResumeOutcome::Refused(why) => {
                            println!("resume refused: {why:?}");
                        }
                    }
                }
                outcome => print_outcome(outcome),
            }
        }
        _ => {
            eprintln!(
                "commands: sentinel | freeze <cgroup> [thaw-too] | restart-container <id> [approve] | signal <pid> <sig> [approve] | service-restart <unit> [approve]"
            );
            std::process::exit(2);
        }
    }
    let _ = Arc::new(0u8); // keep Arc import used across cfg variants
}
