//! Per-invocation execution approvals.
//!
//! An approval authorizes exactly one invocation, identified by its single-use
//! `token` and bound to a `context_hash`, and only until it expires. Binding
//! approval to a capability instead would silently convert a reviewed action
//! into a standing, unreviewed permission (ADR-0020 §3); the hash binding ties
//! the grant to the exact plan the operator reviewed (ADR-0030 §3).

use std::collections::HashMap;
use std::sync::Mutex;

use argus_domain::ExecutionApproval;
use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

/// Approvals granted locally, keyed by the single-use token they authorize.
#[derive(Debug, Default)]
pub struct ApprovalStore {
    approvals: Mutex<HashMap<Uuid, ExecutionApproval>>,
}

impl ApprovalStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records an approval, replacing any earlier one for the same token.
    pub fn grant(&self, approval: ExecutionApproval) {
        let mut approvals = self
            .approvals
            .lock()
            .expect("approval store is not poisoned");
        approvals.insert(approval.token, approval);
    }

    /// Grants a timed approval, attributing it to the local principal that gave it.
    pub fn grant_for(
        &self,
        token: Uuid,
        context_hash: impl Into<String>,
        granted_by: impl Into<String>,
        granted_at: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> ExecutionApproval {
        let approval =
            ExecutionApproval::grant(token, context_hash, granted_by, granted_at, expires_at);
        self.grant(approval.clone());
        approval
    }

    /// Grants an approval valid for `ttl` from `now`.
    pub fn grant_for_a_while(
        &self,
        token: Uuid,
        context_hash: impl Into<String>,
        granted_by: impl Into<String>,
        now: DateTime<Utc>,
        ttl: Duration,
    ) -> ExecutionApproval {
        self.grant_for(token, context_hash, granted_by, now, now + ttl)
    }

    /// Records an explicit denial for a token, replacing any earlier decision.
    pub fn deny(
        &self,
        token: Uuid,
        context_hash: impl Into<String>,
        granted_by: impl Into<String>,
        now: DateTime<Utc>,
    ) -> ExecutionApproval {
        let approval = ExecutionApproval::deny(token, context_hash, granted_by, now);
        self.grant(approval.clone());
        approval
    }

    /// Consumes the approval for `token` exactly once (ADR-0030 §4).
    ///
    /// Returns the approval and removes it when a granted, unexpired,
    /// hash-matching approval exists; otherwise returns `None` and leaves the
    /// store untouched, so a second call with the same token refuses.
    pub fn consume(
        &self,
        token: Uuid,
        context_hash: &str,
        now: DateTime<Utc>,
    ) -> Option<ExecutionApproval> {
        let mut approvals = self
            .approvals
            .lock()
            .expect("approval store is not poisoned");
        if approvals
            .get(&token)
            .is_some_and(|approval| approval.authorizes(token, context_hash, now))
        {
            approvals.remove(&token)
        } else {
            None
        }
    }

    pub fn get(&self, token: Uuid) -> Option<ExecutionApproval> {
        let approvals = self
            .approvals
            .lock()
            .expect("approval store is not poisoned");
        approvals.get(&token).cloned()
    }

    /// Forgets a token's approval, so it cannot authorize a later attempt.
    pub fn revoke(&self, token: Uuid) -> Option<ExecutionApproval> {
        let mut approvals = self
            .approvals
            .lock()
            .expect("approval store is not poisoned");
        approvals.remove(&token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use argus_domain::ApprovalState;

    const HASH: &str = "hash-a";

    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    #[test]
    fn a_granted_approval_consumes_exactly_its_token_and_hash() {
        let store = ApprovalStore::new();
        let token = Uuid::new_v4();
        let other = Uuid::new_v4();

        store.grant_for_a_while(token, HASH, "root", now(), Duration::minutes(5));

        assert!(store.consume(token, HASH, now()).is_some());
        assert!(
            store.consume(other, HASH, now()).is_none(),
            "an approval must never authorize a different token"
        );
    }

    #[test]
    fn a_hash_mismatch_never_consumes() {
        let store = ApprovalStore::new();
        let token = Uuid::new_v4();

        store.grant_for_a_while(token, HASH, "root", now(), Duration::minutes(5));

        assert!(
            store.consume(token, "hash-b", now()).is_none(),
            "a grant bound to a different context hash must not authorize"
        );
        // The grant is still present, so the correct hash can still consume it.
        assert!(store.consume(token, HASH, now()).is_some());
    }

    #[test]
    fn a_consumed_token_is_replayed_refused() {
        let store = ApprovalStore::new();
        let token = Uuid::new_v4();

        store.grant_for_a_while(token, HASH, "root", now(), Duration::minutes(5));

        assert!(store.consume(token, HASH, now()).is_some());
        assert!(
            store.consume(token, HASH, now()).is_none(),
            "a consumed token must never authorize a second resume"
        );
    }

    #[test]
    fn an_expired_approval_consumes_nothing() {
        let store = ApprovalStore::new();
        let token = Uuid::new_v4();
        let granted_at = now() - Duration::minutes(10);

        store.grant_for(
            token,
            HASH,
            "root",
            granted_at,
            granted_at + Duration::minutes(1),
        );

        assert!(
            store.consume(token, HASH, now()).is_none(),
            "an approval past its expiry must not authorize execution"
        );
    }

    #[test]
    fn an_approval_consumes_within_its_window_only() {
        let store = ApprovalStore::new();
        let token = Uuid::new_v4();
        let granted_at = now();

        store.grant_for(
            token,
            HASH,
            "root",
            granted_at,
            granted_at + Duration::seconds(60),
        );

        assert!(
            store
                .consume(token, HASH, granted_at + Duration::seconds(30))
                .is_some()
        );
    }

    #[test]
    fn the_granting_principal_is_recorded() {
        let store = ApprovalStore::new();
        let token = Uuid::new_v4();

        let approval =
            store.grant_for_a_while(token, HASH, "operator", now(), Duration::minutes(5));

        assert_eq!(approval.granted_by, "operator");
        assert_eq!(approval.state, ApprovalState::Granted);
        assert_eq!(
            store.get(token).map(|a| a.granted_by),
            Some("operator".into())
        );
    }

    #[test]
    fn a_denied_approval_never_consumes() {
        let store = ApprovalStore::new();
        let token = Uuid::new_v4();

        store.deny(token, HASH, "root", now());

        assert!(store.consume(token, HASH, now()).is_none());
    }

    #[test]
    fn revoking_removes_the_authorization() {
        let store = ApprovalStore::new();
        let token = Uuid::new_v4();
        store.grant_for_a_while(token, HASH, "root", now(), Duration::minutes(5));

        assert!(store.revoke(token).is_some());
        assert!(store.consume(token, HASH, now()).is_none());
        assert!(store.get(token).is_none());
    }

    #[test]
    fn regranting_replaces_the_earlier_decision() {
        let store = ApprovalStore::new();
        let token = Uuid::new_v4();

        store.deny(token, HASH, "root", now());
        assert!(store.consume(token, HASH, now()).is_none());

        store.grant_for_a_while(token, HASH, "operator", now(), Duration::minutes(5));
        assert!(store.consume(token, HASH, now()).is_some());
    }
}
