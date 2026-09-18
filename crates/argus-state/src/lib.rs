//! Persistence abstraction (`DomainRepository`).
//!
//! Storage is accessed only through the [`DomainRepository`] trait so the
//! domain model never depends on a specific backend (Principle 12).
//!
//! The default backend is [`SqliteRepository`] (interim, see ADR-011 note);
//! [`LanceDbRepository`] is available behind the `lancedb` cargo feature.
//! [`InMemoryRepository`] provides a deterministic backend for tests and
//! degraded operation.

#[cfg(feature = "lancedb")]
mod lancedb;
mod memory;
mod repository;
mod sqlite;

#[cfg(feature = "lancedb")]
pub use lancedb::LanceDbRepository;
pub use memory::InMemoryRepository;
pub use repository::{DomainRepository, RepositoryError};
pub use sqlite::SqliteRepository;
