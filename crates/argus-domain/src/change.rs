//! Changes: recorded environment mutations for change intelligence
//! (CAP-18, FR-013).
//!
//! A [`Change`] is an immutable record of one observed mutation — a commit, a
//! deployment, an image swap, a config edit, a Terraform apply. The ledger and
//! the "what changed before the incident" query live in `argus-correlate`;
//! this type is the pure data shape (data-model §5).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::DomainError;
use crate::id::ResourceId;

/// The surface a change was observed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeSource {
    Git,
    Deploy,
    Image,
    Config,
    K8s,
    Terraform,
    Cloud,
    Package,
    Kernel,
    ServiceConfig,
}

/// One recorded environment change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    id: Uuid,
    source: ChangeSource,
    subject: ResourceId,
    before: Option<String>,
    after: Option<String>,
    actor: Option<String>,
    changed_at: DateTime<Utc>,
}

impl Change {
    /// `before`/`after`/`actor`, when present, must be non-empty: an empty
    /// string is not a value and would corrupt the evidence prose.
    pub fn new(
        id: Uuid,
        source: ChangeSource,
        subject: ResourceId,
        before: Option<String>,
        after: Option<String>,
        actor: Option<String>,
        changed_at: DateTime<Utc>,
    ) -> Result<Self, DomainError> {
        for (field, value) in [("before", &before), ("after", &after), ("actor", &actor)] {
            if value.as_ref().is_some_and(String::is_empty) {
                return Err(DomainError::InvalidChange(format!(
                    "{field} must be non-empty when present"
                )));
            }
        }
        Ok(Self {
            id,
            source,
            subject,
            before,
            after,
            actor,
            changed_at,
        })
    }

    pub fn id(&self) -> Uuid {
        self.id
    }

    pub fn source(&self) -> ChangeSource {
        self.source
    }

    pub fn subject(&self) -> &ResourceId {
        &self.subject
    }

    pub fn before(&self) -> Option<&str> {
        self.before.as_deref()
    }

    pub fn after(&self) -> Option<&str> {
        self.after.as_deref()
    }

    pub fn actor(&self) -> Option<&str> {
        self.actor.as_deref()
    }

    pub fn changed_at(&self) -> DateTime<Utc> {
        self.changed_at
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn ts() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 5, 12, 0, 0).unwrap()
    }

    fn change(before: Option<String>, after: Option<String>) -> Result<Change, DomainError> {
        Change::new(
            Uuid::new_v4(),
            ChangeSource::Git,
            ResourceId::new("service", "api").unwrap(),
            before,
            after,
            Some("alice".to_string()),
            ts(),
        )
    }

    #[test]
    fn accepts_a_well_formed_change() {
        let c = change(Some("a".to_string()), Some("b".to_string())).unwrap();
        assert_eq!(c.source(), ChangeSource::Git);
        assert_eq!(c.before(), Some("a"));
        assert_eq!(c.after(), Some("b"));
        assert_eq!(c.actor(), Some("alice"));
    }

    #[test]
    fn rejects_empty_before_after_actor() {
        assert!(change(Some(String::new()), None).is_err());
        assert!(change(None, Some(String::new())).is_err());
        let c = Change::new(
            Uuid::new_v4(),
            ChangeSource::Deploy,
            ResourceId::new("service", "api").unwrap(),
            None,
            None,
            Some(String::new()),
            ts(),
        );
        assert!(c.is_err());
    }

    #[test]
    fn serde_round_trip() {
        let c = change(Some("a".to_string()), None).unwrap();
        let json = serde_json::to_string(&c).unwrap();
        let back: Change = serde_json::from_str(&json).unwrap();
        assert_eq!(back, c);
    }
}
