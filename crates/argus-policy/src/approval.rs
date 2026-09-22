//! Per-invocation execution approvals.
//!
//! An approval authorizes exactly one invocation, identified by its `command_id`,
//! and only until it expires. Binding approval to a capability instead would
//! silently convert a reviewed action into a standing, unreviewed permission
//! (ADR-0020 §3).

use std::collections::HashMap;
use std::sync::Mutex;

use argus_domain::ExecutionApproval;
use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

/// Approvals granted locally, keyed by the invocation they authorize.
#[derive(Debug, Default)]
pub struct ApprovalStore {
    approvals: Mutex<HashMap<Uuid, ExecutionApproval>>,
}

impl ApprovalStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records an approval, replacing any earlier one for the same invocation.
    pub fn grant(&self, approval: ExecutionApproval) {
        let mut approvals = self.approvals.lock().expect("approval store is not poisoned");
        approvals.insert(approval.command_id, approval);
    }

    /// Grants a timed approval, attributing it to the local principal that gave it.
    pub fn grant_for(
        &self,
        command_id: Uuid,
        granted_by: impl Into<String>,
        granted_at: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> ExecutionApproval {
        let approval =
            ExecutionApproval::grant(command_id, granted_by, granted_at, expires_at);
        self.grant(approval.clone());
        approval
    }

    /// Grants an approval valid for `ttl` from `now`.
    pub fn grant_for_a_while(
        &self,
        command_id: Uuid,
        granted_by: impl Into<String>,
        now: DateTime<Utc>,
        ttl: Duration,
    ) -> ExecutionApproval {
        self.grant_for(command_id, granted_by, now, now + ttl)
    }

    /// Whether a valid, unexpired approval exists for this invocation.
    ///
    /// An approval for a different invocation never authorizes this one, and an
    /// expired approval authorizes nothing.
    pub fn authorizes(&self, command_id: Uuid, now: DateTime<Utc>) -> bool {
        let approvals = self.approvals.lock().expect("approval store is not poisoned");
        approvals
            .get(&command_id)
            .is_some_and(|approval| approval.authorizes(command_id, now))
    }

    pub fn get(&self, command_id: Uuid) -> Option<ExecutionApproval> {
        let approvals = self.approvals.lock().expect("approval store is not poisoned");
        approvals.get(&command_id).cloned()
    }

    /// Forgets an invocation's approval, so it cannot authorize a later attempt.
    pub fn revoke(&self, command_id: Uuid) -> Option<ExecutionApproval> {
        let mut approvals = self.approvals.lock().expect("approval store is not poisoned");
        approvals.remove(&command_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use argus_domain::ApprovalState;

    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    #[test]
    fn a_granted_approval_authorizes_exactly_its_invocation() {
        let store = ApprovalStore::new();
        let command = Uuid::new_v4();
        let other = Uuid::new_v4();

        store.grant_for_a_while(command, "root", now(), Duration::minutes(5));

        assert!(store.authorizes(command, now()));
        assert!(
            !store.authorizes(other, now()),
            "an approval must never authorize a different invocation"
        );
    }

    #[test]
    fn an_expired_approval_authorizes_nothing() {
        let store = ApprovalStore::new();
        let command = Uuid::new_v4();
        let granted_at = now() - Duration::minutes(10);

        store.grant_for(command, "root", granted_at, granted_at + Duration::minutes(1));

        assert!(
            !store.authorizes(command, now()),
            "an approval past its expiry must not authorize execution"
        );
    }

    #[test]
    fn an_approval_not_yet_in_force_still_authorizes_in_its_window() {
        let store = ApprovalStore::new();
        let command = Uuid::new_v4();
        let granted_at = now();

        store.grant_for(command, "root", granted_at, granted_at + Duration::seconds(60));

        assert!(store.authorizes(command, granted_at + Duration::seconds(30)));
        assert!(!store.authorizes(command, granted_at + Duration::seconds(60)));
    }

    #[test]
    fn the_granting_principal_is_recorded() {
        let store = ApprovalStore::new();
        let command = Uuid::new_v4();

        let approval = store.grant_for_a_while(command, "operator", now(), Duration::minutes(5));

        assert_eq!(approval.granted_by, "operator");
        assert_eq!(approval.state, ApprovalState::Granted);
        assert_eq!(store.get(command).map(|a| a.granted_by), Some("operator".into()));
    }

    #[test]
    fn a_denied_approval_never_authorizes() {
        let store = ApprovalStore::new();
        let command = Uuid::new_v4();
        let mut approval = ExecutionApproval::grant(command, "root", now(), now() + Duration::minutes(5));
        approval.state = ApprovalState::Denied;
        store.grant(approval);

        assert!(!store.authorizes(command, now()));
    }

    #[test]
    fn revoking_removes_the_authorization() {
        let store = ApprovalStore::new();
        let command = Uuid::new_v4();
        store.grant_for_a_while(command, "root", now(), Duration::minutes(5));

        assert!(store.revoke(command).is_some());
        assert!(!store.authorizes(command, now()));
        assert!(store.get(command).is_none());
    }

    #[test]
    fn regranting_replaces_the_earlier_decision() {
        let store = ApprovalStore::new();
        let command = Uuid::new_v4();
        let mut denied = ExecutionApproval::grant(command, "root", now(), now() + Duration::minutes(5));
        denied.state = ApprovalState::Denied;
        store.grant(denied);
        assert!(!store.authorizes(command, now()));

        store.grant_for_a_while(command, "operator", now(), Duration::minutes(5));
        assert!(store.authorizes(command, now()));
    }
}
