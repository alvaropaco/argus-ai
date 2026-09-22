//! Scripted fake transport.
//!
//! Drives the cloud client's lifecycle with no sockets, so every branch of
//! `contracts/agent-protocol-conformance.md` §7 — handshake variants, each
//! pairing denial, revocation, suspension, rotation, throttle, malformed frames,
//! and disconnects at arbitrary points — is exercisable in CI. This is what makes
//! SC-005, SC-016, and SC-017 verifiable without a running cloud.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::client::supervisor::TransportFactory;
use crate::protocol::{Envelope, MessageType};
use crate::transport::{CloseReason, Transport, TransportError};

/// A transport whose inbound frames are scripted and whose sends are recorded.
pub struct FakeTransport {
    inbound: VecDeque<Envelope>,
    sent: Arc<Mutex<Vec<Envelope>>>,
    closed: Mutex<Option<CloseReason>>,
    fail_send_after: Option<usize>,
    malformed_frames: VecDeque<String>,
}

impl Default for FakeTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeTransport {
    /// An empty transport with nothing scripted.
    pub fn new() -> Self {
        Self {
            inbound: VecDeque::new(),
            sent: Arc::new(Mutex::new(Vec::new())),
            closed: Mutex::new(None),
            fail_send_after: None,
            malformed_frames: VecDeque::new(),
        }
    }

    /// A transport preloaded with inbound frames, in delivery order.
    pub fn with_inbound(frames: Vec<Envelope>) -> Self {
        Self {
            inbound: frames.into(),
            ..Self::new()
        }
    }

    /// Lets a factory observe every connection's traffic, which is how a
    /// reconnection is checked for duplicated frames.
    pub fn with_shared_log(frames: Vec<Envelope>, log: Arc<Mutex<Vec<Envelope>>>) -> Self {
        Self {
            inbound: frames.into(),
            sent: log,
            closed: Mutex::new(None),
            fail_send_after: None,
            malformed_frames: VecDeque::new(),
        }
    }

    /// Queues one inbound frame.
    pub fn push_inbound(&mut self, frame: Envelope) {
        self.inbound.push_back(frame);
    }

    /// Queues a raw frame that is not a valid envelope, to exercise `MALFORMED`.
    pub fn push_malformed_frame(&mut self, raw: impl Into<String>) {
        self.malformed_frames.push_back(raw.into());
    }

    /// Makes `send` fail once `n` sends have succeeded.
    pub fn fail_send_after(&mut self, n: usize) {
        self.fail_send_after = Some(n);
    }

    /// Everything the client has sent, in order.
    pub fn sent(&self) -> Vec<Envelope> {
        self.sent.lock().expect("sent lock").clone()
    }

    /// Sent envelopes of one type.
    pub fn sent_of_type(&self, ty: MessageType) -> Vec<Envelope> {
        self.sent()
            .into_iter()
            .filter(|e| e.kind() == Some(ty))
            .collect()
    }

    /// Whether the last send of `ty` happened, and how many times.
    pub fn count_sent(&self, ty: MessageType) -> usize {
        self.sent_of_type(ty).len()
    }

    /// The reason the connection was closed, if it was.
    pub fn close_reason(&self) -> Option<CloseReason> {
        *self.closed.lock().expect("closed lock")
    }

    /// Whether `close` has been called.
    pub fn is_closed(&self) -> bool {
        self.close_reason().is_some()
    }

    /// Frames still waiting to be delivered.
    pub fn pending_inbound(&self) -> usize {
        self.inbound.len()
    }
}

#[async_trait]
impl Transport for FakeTransport {
    async fn send(&self, envelope: &Envelope) -> Result<(), TransportError> {
        let mut sent = self.sent.lock().expect("sent lock");
        if let Some(limit) = self.fail_send_after
            && sent.len() >= limit
        {
            return Err(TransportError::Send("scripted send failure".into()));
        }
        sent.push(envelope.clone());
        Ok(())
    }

    async fn recv(&mut self) -> Result<Option<Envelope>, TransportError> {
        if let Some(raw) = self.malformed_frames.pop_front() {
            return Err(TransportError::Malformed(raw));
        }
        Ok(self.inbound.pop_front())
    }

    async fn close(&mut self, reason: CloseReason) {
        *self.closed.lock().expect("closed lock") = Some(reason);
    }
}

/// Scripts a sequence of connections, one per `connect` call.
///
/// Every connection shares one send log, so a test can assert what crossed the
/// wire across a whole reconnect cycle rather than per connection.
#[derive(Default)]
pub struct FakeFactory {
    script: Mutex<VecDeque<Vec<Envelope>>>,
    failures: Mutex<VecDeque<String>>,
    log: Arc<Mutex<Vec<Envelope>>>,
    connects: AtomicUsize,
}

impl FakeFactory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queues the inbound frames the next connection will deliver.
    pub fn script_connection(&self, frames: Vec<Envelope>) {
        self.script.lock().expect("script lock").push_back(frames);
    }

    /// Makes the next `connect` call fail.
    pub fn fail_next_connection(&self, reason: impl Into<String>) {
        self.failures
            .lock()
            .expect("failures lock")
            .push_back(reason.into());
    }

    /// How many `connect` calls were attempted, including failed ones.
    pub fn connect_attempts(&self) -> usize {
        self.connects.load(Ordering::SeqCst)
    }

    /// Everything sent across every connection, in order.
    pub fn sent(&self) -> Vec<Envelope> {
        self.log.lock().expect("log lock").clone()
    }

    /// How many frames of one type crossed any connection.
    pub fn count_sent(&self, ty: MessageType) -> usize {
        self.sent()
            .into_iter()
            .filter(|envelope| envelope.kind() == Some(ty))
            .count()
    }
}

#[async_trait]
impl TransportFactory for FakeFactory {
    async fn connect(&self, _endpoint: &str) -> Result<Box<dyn Transport>, TransportError> {
        self.connects.fetch_add(1, Ordering::SeqCst);

        if let Some(reason) = self.failures.lock().expect("failures lock").pop_front() {
            return Err(TransportError::Connect(reason));
        }

        let frames = self
            .script
            .lock()
            .expect("script lock")
            .pop_front()
            .unwrap_or_default();

        Ok(Box::new(FakeTransport::with_shared_log(
            frames,
            Arc::clone(&self.log),
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;
    use uuid::Uuid;

    fn frame(ty: MessageType) -> Envelope {
        Envelope::new(ty, json!({}), None)
    }

    #[tokio::test]
    async fn records_sends_in_order() {
        let t = FakeTransport::new();
        t.send(&frame(MessageType::Ping)).await.unwrap();
        t.send(&frame(MessageType::Pong)).await.unwrap();

        assert_eq!(t.count_sent(MessageType::Ping), 1);
        assert_eq!(t.count_sent(MessageType::Pong), 1);
        assert_eq!(t.sent().len(), 2);
        assert_eq!(t.sent()[0].kind(), Some(MessageType::Ping));
    }

    #[tokio::test]
    async fn delivers_scripted_inbound_then_reports_close() {
        let mut t = FakeTransport::with_inbound(vec![frame(MessageType::HandshakeHello)]);
        assert_eq!(
            t.recv().await.unwrap().map(|e| e.kind()),
            Some(Some(MessageType::HandshakeHello))
        );
        // Stream exhausted: a clean end, not an error.
        assert!(t.recv().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn surfaces_malformed_frames() {
        let mut t = FakeTransport::new();
        t.push_malformed_frame("{not json");
        let err = t.recv().await.unwrap_err();
        assert!(matches!(err, TransportError::Malformed(_)));
    }

    #[tokio::test]
    async fn honours_scripted_send_failure() {
        let mut t = FakeTransport::new();
        t.fail_send_after(1);
        t.send(&frame(MessageType::Ping)).await.unwrap();
        assert!(t.send(&frame(MessageType::Ping)).await.is_err());
        assert_eq!(t.count_sent(MessageType::Ping), 1);
    }

    #[tokio::test]
    async fn records_close_reason() {
        let mut t = FakeTransport::new();
        assert!(!t.is_closed());
        t.close(CloseReason::Revoked).await;
        assert!(t.is_closed());
        assert_eq!(t.close_reason(), Some(CloseReason::Revoked));
    }

    #[tokio::test]
    async fn can_replay_the_reference_handshake_sequence() {
        // Mirrors the delivered cloud integration test's flow.
        let mut t = FakeTransport::with_inbound(vec![
            Envelope::new(
                MessageType::HandshakeHello,
                json!({
                    "cloud_id": "argus-cloud",
                    "cloud_instance": "gateway-7",
                    "server_time": Utc::now().to_rfc3339(),
                    "supported_protocol_versions": ["1.0.0"],
                    "challenge": "nonce",
                    "heartbeat_interval_seconds": 20
                }),
                None,
            ),
            Envelope::new(
                MessageType::HandshakeReady,
                json!({
                    "negotiated_protocol_version": "1.0.0",
                    "instance_id": Uuid::new_v4().to_string(),
                    "tenant_id": Uuid::new_v4().to_string(),
                    "config_pull_required": true
                }),
                None,
            ),
        ]);

        let hello = t.recv().await.unwrap().unwrap();
        assert_eq!(hello.kind(), Some(MessageType::HandshakeHello));
        let ready = t.recv().await.unwrap().unwrap();
        assert_eq!(ready.kind(), Some(MessageType::HandshakeReady));
        assert_eq!(t.pending_inbound(), 0);
    }
}
