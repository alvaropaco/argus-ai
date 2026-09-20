//! Decision-provider adapters.

pub mod fake;
pub mod jev;

pub use fake::FakeDecisionProvider;
pub use jev::JevHttpProvider;
