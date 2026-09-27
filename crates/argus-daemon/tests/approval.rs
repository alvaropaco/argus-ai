//! Operator approval surface: the pending store and its IPC dispatch.

use argus_daemon::Daemon;
use argus_daemon::config::DaemonConfig;
use argus_daemon::control::PendingPlan;
use argus_daemon::handler;
use argus_domain::{BlastRadius, Plan, PlanStatus, Principal};
use argus_ipc::{Operation, Request};
use serde_json::json;
use uuid::Uuid;

fn temp_state(name: &str) -> String {
    std::env::temp_dir()
        .join(format!("argus-approval-{name}-{}.db", std::process::id()))
        .to_string_lossy()
        .into_owned()
}

fn pending(token: Uuid) -> PendingPlan {
    PendingPlan {
        plan: Plan {
            objective: "restore nginx".into(),
            steps: vec![],
            preconditions: vec![],
            expected_outcomes: vec![],
            blast_radius: BlastRadius::Host,
            confidence: 0.9,
            status: PlanStatus::AwaitingApproval,
        },
        token,
        context_hash: "hash-a".into(),
    }
}

fn principal() -> Principal {
    Principal::new(Some(1000), Some(1000))
}

fn request(operation: Operation, payload: serde_json::Value) -> Request {
    Request::new(operation, Uuid::new_v4(), payload)
}

#[tokio::test]
async fn approval_list_grant_and_deny_round_trip() {
    let daemon = Daemon::init(DaemonConfig {
        state_path: temp_state("round-trip"),
        ..DaemonConfig::default()
    })
    .await
    .unwrap();
    let token = Uuid::new_v4();

    daemon.store_pending(pending(token));

    let list = handler::handle(
        &daemon,
        principal(),
        request(Operation::ApprovalList, json!({})),
    )
    .await;
    let listed = list.result.as_ref().unwrap().as_array().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["token"], json!(token.to_string()));

    let grant = handler::handle(
        &daemon,
        principal(),
        request(
            Operation::ApprovalGrant,
            json!({ "token": token.to_string() }),
        ),
    )
    .await;
    assert!(grant.ok, "grant should succeed: {grant:?}");

    // Granting consumed the pending plan, so the list is empty and a deny on
    // the now-consumed token is refused.
    let after = handler::handle(
        &daemon,
        principal(),
        request(Operation::ApprovalList, json!({})),
    )
    .await;
    assert_eq!(after.result.as_ref().unwrap().as_array().unwrap().len(), 0);

    let deny = handler::handle(
        &daemon,
        principal(),
        request(
            Operation::ApprovalDeny,
            json!({ "token": token.to_string() }),
        ),
    )
    .await;
    assert!(
        !deny.ok,
        "denying a consumed token must be refused: {deny:?}"
    );
}
