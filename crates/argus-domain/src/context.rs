//! Versioned request context and client principal.

use chrono::{DateTime, Utc};
use semver::Version;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The identity of a client principal, verified out-of-band (e.g. `SO_PEERCRED`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Principal {
    uid: Option<u32>,
    gid: Option<u32>,
}

impl Principal {
    pub fn new(uid: Option<u32>, gid: Option<u32>) -> Self {
        Self { uid, gid }
    }

    /// The principal attributed to a request that arrived over the cloud channel.
    ///
    /// It carries no local uid or gid, because pairing grants cloud identity and
    /// not infrastructure authority: the cloud is a request origin, never an
    /// authorization grant (ADR-0020 §1, §5). A policy that consults the principal
    /// therefore sees an unattributed caller, exactly as it would for any other
    /// unverified client.
    pub fn cloud() -> Self {
        Self {
            uid: None,
            gid: None,
        }
    }

    pub fn uid(&self) -> Option<u32> {
        self.uid
    }

    pub fn gid(&self) -> Option<u32> {
        self.gid
    }
}

/// Versioned context attached to every request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestContext {
    correlation_id: Uuid,
    protocol_version: Version,
    principal: Principal,
    requested_at: DateTime<Utc>,
}

impl RequestContext {
    pub fn new(
        correlation_id: Uuid,
        protocol_version: Version,
        principal: Principal,
        requested_at: DateTime<Utc>,
    ) -> Self {
        Self {
            correlation_id,
            protocol_version,
            principal,
            requested_at,
        }
    }

    pub fn correlation_id(&self) -> Uuid {
        self.correlation_id
    }

    pub fn protocol_version(&self) -> &Version {
        &self.protocol_version
    }

    pub fn principal(&self) -> Principal {
        self.principal
    }

    pub fn requested_at(&self) -> DateTime<Utc> {
        self.requested_at
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    #[test]
    fn serde_round_trip() {
        let ctx = RequestContext::new(
            Uuid::new_v4(),
            Version::new(0, 1, 0),
            Principal::new(Some(1000), Some(1000)),
            Utc.with_ymd_and_hms(2026, 9, 16, 12, 0, 0).unwrap(),
        );
        let json = serde_json::to_string(&ctx).unwrap();
        let back: RequestContext = serde_json::from_str(&json).unwrap();
        assert_eq!(ctx, back);
        assert_eq!(back.protocol_version(), &Version::new(0, 1, 0));
    }

    #[test]
    fn principal_fields() {
        let p = Principal::new(Some(1000), None);
        assert_eq!(p.uid(), Some(1000));
        assert_eq!(p.gid(), None);
    }
}
