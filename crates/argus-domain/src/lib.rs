//! ARGUS domain model: canonical entities, identifiers, and value objects.
//!
//! This crate is the architectural center of ARGUS. It MUST NOT depend on
//! infrastructure-specific types (LanceDB, NATS, Kubernetes, or a specific LLM
//! vendor). See `.specify/memory/constitution.md` and `docs/architecture/domain-model.md`.
//!
//! The bootstrap materializes a small, well-defined subset of the full domain
//! model (see `specs/001-bootstrap/data-model.md`).

mod authorization;
mod capability;
mod context;
mod error;
mod event;
mod health;
mod id;
mod observation;
mod validate;

pub use authorization::{
    AuthorizationRequest, BlastRadius, CapabilityRequest, PolicyDecision, PolicyOutcome,
};
pub use capability::{CapabilityDescriptor, Reversibility, RiskClass};
pub use context::{Principal, RequestContext};
pub use error::DomainError;
pub use event::{DomainEvent, EventType, Severity};
pub use health::{HealthState, HealthStatus};
pub use id::{CapabilityId, EnvironmentId, ResourceId};
pub use observation::{Observation, ObservedValue, Provenance};
