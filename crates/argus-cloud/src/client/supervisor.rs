//! Connection supervision: reconnect policy and transport creation.
//!
//! The cloud rate-limits handshakes, so retrying on a fixed interval would turn
//! a fleet-wide cloud restart into a self-inflicted denial of service. The
//! policy here decides *whether* and *how soon* to retry, and it does so per
//! rejection code rather than collapsing every failure into "try again":
//!
//! - revocation stops permanently,
//! - a version mismatch stops and needs a human,
//! - rate limiting waits substantially longer than an ordinary failure.
//!
//! Reconnecting needs a *new* connection, so transport creation sits behind
//! [`TransportFactory`]; that is what lets the whole policy be exercised against
//! a scripted fake with no sockets.

use std::time::Duration;

use async_trait::async_trait;

use crate::client::handshake::{AuthenticatedSession, HandshakeFailure, authenticate};
use crate::state::EnrolledIdentity;
use crate::transport::backoff::Backoff;
use crate::transport::websocket::WssTransport;
use crate::transport::{Transport, TransportError};

/// How a connection attempt ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionOutcome {
    /// A session was established and then ended; reconnecting is routine.
    SessionEnded,
    /// The handshake was refused or failed.
    HandshakeFailed(HandshakeFailure),
    /// The transport failed before or during the session.
    TransportFailed(String),
    /// A human asked the installation to stop.
    Stopped,
}

/// Why supervision stopped for good.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    /// The instance was revoked. Terminal until re-enrollment.
    Revoked,
    /// No mutually supported protocol version.
    UnsupportedVersion,
    /// A human asked to stop.
    Requested,
    /// The installation is not enrolled.
    NotEnrolled,
}

/// What supervision should do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NextAction {
    Reconnect(Duration),
    Stop(StopReason),
}

/// Decides the next action after each connection attempt.
#[derive(Debug, Clone)]
pub struct ReconnectPolicy {
    backoff: Backoff,
    /// Extra delay applied when the cloud asks us to slow down.
    rate_limit_floor: Duration,
}

impl ReconnectPolicy {
    pub fn new(base: Duration, max: Duration) -> Self {
        Self {
            backoff: Backoff::new(base, max),
            rate_limit_floor: Duration::from_secs(60),
        }
    }

    /// Deterministic construction for tests.
    pub fn with_seed(base: Duration, max: Duration, seed: u64) -> Self {
        Self {
            backoff: Backoff::with_seed(base, max, seed),
            rate_limit_floor: Duration::from_secs(60),
        }
    }

    /// Attempts made since the last successful session.
    pub fn attempt(&self) -> u32 {
        self.backoff.attempt()
    }

    /// A successful session resets the schedule, so an installation that flaps
    /// once does not reconnect slowly for the rest of its life.
    pub fn note_connected(&mut self) {
        self.backoff.reset();
    }

    /// Classifies an outcome into the next action.
    pub fn after(&mut self, outcome: ConnectionOutcome) -> NextAction {
        match outcome {
            ConnectionOutcome::Stopped => NextAction::Stop(StopReason::Requested),
            ConnectionOutcome::SessionEnded => NextAction::Reconnect(self.backoff.next_delay()),
            ConnectionOutcome::TransportFailed(_) => {
                NextAction::Reconnect(self.backoff.next_delay())
            }
            ConnectionOutcome::HandshakeFailed(failure) => {
                if failure.is_terminal() {
                    return NextAction::Stop(StopReason::Revoked);
                }
                if matches!(failure, HandshakeFailure::UnsupportedVersion { .. }) {
                    return NextAction::Stop(StopReason::UnsupportedVersion);
                }
                if matches!(
                    failure,
                    HandshakeFailure::Rejected { ref code, .. } if code == "UNSUPPORTED_VERSION"
                ) {
                    return NextAction::Stop(StopReason::UnsupportedVersion);
                }

                let delay = self.backoff.next_delay();
                if failure.is_rate_limited() {
                    return NextAction::Reconnect(delay.max(self.rate_limit_floor));
                }
                NextAction::Reconnect(delay)
            }
        }
    }
}

/// Everything one connection cycle needs.
pub struct SessionRequest<'a> {
    pub endpoint: &'a str,
    pub expected_cloud_id: &'a str,
    pub identity: &'a EnrolledIdentity,
    pub session_proof: &'a str,
    pub hostname: &'a str,
    pub agent_version: &'a str,
    pub capability_schema_version: Option<&'a str>,
}

/// Performs one connection cycle: connect, then authenticate.
///
/// On success the caller owns an established session and the transport it runs
/// on. On failure the outcome is classified so [`ReconnectPolicy`] can decide
/// whether and when to try again. The loop itself belongs to the caller, which
/// keeps this function free of timers and therefore directly testable.
pub async fn connect_once(
    factory: &dyn TransportFactory,
    request: SessionRequest<'_>,
) -> Result<(Box<dyn Transport>, AuthenticatedSession), ConnectionOutcome> {
    let mut transport = factory
        .connect(request.endpoint)
        .await
        .map_err(|error| ConnectionOutcome::TransportFailed(error.to_string()))?;

    let session = authenticate(
        transport.as_mut(),
        request.expected_cloud_id,
        request.identity,
        request.session_proof,
        request.hostname,
        request.agent_version,
        request.capability_schema_version,
    )
    .await
    .map_err(ConnectionOutcome::HandshakeFailed)?;

    Ok((transport, session))
}

/// Creates new connections, which is what makes reconnection possible.
#[async_trait]
pub trait TransportFactory: Send + Sync {
    async fn connect(&self, endpoint: &str) -> Result<Box<dyn Transport>, TransportError>;
}

/// Produces real WebSocket connections.
#[derive(Debug, Default, Clone, Copy)]
pub struct WssFactory;

#[async_trait]
impl TransportFactory for WssFactory {
    async fn connect(&self, endpoint: &str) -> Result<Box<dyn Transport>, TransportError> {
        Ok(Box::new(WssTransport::connect(endpoint).await?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: Duration = Duration::from_secs(1);
    const MAX: Duration = Duration::from_secs(300);

    fn policy() -> ReconnectPolicy {
        ReconnectPolicy::with_seed(BASE, MAX, 7)
    }

    fn rejected(code: &str) -> ConnectionOutcome {
        ConnectionOutcome::HandshakeFailed(HandshakeFailure::Rejected {
            code: code.into(),
            message: String::new(),
        })
    }

    #[test]
    fn a_revoked_instance_stops_reconnecting() {
        let action = policy().after(rejected("REVOKED"));
        assert_eq!(action, NextAction::Stop(StopReason::Revoked));
    }

    #[test]
    fn a_version_mismatch_stops_and_needs_a_human() {
        let action = policy().after(rejected("UNSUPPORTED_VERSION"));
        assert_eq!(action, NextAction::Stop(StopReason::UnsupportedVersion));

        let local = policy().after(ConnectionOutcome::HandshakeFailed(
            HandshakeFailure::UnsupportedVersion {
                offered: vec!["9.0.0".into()],
            },
        ));
        assert_eq!(local, NextAction::Stop(StopReason::UnsupportedVersion));
    }

    #[test]
    fn a_suspension_reconnects_rather_than_giving_up() {
        let action = policy().after(rejected("SUSPENDED"));
        assert!(
            matches!(action, NextAction::Reconnect(_)),
            "suspension must not strand an installation: {action:?}"
        );
    }

    #[test]
    fn rate_limiting_waits_substantially_longer() {
        let mut policy = policy();
        let limited = policy.after(rejected("RATE_LIMITED"));

        match limited {
            NextAction::Reconnect(delay) => assert!(
                delay >= Duration::from_secs(60),
                "a rate limit must not be answered at the ordinary cadence: {delay:?}"
            ),
            other => panic!("expected Reconnect, got {other:?}"),
        }
    }

    #[test]
    fn ordinary_failures_back_off_within_the_cap() {
        let mut policy = policy();
        for _ in 0..25 {
            match policy.after(ConnectionOutcome::TransportFailed("reset".into())) {
                NextAction::Reconnect(delay) => {
                    assert!(delay <= MAX, "delay {delay:?} exceeded cap")
                }
                other => panic!("expected Reconnect, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_session_ending_is_routine() {
        assert!(matches!(
            policy().after(ConnectionOutcome::SessionEnded),
            NextAction::Reconnect(_)
        ));
    }

    #[test]
    fn a_stop_request_is_honoured() {
        assert_eq!(
            policy().after(ConnectionOutcome::Stopped),
            NextAction::Stop(StopReason::Requested)
        );
    }

    #[test]
    fn a_successful_session_resets_the_backoff() {
        let mut policy = policy();
        for _ in 0..6 {
            policy.after(ConnectionOutcome::TransportFailed("reset".into()));
        }
        assert_eq!(policy.attempt(), 6);

        policy.note_connected();
        assert_eq!(policy.attempt(), 0);
    }

    #[test]
    fn attempts_accumulate_across_failures_so_the_schedule_grows() {
        let mut policy = policy();
        let first = policy.after(ConnectionOutcome::TransportFailed("a".into()));
        let second = policy.after(ConnectionOutcome::TransportFailed("b".into()));
        assert_eq!(policy.attempt(), 2);
        assert!(matches!(first, NextAction::Reconnect(_)));
        assert!(matches!(second, NextAction::Reconnect(_)));
    }
}
