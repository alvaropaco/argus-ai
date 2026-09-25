//! Cloud connectivity state machine and last-exchange tracking.
//!
//! The installation's own view of its cloud relationship. Only `revoked` is
//! terminal; suspension is recoverable and must never be treated as terminal
//! (FR-041, FR-042).

use chrono::{DateTime, Utc};
use uuid::Uuid;

use argus_domain::{CloudConnectivityState, CloudEnrollment, TrustState};

use crate::protocol::messages::PairingGrantedPayload;

/// The durable identity an installation holds after enrollment.
///
/// Carries no secret: the private seed lives in the secret store, and only a
/// handle to it belongs here. `public_key` is the base64url DER public key sent
/// to the cloud and is non-secret by construction (ADR-0024).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrolledIdentity {
    pub installation_id: Uuid,
    pub tenant_id: Uuid,
    pub instance_name: String,
    /// Rotation generation. Advances on `session.rotate` so a credential issued
    /// before the rotation can be rejected.
    pub credential_epoch: u64,
    pub public_key: String,
    pub protocol_version: String,
    pub enrolled_at: DateTime<Utc>,
}

impl EnrolledIdentity {
    /// Adopts the identity the cloud granted at enrollment.
    pub fn from_granted(
        payload: &PairingGrantedPayload,
        at: DateTime<Utc>,
        public_key: String,
    ) -> Self {
        Self {
            installation_id: payload.instance_id,
            tenant_id: payload.tenant_id,
            instance_name: payload.instance_name.clone(),
            credential_epoch: 0,
            public_key,
            protocol_version: payload.negotiated_protocol_version.clone(),
            enrolled_at: at,
        }
    }

    /// Rebuilds the identity from the persisted enrollment.
    ///
    /// The seed itself is not part of the identity: it lives in the secret
    /// store and is supplied separately wherever a signature is needed.
    pub fn from_enrollment(
        enrollment: &CloudEnrollment,
        protocol_version: &str,
        public_key: String,
    ) -> Self {
        Self {
            installation_id: enrollment.installation_id,
            tenant_id: enrollment.tenant_id,
            instance_name: enrollment.instance_name.clone(),
            credential_epoch: 0,
            public_key,
            protocol_version: protocol_version.to_string(),
            enrolled_at: enrollment.enrolled_at,
        }
    }

    pub fn to_enrollment(&self) -> CloudEnrollment {
        let mut enrollment = CloudEnrollment::new(
            self.installation_id,
            self.tenant_id,
            self.instance_name.clone(),
            self.enrolled_at,
        );
        enrollment.enrolled_by = None;
        enrollment
    }

    pub fn trust_state(&self, at: DateTime<Utc>) -> TrustState {
        let mut trust = TrustState::active(self.enrolled_at);
        trust.credential_epoch = self.credential_epoch;
        trust.last_change_at = at;
        trust
    }

    /// The identity after a credential rotation, with the epoch advanced.
    pub fn rotated(&self) -> Self {
        Self {
            credential_epoch: self.credential_epoch.saturating_add(1),
            ..self.clone()
        }
    }
}

/// Tracks the installation's cloud relationship as it changes.
///
/// The value set and its terminal rules live in `argus-domain`; this tracks the
/// live instance plus the evidence an operator needs: when the last exchange
/// succeeded, what went wrong, and what to do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectivityTracker {
    state: CloudConnectivityState,
    last_exchange_at: Option<DateTime<Utc>>,
    last_error: Option<String>,
}

impl Default for ConnectivityTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl ConnectivityTracker {
    pub fn new() -> Self {
        Self {
            state: CloudConnectivityState::NotConfigured,
            last_exchange_at: None,
            last_error: None,
        }
    }

    pub fn state(&self) -> CloudConnectivityState {
        self.state
    }

    pub fn last_exchange_at(&self) -> Option<DateTime<Utc>> {
        self.last_exchange_at
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Moves to a new state, dropping a stale error once the relationship
    /// recovers so an operator is not shown a failure that no longer applies.
    pub fn transition(&mut self, state: CloudConnectivityState) {
        self.state = state;
        if !matches!(
            state,
            CloudConnectivityState::Disconnected | CloudConnectivityState::Degraded
        ) {
            self.last_error = None;
        }
    }

    /// Records a successful exchange, which is what "last seen" means to an
    /// operator and what makes staleness measurable.
    pub fn record_exchange(&mut self, at: DateTime<Utc>) {
        self.last_exchange_at = Some(at);
        self.last_error = None;
    }

    pub fn record_error(&mut self, error: impl Into<String>) {
        self.last_error = Some(error.into());
    }

    pub fn allows_reconnect(&self) -> bool {
        self.state.allows_reconnect()
    }

    /// What the operator should do next, or `None` when nothing needs doing.
    pub fn remediation(&self) -> Option<&'static str> {
        match self.state {
            CloudConnectivityState::NotConfigured => {
                Some("run `argus cloud enroll` to connect this installation")
            }
            CloudConnectivityState::Revoked => {
                Some("this installation was revoked; re-enrollment is required")
            }
            CloudConnectivityState::Suspended => {
                Some("this installation is suspended; no action is needed until it is resumed")
            }
            CloudConnectivityState::Disconnected => {
                Some("check network reachability and the configured cloud endpoint")
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::identity::InstallationKey;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 21, 12, 0, 0).unwrap()
    }

    fn granted(instance_name: &str) -> PairingGrantedPayload {
        PairingGrantedPayload {
            instance_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            instance_name: instance_name.to_string(),
            session_token: "credential-that-must-not-be-persisted-here".to_string(),
            session_expires_at: now() + chrono::Duration::hours(1),
            negotiated_protocol_version: "1.0.0".to_string(),
        }
    }

    fn public_key() -> String {
        InstallationKey::from_seed_bytes(&[7u8; 32]).public_key_b64()
    }

    #[test]
    fn identity_carries_the_granted_binding() {
        let payload = granted("web-01");
        let identity = EnrolledIdentity::from_granted(&payload, now(), public_key());

        assert_eq!(identity.installation_id, payload.instance_id);
        assert_eq!(identity.tenant_id, payload.tenant_id);
        assert_eq!(identity.instance_name, "web-01");
        assert_eq!(identity.protocol_version, "1.0.0");
        assert_eq!(identity.credential_epoch, 0);
        assert_eq!(identity.enrolled_at, now());
    }

    #[test]
    fn identity_holds_no_secret() {
        let identity = EnrolledIdentity::from_granted(&granted("web-01"), now(), public_key());
        let rendered = format!("{identity:?}");
        assert!(
            !rendered.contains("credential-that-must-not-be-persisted-here"),
            "the credential must not travel with the identity: {rendered}"
        );
    }

    #[test]
    fn the_identity_carries_the_public_key_it_was_built_with() {
        let identity = EnrolledIdentity::from_granted(&granted("web-01"), now(), public_key());
        assert_eq!(identity.public_key, public_key());
    }

    #[test]
    fn the_enrollment_record_stays_bound_to_one_organization() {
        let payload = granted("web-01");
        let identity = EnrolledIdentity::from_granted(&payload, now(), public_key());
        let enrollment = identity.to_enrollment();

        assert_eq!(enrollment.installation_id, payload.instance_id);
        assert_eq!(enrollment.tenant_id, payload.tenant_id);
        assert_eq!(enrollment.instance_name, "web-01");
        assert_eq!(enrollment.enrolled_at, now());
    }

    #[test]
    fn rotation_advances_the_credential_epoch() {
        let identity = EnrolledIdentity::from_granted(&granted("web-01"), now(), public_key());
        let rotated = identity.rotated();
        assert_eq!(rotated.credential_epoch, 1);
        assert_eq!(rotated.installation_id, identity.installation_id);
        assert_eq!(rotated.rotated().credential_epoch, 2);
    }

    #[test]
    fn trust_state_starts_active_with_the_current_epoch() {
        let identity =
            EnrolledIdentity::from_granted(&granted("web-01"), now(), public_key()).rotated();
        let trust = identity.trust_state(now());
        assert!(trust.status.permits_connection());
        assert_eq!(trust.credential_epoch, 1);
    }

    #[test]
    fn a_fresh_tracker_is_not_configured_with_nothing_recorded() {
        let tracker = ConnectivityTracker::new();
        assert_eq!(tracker.state(), CloudConnectivityState::NotConfigured);
        assert_eq!(tracker.last_exchange_at(), None);
        assert_eq!(tracker.last_error(), None);
    }

    #[test]
    fn a_successful_exchange_records_the_time_and_clears_the_error() {
        let mut tracker = ConnectivityTracker::new();
        tracker.record_error("previous failure");
        tracker.transition(CloudConnectivityState::Disconnected);

        tracker.record_exchange(now());

        assert_eq!(tracker.last_exchange_at(), Some(now()));
        assert_eq!(
            tracker.last_error(),
            None,
            "a recovered link shows no error"
        );
    }

    #[test]
    fn recovering_to_connected_drops_a_stale_error() {
        let mut tracker = ConnectivityTracker::new();
        tracker.transition(CloudConnectivityState::Disconnected);
        tracker.record_error("connection reset");

        tracker.transition(CloudConnectivityState::Connected);

        assert_eq!(tracker.state(), CloudConnectivityState::Connected);
        assert_eq!(
            tracker.last_error(),
            None,
            "an operator must not be shown a failure that no longer applies"
        );
    }

    #[test]
    fn staying_disconnected_keeps_the_reason_visible() {
        let mut tracker = ConnectivityTracker::new();
        tracker.transition(CloudConnectivityState::Disconnected);
        tracker.record_error("endpoint unreachable");

        tracker.transition(CloudConnectivityState::Disconnected);

        assert_eq!(tracker.last_error(), Some("endpoint unreachable"));
    }

    #[test]
    fn a_degraded_link_keeps_its_error_but_still_counts_as_online() {
        let mut tracker = ConnectivityTracker::new();
        tracker.transition(CloudConnectivityState::Degraded);
        tracker.record_error("reporting throttled");

        assert_eq!(tracker.last_error(), Some("reporting throttled"));
        assert!(tracker.state().is_online());
    }

    #[test]
    fn a_revoked_installation_stops_reconnecting() {
        let mut tracker = ConnectivityTracker::new();
        tracker.transition(CloudConnectivityState::Connected);
        assert!(tracker.allows_reconnect());

        tracker.transition(CloudConnectivityState::Revoked);
        assert!(!tracker.allows_reconnect());
        assert!(tracker.state().is_terminal());
    }

    #[test]
    fn a_suspended_installation_keeps_retrying() {
        let mut tracker = ConnectivityTracker::new();
        tracker.transition(CloudConnectivityState::Suspended);

        assert!(tracker.allows_reconnect(), "suspension is recoverable");
        assert!(!tracker.state().is_terminal());
    }

    #[test]
    fn every_actionable_state_offers_the_operator_a_next_step() {
        let cases = [
            (CloudConnectivityState::NotConfigured, "enroll"),
            (CloudConnectivityState::Revoked, "re-enrollment"),
            (CloudConnectivityState::Suspended, "resumed"),
            (CloudConnectivityState::Disconnected, "reachability"),
        ];
        for (state, expected) in cases {
            let mut tracker = ConnectivityTracker::new();
            tracker.transition(state);
            let remediation = tracker.remediation().unwrap_or_else(|| {
                panic!("{state:?} requires human action and must offer guidance")
            });
            assert!(
                remediation.contains(expected),
                "{state:?}: expected guidance mentioning '{expected}', got '{remediation}'"
            );
        }
    }

    #[test]
    fn a_healthy_installation_is_offered_no_action() {
        for state in [
            CloudConnectivityState::Connected,
            CloudConnectivityState::Connecting,
            CloudConnectivityState::Degraded,
        ] {
            let mut tracker = ConnectivityTracker::new();
            tracker.transition(state);
            assert_eq!(tracker.remediation(), None, "{state:?}");
        }
    }
}
