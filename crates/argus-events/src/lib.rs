//! Transport-independent domain events and the local event bus.
//!
//! The default local transport uses Tokio primitives and does not require
//! NATS. NATS/JetStream is an optional adapter behind the [`EventBus`] trait
//! (ADR-012).

mod bus;
mod error;

pub use bus::{EventBus, LocalEventBus};
pub use error::EventError;
