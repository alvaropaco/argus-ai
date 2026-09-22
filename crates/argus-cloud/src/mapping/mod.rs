//! Pure projections between local domain types and the cloud wire vocabulary.
//!
//! Translation is explicit and tested; nothing is coerced silently. The
//! authoritative mappings are fixed in ADR-0016 and repeated in
//! `specs/001-argus-cloud-sync/contracts/agent-protocol-conformance.md` §5.

pub mod activity;
pub mod capability;
pub mod event;
pub mod health;
pub mod telemetry;
