//! WebSocket-over-TLS transport.
//!
//! The only networked implementation of [`Transport`]. Two things are load
//! bearing here: the connection is installation-initiated (no inbound
//! reachability is required), and the cloud's advertised identity is verified
//! before any telemetry or configuration is exchanged (FR-009).

use std::sync::Arc;

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::protocol::{Envelope, MessageType};
use crate::transport::{CloseReason, Transport, TransportError};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// A live connection to the cloud agent endpoint.
pub struct WssTransport {
    sink: Arc<Mutex<futures_util::stream::SplitSink<Socket, Message>>>,
    stream: futures_util::stream::SplitStream<Socket>,
    closed: Option<CloseReason>,
}

impl WssTransport {
    /// Connects to `endpoint` and returns the transport.
    ///
    /// Identity verification happens in [`Self::recv`], where the cloud's
    /// `handshake.hello` is inspected against `expected_cloud_id`, because the
    /// cloud announces its identity as the first frame rather than at the TLS
    /// layer.
    pub async fn connect(endpoint: &str) -> Result<Self, TransportError> {
        let (socket, _response) = tokio_tungstenite::connect_async(endpoint)
            .await
            .map_err(|e| TransportError::Connect(e.to_string()))?;
        let (sink, stream) = socket.split();
        Ok(Self {
            sink: Arc::new(Mutex::new(sink)),
            stream,
            closed: None,
        })
    }

    /// The cloud identity announced by this frame, if any.
    pub fn announced_cloud_id(frame: &Envelope) -> Option<String> {
        if frame.kind() != Some(MessageType::HandshakeHello) {
            return None;
        }
        frame
            .payload
            .get("cloud_id")
            .and_then(|v| v.as_str())
            .map(str::to_owned)
    }

    /// Verifies the cloud's announced identity against what we expect.
    ///
    /// A mismatch terminates the session rather than continuing, because sending
    /// data to an unexpected peer is the failure this check exists to prevent.
    pub fn verify_cloud_identity(
        expected: &str,
        announced: Option<&str>,
    ) -> Result<(), TransportError> {
        match announced {
            Some(actual) if actual == expected => Ok(()),
            Some(actual) => Err(TransportError::CloudIdentityMismatch {
                expected: expected.to_string(),
                actual: actual.to_string(),
            }),
            None => Err(TransportError::Malformed(
                "handshake.hello did not announce a cloud_id".to_string(),
            )),
        }
    }
}

#[async_trait]
impl Transport for WssTransport {
    async fn send(&self, envelope: &Envelope) -> Result<(), TransportError> {
        let text =
            serde_json::to_string(envelope).map_err(|e| TransportError::Send(e.to_string()))?;
        let mut sink = self.sink.lock().await;
        sink.send(Message::Text(text.into()))
            .await
            .map_err(|e| TransportError::Send(e.to_string()))
    }

    async fn recv(&mut self) -> Result<Option<Envelope>, TransportError> {
        loop {
            let Some(message) = self.stream.next().await else {
                return Ok(None);
            };
            let message = message.map_err(|e| TransportError::Receive(e.to_string()))?;
            match message {
                Message::Text(text) => {
                    let envelope: Envelope = serde_json::from_str(&text)
                        .map_err(|e| TransportError::Malformed(e.to_string()))?;
                    return Ok(Some(envelope));
                }
                Message::Binary(bytes) => {
                    let envelope: Envelope = serde_json::from_slice(&bytes)
                        .map_err(|e| TransportError::Malformed(e.to_string()))?;
                    return Ok(Some(envelope));
                }
                Message::Close(_) => return Ok(None),
                // Ping/Pong are handled by tungstenite; other frames are ignored.
                _ => continue,
            }
        }
    }

    async fn close(&mut self, reason: CloseReason) {
        self.closed = Some(reason);
        let mut sink = self.sink.lock().await;
        let _ = sink.close().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;

    fn hello(cloud_id: serde_json::Value) -> Envelope {
        Envelope::new(
            MessageType::HandshakeHello,
            json!({
                "cloud_id": cloud_id,
                "cloud_instance": "gateway-7",
                "server_time": Utc::now().to_rfc3339(),
                "supported_protocol_versions": ["1.0.0"],
                "challenge": "nonce",
                "heartbeat_interval_seconds": 20
            }),
            None,
        )
    }

    #[test]
    fn accepts_the_expected_cloud() {
        let frame = hello(json!("argus-cloud"));
        let announced = WssTransport::announced_cloud_id(&frame);
        assert_eq!(announced.as_deref(), Some("argus-cloud"));
        assert!(WssTransport::verify_cloud_identity("argus-cloud", announced.as_deref()).is_ok());
    }

    #[test]
    fn refuses_an_unexpected_cloud() {
        let announced = Some("someone-else");
        let err = WssTransport::verify_cloud_identity("argus-cloud", announced).unwrap_err();
        assert!(matches!(err, TransportError::CloudIdentityMismatch { .. }));
    }

    #[test]
    fn refuses_a_hello_without_an_identity() {
        let err = WssTransport::verify_cloud_identity("argus-cloud", None).unwrap_err();
        assert!(matches!(err, TransportError::Malformed(_)));
    }

    #[test]
    fn ignores_non_hello_frames_when_reading_identity() {
        let frame = Envelope::new(MessageType::Ping, json!({}), None);
        assert!(WssTransport::announced_cloud_id(&frame).is_none());
    }
}
