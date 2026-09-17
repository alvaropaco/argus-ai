//! Local event bus and the transport abstraction.

use argus_domain::DomainEvent;
use async_trait::async_trait;
use tokio::sync::broadcast;

use crate::error::EventError;

/// The event transport abstraction.
///
/// Implementations are transport-independent: the local bus uses Tokio
/// channels, and a NATS/JetStream adapter implements the same trait without
/// changing producers (ADR-012).
#[async_trait]
pub trait EventBus: Send + Sync {
    async fn publish(&self, event: &DomainEvent) -> Result<(), EventError>;
}

/// A local, in-process event bus backed by a Tokio broadcast channel.
pub struct LocalEventBus {
    tx: broadcast::Sender<DomainEvent>,
}

impl LocalEventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _rx) = broadcast::channel(capacity);
        Self { tx }
    }

    /// Subscribes to the stream of published events.
    pub fn subscribe(&self) -> broadcast::Receiver<DomainEvent> {
        self.tx.subscribe()
    }
}

#[async_trait]
impl EventBus for LocalEventBus {
    async fn publish(&self, event: &DomainEvent) -> Result<(), EventError> {
        // Publishing with no subscribers is not an error for a local,
        // fire-and-forget bus; the event is simply dropped.
        let _ = self.tx.send(event.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use argus_domain::{EventType, Severity};
    use chrono::Utc;
    use tokio::sync::broadcast::error::TryRecvError;

    use super::*;

    fn event(name: &str) -> DomainEvent {
        DomainEvent::new(
            uuid::Uuid::new_v4(),
            EventType::new(name).unwrap(),
            Utc::now(),
            "argusd",
            "argusd",
            Severity::Info,
            None,
            None,
            serde_json::json!({}),
        )
    }

    #[tokio::test]
    async fn publish_delivers_to_subscriber() {
        let bus = LocalEventBus::new(16);
        let mut rx = bus.subscribe();

        let e = event("argus.ready");
        bus.publish(&e).await.unwrap();

        let received = rx.recv().await.unwrap();
        assert_eq!(received, e);
    }

    #[tokio::test]
    async fn publish_fans_out_to_multiple_subscribers() {
        let bus = LocalEventBus::new(16);
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();

        let e = event("plugin.loaded");
        bus.publish(&e).await.unwrap();

        assert_eq!(rx1.recv().await.unwrap(), e);
        assert_eq!(rx2.recv().await.unwrap(), e);
    }

    #[tokio::test]
    async fn publish_without_subscribers_is_ok() {
        let bus = LocalEventBus::new(16);
        let e = event("argus.degraded");
        assert!(bus.publish(&e).await.is_ok());
    }

    #[tokio::test]
    async fn late_subscriber_does_not_receive_old_events() {
        let bus = LocalEventBus::new(16);
        let mut early = bus.subscribe();
        bus.publish(&event("argus.started")).await.unwrap();
        assert_eq!(
            early.recv().await.unwrap().event_type().as_str(),
            "argus.started"
        );

        // A subscriber that joins afterwards has no backlog to receive.
        let mut late = bus.subscribe();
        assert_eq!(late.try_recv(), Err(TryRecvError::Empty));
    }
}
