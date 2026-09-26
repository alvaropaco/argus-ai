//! Decision-provider adapters.

pub mod fake;
pub mod jev;
pub mod laya;

pub use fake::FakeDecisionProvider;
pub use jev::JevHttpProvider;
pub use laya::LayaHttpProvider;
